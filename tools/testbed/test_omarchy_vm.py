#!/usr/bin/env python3
"""Contract tests for the maintainer Omarchy VM runner."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / "tools/testbed/omarchy-vm.sh"


class OmarchyVmRunnerTests(unittest.TestCase):
    def run_runner(
        self,
        *args: str,
        configured: bool = True,
        remote_root: str = "/home/omarchy/Projects/splinterm-testbed-review",
    ) -> tuple[subprocess.CompletedProcess[str], Path]:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        base = Path(temporary.name)
        fake_bin = base / "bin"
        fake_bin.mkdir()
        log = base / "calls.log"
        identity = base / "id_ed25519"
        known_hosts = base / "known_hosts"
        identity.touch()
        known_hosts.write_text("[127.0.0.1]:2222 ssh-ed25519 test-key\n")

        fake = "#!/bin/sh\nprintf '%s\\n' \"--- $0\" \"$@\" >>\"$CALL_LOG\"\n"
        for command in ("ssh", "rsync"):
            path = fake_bin / command
            path.write_text(fake)
            path.chmod(0o755)

        config = base / "testbed.env"
        if configured:
            config.write_text(
                "\n".join(
                    (
                        "SPLINTERM_TESTBED_HOST=127.0.0.1",
                        "SPLINTERM_TESTBED_PORT=2222",
                        "SPLINTERM_TESTBED_USER=omarchy",
                        f"SPLINTERM_TESTBED_IDENTITY={identity}",
                        f"SPLINTERM_TESTBED_KNOWN_HOSTS={known_hosts}",
                        f"SPLINTERM_TESTBED_REMOTE_ROOT='{remote_root}'",
                    )
                )
                + "\n"
            )

        environment = os.environ.copy()
        environment.update(
            {
                "CALL_LOG": str(log),
                "PATH": f"{fake_bin}:{environment['PATH']}",
                "SPLINTERM_TESTBED_CONFIG": str(config),
            }
        )
        result = subprocess.run(
            [str(RUNNER), *args],
            cwd=ROOT,
            env=environment,
            text=True,
            capture_output=True,
            check=False,
        )
        return result, log

    def test_help_does_not_require_private_configuration(self) -> None:
        result, _ = self.run_runner("--help", configured=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Omarchy VM", result.stdout)

    def test_repository_policy_is_vm_first_for_graphical_testing(self) -> None:
        agents = (ROOT / "AGENTS.md").read_text(encoding="utf-8")
        contributing = (ROOT / "CONTRIBUTING.md").read_text(encoding="utf-8")
        self.assertIn("Run every Splinterm graphical smoke", agents)
        self.assertIn("guest workspace 8 / `Virtual-1`", agents)
        self.assertIn("Host graphical testing is an exception", agents)
        self.assertIn("must run Splinterm graphical smokes", contributing)
        self.assertIn("Watching the QEMU viewer does not authorize", contributing)
        self.assertIn("package-install --confirm-guest-install", agents)
        self.assertIn("This is the trusted-UI path", contributing)

    def test_package_install_requires_explicit_guest_confirmation(self) -> None:
        result, log = self.run_runner("package-install")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("--confirm-guest-install", result.stderr)
        self.assertFalse(log.exists())

    def test_packaged_acceptance_uses_clean_head_and_private_guest_state(self) -> None:
        runner = RUNNER.read_text(encoding="utf-8")
        self.assertIn('git -C "$repo_root" bundle create "$bundle" HEAD', runner)
        self.assertIn("package-build requires a clean committed checkout", runner)
        self.assertIn("./tools/package/upgrade-local-package.sh --yes", runner)
        self.assertIn("/usr/bin/splinterm launch", runner)
        self.assertIn('XDG_STATE_HOME="$state"', runner)
        self.assertIn('XDG_CONFIG_HOME="$config"', runner)
        self.assertIn("splinterm-package-install-check", runner)
        self.assertIn(
            'SPLINTERM_SOCKET="$socket" /usr/bin/splinterm list', runner
        )
        self.assertIn("--exclude=/.testbed-package/", runner)
        active_guard = "packaged test is already active; run package-stop first"
        self.assertIn(active_guard, runner)
        package_launch = runner.split("  package-launch)", 1)[1].split(
            "  package-stop)", 1
        )[0]
        self.assertLess(
            package_launch.index(active_guard),
            package_launch.index('rm -rf "$runtime"'),
        )

    def test_graphical_launches_use_guarded_guest_window_lifecycle(self) -> None:
        runner = RUNNER.read_text(encoding="utf-8")
        self.assertEqual(runner.count("guest-window.py prepare"), 1)
        self.assertEqual(runner.count("guest-window.py place"), 1)
        self.assertGreaterEqual(runner.count("guest-window.py restore"), 2)
        self.assertIn('"$package_root/source/tools/testbed/guest-window.py" prepare', runner)
        self.assertIn('"$package_root/source/tools/testbed/guest-window.py" place', runner)
        self.assertIn(
            '"$SPLINTERM_TESTBED_PACKAGE_ROOT/source/tools/testbed/guest-window.py" restore',
            runner,
        )
        self.assertIn("expected exactly one Hyprland instance", runner)
        development_launch = runner.split("  launch)", 1)[1].split("  stop)", 1)[0]
        self.assertIn('kill "$client_pid"', development_launch)
        self.assertIn('wait "$client_pid"', development_launch)

    def test_status_uses_pinned_noninteractive_ssh(self) -> None:
        result, log = self.run_runner("status")
        self.assertEqual(result.returncode, 0, result.stderr)
        call = log.read_text()
        self.assertIn("BatchMode=yes", call)
        self.assertIn("IdentitiesOnly=yes", call)
        self.assertIn("StrictHostKeyChecking=yes", call)
        self.assertIn("UserKnownHostsFile=", call)
        self.assertNotIn("StrictHostKeyChecking=no", call)

    def test_exec_quotes_remote_checkout_and_arguments(self) -> None:
        result, log = self.run_runner("exec", "printf", "%s", "two words")
        self.assertEqual(result.returncode, 0, result.stderr)
        call = log.read_text()
        self.assertIn("cd /home/omarchy/Projects/splinterm-testbed-review", call)
        self.assertIn("exec printf %s two\\ words", call)

    def test_desktop_exec_discovers_guest_wayland_environment(self) -> None:
        result, log = self.run_runner("desktop-exec", "wtype", "--", "two words")
        self.assertEqual(result.returncode, 0, result.stderr)
        call = log.read_text()
        self.assertIn("hyprctl instances -j", call)
        self.assertIn("XDG_RUNTIME_DIR=", call)
        self.assertIn("WAYLAND_DISPLAY=", call)
        self.assertIn("HYPRLAND_INSTANCE_SIGNATURE=", call)
        self.assertIn("exec wtype -- two\\ words", call)

    def test_input_uses_private_guest_runtime_socket(self) -> None:
        result, log = self.run_runner("input", "type", "two words")
        self.assertEqual(result.returncode, 0, result.stderr)
        call = log.read_text()
        self.assertIn("/run/user/$(id -u)/.ydotool_socket", call)
        self.assertIn("YDOTOOL_SOCKET=", call)
        self.assertIn("exec ydotool type two\\ words", call)

    def test_ping_and_launch_use_the_synced_checkout_as_repo(self) -> None:
        result, log = self.run_runner("ping")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(
            "SPLINTERM_REPO=/home/omarchy/Projects/splinterm-testbed-review",
            log.read_text(),
        )
        self.assertIn(
            'SPLINTERM_REPO="$SPLINTERM_TESTBED_ROOT"',
            RUNNER.read_text(encoding="utf-8"),
        )

    def test_sync_deletes_only_inside_dedicated_remote_root(self) -> None:
        result, log = self.run_runner("sync")
        self.assertEqual(result.returncode, 0, result.stderr)
        call = log.read_text()
        self.assertIn("parent=/home/omarchy/Projects", call)
        self.assertIn("root=/home/omarchy/Projects/splinterm-testbed-review", call)
        self.assertIn('test ! -L "$parent"', call)
        self.assertIn('test ! -L "$root"', call)
        self.assertIn("--delete", call)
        self.assertIn("--exclude=/target/", call)
        self.assertIn("--exclude=/.git/", call)
        self.assertIn("--exclude=/.env", call)
        self.assertIn("--exclude=/.splinterm-testbed.env", call)
        self.assertIn(
            "omarchy@127.0.0.1:/home/omarchy/Projects/splinterm-testbed-review/",
            call,
        )

    def test_unsafe_sync_roots_fail_before_remote_commands(self) -> None:
        unsafe_roots = (
            "/",
            "/home/omarchy",
            "/home/omarchy/Projects",
            "/home/omarchy/Projects/ordinary-project",
            "/home/omarchy/Projects/splinterm-testbed/..",
        )
        for remote_root in unsafe_roots:
            with self.subTest(remote_root=remote_root):
                result, log = self.run_runner("sync", remote_root=remote_root)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("must be", result.stderr)
                self.assertFalse(log.exists())


if __name__ == "__main__":
    unittest.main()
