"""Guard current-Arch compiler coverage and warning-strict package builds."""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class RustWarningGateTests(unittest.TestCase):
    def test_current_arch_gate_is_required_and_checks_all_release_targets(self):
        text = (ROOT / ".github/workflows/ci.yml").read_text()
        job = text.split("\n  arch-warning-check:\n", 1)[1].split("\n  check:\n", 1)[0]
        self.assertIn("container: archlinux:base-devel", job)
        self.assertIn("pacman -Syu --noconfirm --needed git rust", job)
        self.assertIn('RUSTFLAGS: "-D warnings"', job)
        self.assertIn("cargo check --locked --workspace --all-targets", job)
        self.assertIn("cargo check --locked --release --workspace", job)
        self.assertNotIn("continue-on-error", job)
        self.assertNotRegex(job, r"(?m)^\s+if:")
        check = text[text.index("\n  check:\n") :]
        self.assertIn("\n      - arch-warning-check\n", check)
        self.assertIn("needs.arch-warning-check.result != 'success'", check)
        self.assertIn("needs.arch-warning-check.result == 'success'", check)
        self.assertIn("dtolnay/rust-toolchain@1.88.0", text)
        self.assertIn("tools/release/test_rust_warning_gates.py", text)

    def test_package_builds_deny_warnings_and_preserve_user_flags(self):
        script = '''
set -eu
source "$1"
cargo() { printf '%s\\n' "$RUSTFLAGS" "$*"; }
mkdir -p "splinterm-${_upstream_ver:-$pkgver}"
build
'''
        for recipe in ("packaging/PKGBUILD", "packaging/aur/PKGBUILD"):
            for flags in ("", "-C debuginfo=1"):
                with self.subTest(recipe=recipe, flags=flags), tempfile.TemporaryDirectory() as tmp:
                    # build-local-package.sh supplies these for the local recipe.
                    env = dict(
                        os.environ,
                        RUSTFLAGS=flags,
                        SPLINTERM_BUILD_COMMIT="1" * 40,
                        SPLINTERM_BUILD_PROVENANCE="local-committed-head",
                    )
                    result = subprocess.run(
                        ["bash", "-c", script, "bash", str(ROOT / recipe)],
                        cwd=tmp, env=env, text=True, capture_output=True, check=True,
                    )
                    lines = result.stdout.splitlines()
                    self.assertEqual(lines[0], f"{flags} -D warnings".strip())
                    self.assertEqual(
                        lines[1],
                        "build --frozen --release -p splinterm -p splinterd "
                        "-p splinterm-relay -p splinterm-pty -p splinterm-mcp",
                    )

    def test_local_package_build_requires_declared_provenance(self):
        script = '''
set -eu
source "$1"
cargo() { :; }
mkdir -p "splinterm-$pkgver"
build
'''
        env = {
            key: value
            for key, value in os.environ.items()
            if key not in ("SPLINTERM_BUILD_COMMIT", "SPLINTERM_BUILD_PROVENANCE")
        }
        with tempfile.TemporaryDirectory() as tmp:
            result = subprocess.run(
                ["bash", "-c", script, "bash", str(ROOT / "packaging/PKGBUILD")],
                cwd=tmp, env=env, text=True, capture_output=True, check=False,
            )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("package build requires exact source commit", result.stderr)


if __name__ == "__main__":
    unittest.main()
