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
FC_TOOLS = ("fc-match", "fc-cache", "fc-scan")


def fontconfig_query(pattern_format: str, environment: dict[str, str]) -> str | None:
    result = subprocess.run(
        ["fc-match", "-f", pattern_format, "monospace:style=Regular"],
        env=environment,
        text=True,
        capture_output=True,
        check=False,
    )
    value = result.stdout.strip()
    return value if result.returncode == 0 and value else None


class FontFixtureTests(unittest.TestCase):
    def environment(self, cache_home: Path, config_home: Path | None = None) -> dict[str, str]:
        environment = os.environ.copy()
        environment.pop("FONTCONFIG_FILE", None)
        environment["XDG_CACHE_HOME"] = str(cache_home)
        if config_home is not None:
            environment["XDG_CONFIG_HOME"] = str(config_home)
        return environment

    def require_ambient_monospace(self, environment: dict[str, str]) -> tuple[Path, str]:
        if not all(shutil.which(tool) for tool in FC_TOOLS):
            self.skipTest("Fontconfig tools are unavailable")
        path = fontconfig_query("%{file}", environment)
        family = fontconfig_query("%{family[0]}", environment)
        if path is None or family is None or not Path(path).is_file():
            self.skipTest("no installed monospace font")
        return Path(path), family

    def run_tool(
        self, *arguments: str, environment: dict[str, str]
    ) -> subprocess.CompletedProcess[str]:
        environment = dict(environment)
        # Reproducible-build shells export this; the tool must cope with it.
        environment["SOURCE_DATE_EPOCH"] = "315532800"
        return subprocess.run(
            [sys.executable, str(TOOL), *arguments],
            env=environment,
            text=True,
            capture_output=True,
            check=False,
        )

    def assert_session_resolves_fixture(self, output: Path, cache_home: Path) -> None:
        summary = json.loads((output / "summary.json").read_text())
        font = Path(summary["font"])
        self.assertEqual(summary["monospace_first_match"], str(font))
        self.assertEqual(font.read_bytes(), bytes(8))
        # The packaged session's view: no FONTCONFIG_FILE, a private config home.
        environment = self.environment(cache_home, Path(summary["session_config_home"]))
        environment.pop("SOURCE_DATE_EPOCH", None)
        self.assertEqual(fontconfig_query("%{file}", environment), str(font))

    def build(self, base: Path, environment: dict[str, str]) -> Path:
        output = base / "fixture"
        result = self.run_tool("invalid-candidate", "--output", str(output), environment=environment)
        self.assertEqual(result.returncode, 0, result.stderr)
        (output / "summary.json").write_text(result.stdout)
        return output

    def test_rejects_existing_output_and_non_font_sources(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            environment = self.environment(base / "cache")
            existing = base / "existing"
            existing.mkdir()
            result = self.run_tool(
                "invalid-candidate", "--output", str(existing), environment=environment
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
                environment=environment,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("not a font file", result.stderr)
            self.assertFalse((base / "fixture").exists())

    def test_monospace_resolves_to_a_cached_face_that_is_no_longer_a_font(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            cache_home = base / "xdg-cache"
            cache_home.mkdir()
            environment = self.environment(cache_home)
            self.require_ambient_monospace(environment)
            output = self.build(base, environment)
            self.assert_session_resolves_fixture(output, cache_home)
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
            for path in (output, output / "fonts", output / "cache", output / "xdg-config"):
                self.assertEqual(path.stat().st_mode & 0o777, 0o700)
            user_config = (output / "fonts.conf").read_text(encoding="utf-8")
            self.assertIn('binding="strong"', user_config)
            self.assertIn("<cachedir>", user_config)
            self.assertNotIn("<include", user_config)
            self.assertFalse((output / "verify.conf").exists())

    def test_an_ambient_strong_monospace_choice_does_not_defeat_verification(self) -> None:
        # An Omarchy user configuration strongly prepends its family to
        # monospace. The packaged session never loads it, so neither may the
        # fixture's verification.
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            cache_home = base / "xdg-cache"
            cache_home.mkdir()
            _, family = self.require_ambient_monospace(self.environment(cache_home))
            ambient = base / "ambient-config"
            (ambient / "fontconfig").mkdir(parents=True)
            (ambient / "fontconfig" / "fonts.conf").write_text(
                '<?xml version="1.0"?>\n<fontconfig>\n'
                '  <match target="pattern">\n'
                '    <test qual="any" name="family"><string>monospace</string></test>\n'
                '    <edit name="family" mode="prepend" binding="strong">'
                f"<string>{family}</string></edit>\n"
                "  </match>\n</fontconfig>\n",
                encoding="utf-8",
            )
            output = self.build(base, self.environment(cache_home, ambient))
            self.assert_session_resolves_fixture(output, cache_home)


if __name__ == "__main__":
    unittest.main()
