"""Check runnable documentation examples without contacting a daemon.

Requires Bash and jq. Shell examples run only in a temporary HOME.
"""

from __future__ import annotations

import json
import os
import re
import shlex
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SITE = ROOT / "site/src/content/docs/docs"


def shell_blocks(path: Path) -> list[str]:
    return re.findall(r"```bash\n(.*?)\n```", path.read_text(), re.DOTALL)


def block_containing(path: Path, marker: str) -> str:
    matches = [block for block in shell_blocks(path) if marker in block]
    if len(matches) != 1:
        raise AssertionError(f"{path}: expected one shell block containing {marker!r}")
    return matches[0]


class DocumentationExamplesTests(unittest.TestCase):
    def test_scrollback_examples_respect_protocol_page_limit(self) -> None:
        source = (ROOT / "crates/splinterm-protocol/src/lib.rs").read_text()
        match = re.search(r"MAX_SCROLLBACK_PAGE_ROWS: usize = (\d+);", source)
        self.assertIsNotNone(match)
        maximum = int(match[1])
        examples = 0
        for name in ("cli.md", "usage.md"):
            for block in shell_blocks(ROOT / "docs" / name):
                for line in block.splitlines():
                    if line.startswith("splinterm scrollback "):
                        args = shlex.split(line)
                        rows = int(args[args.index("--max-rows") + 1])
                        self.assertTrue(1 <= rows <= maximum, (name, line))
                        examples += 1
        self.assertEqual(examples, 2)

    def test_policy_examples_supply_required_file_argument(self) -> None:
        examples = 0
        for path in (ROOT / "docs/mcp.md", ROOT / "docs/headless.md", SITE / "mcp.md"):
            for block in shell_blocks(path):
                for line in block.splitlines():
                    if not line.startswith("splinterm policy "):
                        continue
                    args = shlex.split(line)
                    if args[2] in ("validate", "inspect"):
                        self.assertEqual(len(args), 4, (path, line))
                        self.assertTrue(args[3].endswith("/policy.json"), (path, line))
                        examples += 1
        self.assertEqual(examples, 6)

    def test_context_filter_accepts_current_and_rejects_stale_context(self) -> None:
        self.assertIsNotNone(shutil.which("jq"), "documentation examples require jq")
        block = block_containing(ROOT / "docs/integrations.md", "any(.data.splints[];")
        schema = json.loads(
            (ROOT / "dist/schemas/v2/cli-envelope.schema.json").read_text()
        )
        # Bind the example's field name to the public contract, not a second handwritten DTO.
        self.assertIn(
            "current_incarnation", schema["$defs"]["splint_summary"]["required"]
        )
        splint = {
            "lair_id": "L",
            "dojo_id": "D",
            "splint_id": "S",
            "current_incarnation": 1,
            "lifecycle": "running",
        }
        env = {
            **os.environ,
            "SPLINTERM_LAIR_ID": "L",
            "SPLINTERM_DOJO_ID": "D",
            "SPLINTERM_SPLINT_ID": "S",
            "SPLINTERM_SPLINT_INCARNATION": "1",
        }
        cases = [
            ({}, True),
            ({"current_incarnation": 2}, False),
            ({"lifecycle": "exited"}, False),
            ({"splint_id": "other"}, False),
            ({"lair_id": "other"}, False),
            ({"dojo_id": "other"}, False),
        ]
        for changes, accepted in cases:
            with self.subTest(changes=changes):
                env["topology"] = json.dumps(
                    {"data": {"splints": [{**splint, **changes}]}}
                )
                result = subprocess.run(
                    ["bash", "-eu", "-c", block],
                    check=False,
                    env=env,
                    capture_output=True,
                    text=True,
                    timeout=5,
                )
                self.assertEqual(result.returncode, 0 if accepted else 1, result.stderr)

    def test_initial_policy_setup_preserves_existing_files_and_symlinks(self) -> None:
        block = block_containing(ROOT / "docs/headless.md", "env_file=")
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            env = {**os.environ, "HOME": str(home)}
            policy = home / ".config/splinterm/policy.json"
            daemon_env = policy.with_name("daemon.env")

            def run() -> subprocess.CompletedProcess[str]:
                return subprocess.run(
                    ["bash", "-c", block],
                    check=False,
                    env=env,
                    capture_output=True,
                    text=True,
                    timeout=5,
                )

            result = run()
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(
                json.loads(policy.read_text()),
                {"schema": "splinterm.policy.v2", "rules": []},
            )
            self.assertEqual(daemon_env.read_text(), f"SPLINTERM_POLICY={policy}\n")
            for path in (policy, daemon_env):
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            policy.write_text("existing reviewed rules\n")
            daemon_env.write_text("existing service settings\n")
            self.assertNotEqual(run().returncode, 0)
            self.assertEqual(policy.read_text(), "existing reviewed rules\n")
            self.assertEqual(daemon_env.read_text(), "existing service settings\n")
            policy.unlink()
            self.assertNotEqual(run().returncode, 0)
            self.assertFalse(
                policy.exists(), "existing daemon.env must prevent partial setup"
            )
            daemon_env.unlink()
            policy.symlink_to(home / "missing-policy")
            self.assertNotEqual(run().returncode, 0)
            self.assertTrue(policy.is_symlink())
            self.assertFalse(daemon_env.exists())


if __name__ == "__main__":
    unittest.main()
