#!/usr/bin/env python3
"""Regression tests for installation from daemon-owned shells."""

from __future__ import annotations

import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]


class InstallerSafetyTest(unittest.TestCase):
    def test_package_validator_private_protocol_pin_matches_rust_authority(self) -> None:
        protocol = (ROOT / "crates/splinterm-protocol/src/lib.rs").read_text(
            encoding="utf-8"
        )
        validator = (ROOT / "tools/package/validate-package.py").read_text(
            encoding="utf-8"
        )
        rust_version = re.search(
            r"^pub const PROTOCOL_VERSION: u16 = ([0-9]+);$", protocol, re.MULTILINE
        )
        validator_version = re.search(
            r"^PRIVATE_PROTOCOL_VERSION = ([0-9]+)$", validator, re.MULTILINE
        )
        self.assertIsNotNone(rust_version)
        self.assertIsNotNone(validator_version)
        self.assertEqual(validator_version.group(1), rust_version.group(1))

    def test_fake_ssh_fixture_hello_matches_rust_protocol_authority(self) -> None:
        # The remote-session tests' fake daemon must greet with the current
        # private protocol, or every remote test fails the handshake.
        protocol = (ROOT / "crates/splinterm-protocol/src/lib.rs").read_text(
            encoding="utf-8"
        )
        fixture = (ROOT / "crates/splinterm/tests/fixtures/fake_ssh.py").read_text(
            encoding="utf-8"
        )
        rust_version = re.search(
            r"^pub const PROTOCOL_VERSION: u16 = ([0-9]+);$", protocol, re.MULTILINE
        )
        self.assertIsNotNone(rust_version)
        hello_versions = re.findall(
            r"'type': 'hello',\s*'version': ([0-9]+),", fixture
        )
        self.assertTrue(hello_versions)
        self.assertEqual(set(hello_versions), {rust_version.group(1)})

    def test_release_bytes_are_stripped_once_before_provenance_hashing(self) -> None:
        cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
        self.assertIn('strip = "symbols"', cargo)
        for recipe in (
            ROOT / "packaging/PKGBUILD",
            ROOT / "packaging/aur/PKGBUILD",
            ROOT / "packaging/aur-bin/PKGBUILD",
        ):
            source = recipe.read_text(encoding="utf-8")
            self.assertEqual(source.count("options=("), 1)
            self.assertIn("'!strip'", source)

    def test_package_build_version_matches_workspace_authority(self) -> None:
        cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
        package = (ROOT / "packaging/PKGBUILD").read_text(encoding="utf-8")
        cargo_version = re.search(r'^version = "([^"]+)"$', cargo, re.MULTILINE)
        package_version = re.search(r"^_build_version=([^\n]+)$", package, re.MULTILINE)
        self.assertIsNotNone(cargo_version)
        self.assertIsNotNone(package_version)
        self.assertEqual(cargo_version.group(1), package_version.group(1))

    def test_package_provenance_is_generated_by_both_source_recipes(self) -> None:
        private = (ROOT / "packaging/PKGBUILD").read_text(encoding="utf-8")
        aur = (ROOT / "packaging/aur/PKGBUILD").read_text(encoding="utf-8")
        build = (ROOT / "tools/package/build-local-package.sh").read_text(
            encoding="utf-8"
        )
        workflow = (ROOT / ".github/workflows/release-candidate.yml").read_text(
            encoding="utf-8"
        )
        for recipe in (private, aur):
            self.assertIn("generate-package-provenance.py", recipe)
            self.assertIn("package-provenance-v1.json", recipe)
            # The generator hashes, and refuses integration-test builds of,
            # the files the package actually installs (Plan 0061 D9).
            self.assertIn('--daemon "$pkgdir/usr/bin/splinterd"', recipe)
            self.assertIn('--client "$pkgdir/usr/bin/splinterm"', recipe)
            self.assertNotIn("--daemon target/release/", recipe)
            self.assertNotIn("--client target/release/", recipe)
        self.assertIn("git -C \"$root\" rev-parse --verify HEAD", build)
        self.assertIn("SPLINTERM_BUILD_COMMIT=$commit", build)
        self.assertIn("local-committed-head", build)
        self.assertIn("SPLINTERM_BUILD_PROVENANCE=release-candidate", workflow)

    def test_no_shipped_recipe_or_workflow_enables_vm_acceptance(self) -> None:
        # Plan 0061 M3.17: the test-only live-handoff feature is reachable only
        # through the explicit local build flag, never a shipped recipe.
        shipped = [
            ROOT / "packaging/PKGBUILD",
            ROOT / "packaging/aur/PKGBUILD",
            ROOT / "packaging/aur-bin/PKGBUILD",
            ROOT / "nix/package.nix",
            ROOT / "nix/module.nix",
            ROOT / "flake.nix",
            ROOT / "install.sh",
            *sorted((ROOT / ".github/workflows").glob("*.yml")),
        ]
        for path in shipped:
            with self.subTest(path=path.relative_to(ROOT)):
                source = path.read_text(encoding="utf-8")
                self.assertNotIn("vm-acceptance-live-handoff", source)
                self.assertNotIn("VM_ACCEPTANCE", source)
        cargo = (ROOT / "crates/splinterd/Cargo.toml").read_text(encoding="utf-8")
        self.assertIn("vm-acceptance-live-handoff = []", cargo)
        self.assertNotRegex(cargo, r"default\s*=\s*\[[^\]]*vm-acceptance")
        for manifest in ROOT.glob("crates/*/Cargo.toml"):
            if manifest.parent.name == "splinterd":
                continue
            with self.subTest(manifest=manifest.relative_to(ROOT)):
                self.assertNotIn(
                    "vm-acceptance", manifest.read_text(encoding="utf-8")
                )

    def test_vm_acceptance_buildscript_changes_exactly_the_build(self) -> None:
        build = (ROOT / "tools/package/build-local-package.sh").read_text(
            encoding="utf-8"
        )
        function = re.search(
            r"^derive_vm_acceptance_buildscript\(\) \{\n.*?^\}\n",
            build,
            re.MULTILINE | re.DOTALL,
        )
        self.assertIsNotNone(function)
        source = ROOT / "packaging/PKGBUILD"
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "PKGBUILD.vm-acceptance-live-handoff"
            subprocess.run(
                [
                    "bash", "-c",
                    function.group(0) + 'derive_vm_acceptance_buildscript "$1" "$2"',
                    "_", str(source), str(target),
                ],
                check=True,
            )
            original = source.read_text(encoding="utf-8").splitlines()
            derived = target.read_text(encoding="utf-8").splitlines()
            self.assertEqual(
                [line for line in derived if line not in original],
                [
                    "  cargo build --frozen --release --features splinterd/vm-acceptance-live-handoff \\",
                    "  pkgdesc='TEST-ONLY VM acceptance build (live handoff forced on): never ship'",
                    "    --allow-vm-acceptance-live-handoff \\",
                ],
            )
            self.assertEqual(
                [line for line in original if line not in derived],
                [
                    "  cargo build --frozen --release \\",
                    "  pkgdesc='Persistent Wayland terminal for humans and bounded automation'",
                ],
            )
            # The derived script still parses and names the same packages.
            names = subprocess.run(
                ["bash", "-c", 'source "$1"; printf "%s\n" "${pkgname[@]}"', "_", str(target)],
                check=True,
                capture_output=True,
                text=True,
            ).stdout.split()
            self.assertEqual(names, ["splinterm", "splinterm-mcp"])
            # A changed recipe is refused rather than silently built without
            # the feature.
            changed = Path(directory) / "PKGBUILD.changed"
            changed.write_text(
                source.read_text(encoding="utf-8").replace(
                    "  cargo build --frozen --release \\\n", "  cargo build --release \\\n"
                ),
                encoding="utf-8",
            )
            refused = subprocess.run(
                [
                    "bash", "-c",
                    function.group(0) + 'derive_vm_acceptance_buildscript "$1" "$2"',
                    "_", str(changed), str(target) + ".refused",
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn("PKGBUILD changed", refused.stderr)

    def test_vm_acceptance_build_refuses_release_provenance(self) -> None:
        environment = os.environ.copy()
        environment["SPLINTERM_BUILD_PROVENANCE"] = "release-candidate"
        result = subprocess.run(
            [
                str(ROOT / "tools/package/build-local-package.sh"),
                "--vm-acceptance-live-handoff",
                "--skip-system-dependency-check",
            ],
            cwd=ROOT,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 1, result)
        self.assertIn("needs local-committed-head provenance", result.stderr)

    def test_vm_acceptance_install_is_refused_outside_the_testbed(self) -> None:
        environment = os.environ.copy()
        environment.pop("SPLINTERM_TESTBED_PACKAGE_ROOT", None)
        result = subprocess.run(
            [
                str(ROOT / "tools/package/upgrade-local-package.sh"),
                "--yes",
                "--allow-vm-acceptance-live-handoff",
            ],
            cwd=ROOT,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 1, result)
        self.assertIn("outside the VM testbed", result.stderr)
        self.assertEqual(result.stdout, "")
        # The testbed's package-install sets it for the remote installer.
        runner = (ROOT / "tools/testbed/omarchy-vm.sh").read_text(encoding="utf-8")
        install = runner.split("  package-install)", 1)[1].split("  package-launch)", 1)[0]
        self.assertIn('"SPLINTERM_TESTBED_PACKAGE_ROOT=$(printf', install)
        self.assertIn('package_root=$SPLINTERM_TESTBED_PACKAGE_ROOT', install)

    def test_provenance_protocol_constants_match_all_authorities(self) -> None:
        generator = (ROOT / "tools/package/generate-package-provenance.py").read_text(
            encoding="utf-8"
        )
        validator = (ROOT / "tools/package/validate-package.py").read_text(
            encoding="utf-8"
        )
        protocol = (ROOT / "crates/splinterm-protocol/src/lib.rs").read_text(
            encoding="utf-8"
        )
        rust_version = re.search(
            r"^pub const PROTOCOL_VERSION: u16 = ([0-9]+);$", protocol, re.MULTILINE
        )
        self.assertIsNotNone(rust_version)
        self.assertIn(f"PRIVATE_PROTOCOL = {rust_version.group(1)}", generator)
        preflight = (ROOT / "crates/splinterd/src/handoff_preflight.rs").read_text(
            encoding="utf-8"
        )
        handoff_versions = []
        for rust_name, python_name in (
            ("HANDOFF_PROTOCOL_VERSION", "HANDOFF_PROTOCOL"),
            ("TERMINAL_CHECKPOINT_VERSION", "TERMINAL_CHECKPOINT"),
            ("DESCRIPTOR_MANIFEST_VERSION", "DESCRIPTOR_MANIFEST"),
        ):
            rust_value = re.search(
                rf"^const {rust_name}: u16 = ([0-9]+);$", preflight, re.MULTILINE
            )
            self.assertIsNotNone(rust_value, rust_name)
            self.assertIn(f"{python_name} = {rust_value.group(1)}", generator)
            handoff_versions += [rust_value.group(1)] * 2
        self.assertIn(
            "PACKAGE_PAIR_CAPABILITIES = (\n"
            "    PRIVATE_PROTOCOL_VERSION, PRIVATE_PROTOCOL_VERSION, "
            f"{', '.join(handoff_versions)}\n)",
            validator,
        )
        self.assertIn("for value in PACKAGE_PAIR_CAPABILITIES:", validator)

    def daemon_owned_environment(self, directory: Path) -> dict[str, str]:
        proc = directory / "proc"
        binaries = directory / "bin"
        binary = binaries / "splinterm-pty-child"
        binaries.mkdir()
        for executable in (binary, binaries / "sudo", binaries / "pacman"):
            executable.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            executable.chmod(0o700)
        for pid, parent, executable in (
            (101, 100, Path("/usr/bin/bash")),
            (100, 1, binary),
        ):
            process = proc / str(pid)
            process.mkdir(parents=True)
            (process / "status").write_text(
                f"Name:\ttest\nPPid:\t{parent}\n", encoding="utf-8"
            )
            (process / "exe").symlink_to(executable)
        environment = os.environ.copy()
        environment.update(
            {
                "SPLINTERM_INSTALL_PROC_ROOT": str(proc),
                "SPLINTERM_INSTALL_PARENT_PID": "101",
                "PATH": f"{binaries}:{os.environ['PATH']}",
            }
        )
        return environment

    def assert_refuses(self, script: Path, *arguments: str) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-installer-safety-") as value:
            result = subprocess.run(
                [str(script), *arguments],
                cwd=ROOT,
                env=self.daemon_owned_environment(Path(value)),
                text=True,
                capture_output=True,
                check=False,
            )
        self.assertEqual(result.returncode, 1, result)
        self.assertEqual(result.stdout, "")
        self.assertIn("Refusing to install from inside a Splinterm-owned shell", result.stderr)
        self.assertIn("Run the installer from Foot", result.stderr)

    def test_source_installer_refuses_before_build_or_service_stop(self) -> None:
        self.assert_refuses(ROOT / "install.sh", "--source", "--yes")

    def test_local_upgrade_refuses_before_package_or_service_work(self) -> None:
        self.assert_refuses(
            ROOT / "tools/package/upgrade-local-package.sh", "--yes"
        )

    def test_unattended_install_never_opts_into_user_screensaver_integration(self) -> None:
        source = (ROOT / "install.sh").read_text(encoding="utf-8")
        self.assertIn("if [[ $assume_yes == true || ! -t 0 ]]; then", source)
        unattended = source.split(
            "if [[ $assume_yes == true || ! -t 0 ]]; then", maxsplit=1
        )[1].split("fi", maxsplit=1)[0]
        self.assertIn("was not enabled", unattended)
        self.assertNotIn("omarchy-screensaver enable\n", unattended)

    def test_release_installer_binds_package_provenance_before_pacman(self) -> None:
        source = (ROOT / "install.sh").read_text(encoding="utf-8")
        provenance = source.index("inspect-package-provenance")
        package_query = source.index('pacman -Qp "$download_dir/$main_asset"')
        installation = source.rindex('sudo pacman "${pacman_args[@]}"')
        self.assertLess(provenance, package_query)
        self.assertLess(provenance, installation)
        self.assertIn("Downloaded package build provenance does not match", source)

    def test_release_installer_requires_provenance_from_0_2_0(self) -> None:
        source = (ROOT / "install.sh").read_text(encoding="utf-8")
        function = re.search(
            r"^release_has_provenance\(\) \{\n.*?^\}\n", source, re.MULTILINE | re.DOTALL
        )
        self.assertIsNotNone(function)
        for version, required in [
            ("0.1.1", False),
            ("0.1.0-beta1", False),
            ("0.2.0", True),
            ("0.2.0-rc.1", True),
            ("0.10.0", True),
            ("1.0.0", True),
            ("0.08.0", True),
        ]:
            with self.subTest(version=version):
                result = subprocess.run(
                    ["bash", "-c", function.group(0) + 'release_has_provenance "$1"', "_", version],
                    check=False,
                )
                self.assertEqual(result.returncode == 0, required)
        malformed = subprocess.run(
            ["bash", "-c", function.group(0) + 'release_has_provenance "$1"; echo reached', "_", "x.y"],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(malformed.returncode, 0)
        self.assertNotIn("reached", malformed.stdout)

    def test_installer_never_replaces_an_existing_user_launcher(self) -> None:
        source = (ROOT / "install.sh").read_text(encoding="utf-8")
        collision = source.split(
            "*'incomplete or changed'*)", maxsplit=1
        )[1].split("if [[ $assume_yes", maxsplit=1)[0]
        self.assertIn("Splinterm did not modify or replace it", collision)
        self.assertIn("return 0", collision)
        for forbidden in ("rm -f \"$launcher\"", "mv \"$launcher\"", "ln -sf"):
            self.assertNotIn(forbidden, source)


if __name__ == "__main__":
    unittest.main()
