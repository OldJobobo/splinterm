#!/usr/bin/env python3
"""Tests for the private invalid live-font candidate fixture."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


TOOL = Path(__file__).with_name("font-fixture.py")


def ambient_monospace() -> str | None:
    if not all(shutil.which(tool) for tool in ("fc-match", "fc-cache", "fc-scan")):
        return None
    result = subprocess.run(
        ["fc-match", "-f", "%{file}", "monospace:style=Regular"],
        text=True,
        capture_output=True,
        check=False,
    )
    path = result.stdout.strip()
    return path if result.returncode == 0 and Path(path).is_file() else None


class FontFixtureTests(unittest.TestCase):
    def run_tool(self, *arguments: str, cache_home: Path) -> subprocess.CompletedProcess[str]:
        environment = os.environ.copy()
        environment["XDG_CACHE_HOME"] = str(cache_home)
        # Reproducible-build shells export this; the tool must cope with it.
        environment["SOURCE_DATE_EPOCH"] = "315532800"
        return subprocess.run(
            [sys.executable, str(TOOL), *arguments],
            env=environment,
            text=True,
            capture_output=True,
            check=False,
        )

    def test_rejects_existing_output_and_non_font_sources(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            existing = base / "existing"
            existing.mkdir()
            result = self.run_tool(
                "invalid-candidate", "--output", str(existing), cache_home=base / "cache"
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("already exists", result.stderr)
            source = base / "notes.txt"
            source.write_text("not a font")
            result = self.run_tool(
                "invalid-candidate",
                "--output",
                str(base / "fixture"),
                "--source",
                str(source),
                cache_home=base / "cache",
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("not a font file", result.stderr)
            self.assertFalse((base / "fixture").exists())

    def test_monospace_resolves_to_a_cached_face_that_is_no_longer_a_font(self) -> None:
        if ambient_monospace() is None:
            self.skipTest("Fontconfig tools or an installed monospace font are unavailable")
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            cache_home = base / "xdg-cache"
            cache_home.mkdir()
            output = base / "fixture"
            result = self.run_tool(
                "invalid-candidate", "--output", str(output), cache_home=cache_home
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            summary = json.loads(result.stdout)
            font = Path(summary["font"])
            self.assertEqual(summary["monospace_first_match"], str(font))
            self.assertEqual(summary["family"], "Splinterm Invalid Candidate")
            self.assertEqual(font.read_bytes(), bytes(8))
            self.assertTrue(any(".cache-" in path.name for path in (output / "cache").iterdir()))
            # Ambient lookups may cache system font directories in an empty
            # cache home, but the fixture itself is cached only privately.
            fixture_directory = str(output / "fonts").encode()
            self.assertEqual(
                [
                    path
                    for path in cache_home.rglob("*")
                    if path.is_file() and fixture_directory in path.read_bytes()
                ],
                [],
            )
            for path in (output, output / "fonts", output / "cache"):
                self.assertEqual(path.stat().st_mode & 0o777, 0o700)
            user_config = (output / "fonts.conf").read_text(encoding="utf-8")
            self.assertIn('binding="strong"', user_config)
            self.assertIn("<cachedir>", user_config)
            self.assertNotIn("<include", user_config)

            environment = os.environ.copy()
            environment.pop("SOURCE_DATE_EPOCH", None)
            environment["XDG_CACHE_HOME"] = str(cache_home)
            environment["FONTCONFIG_FILE"] = summary["verify_config"]
            resolved = subprocess.run(
                ["fc-match", "-f", "%{file}", "monospace:style=Regular"],
                env=environment,
                text=True,
                capture_output=True,
                check=True,
            )
            self.assertEqual(resolved.stdout, str(font))


if __name__ == "__main__":
    unittest.main()
