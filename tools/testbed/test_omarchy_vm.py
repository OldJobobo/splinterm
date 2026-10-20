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
        root_override: str | None = None,
        input_text: str | None = None,
        locale: str | None = None,
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
        environment.pop("SPLINTERM_TESTBED_REMOTE_ROOT", None)
        if root_override is not None:
            environment["SPLINTERM_TESTBED_REMOTE_ROOT"] = root_override
        if locale is not None:
            environment.update({"LANG": locale, "LC_ALL": locale})
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
            input=input_text,
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

    def test_vm_acceptance_flags_are_explicit_and_test_only(self) -> None:
        # Plan 0061 M3.17: only an explicit flag builds or installs the
        # test-only live-handoff package.
        result, log = self.run_runner("package-build", "--bogus")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("--vm-acceptance-live-handoff", result.stderr)
        self.assertFalse(log.exists())
        for arguments in (
            ("package-install", "--vm-acceptance-live-handoff"),
            ("package-install", "--vm-acceptance-live-handoff", "--confirm-guest-install"),
            ("package-install", "--confirm-guest-install", "--bogus"),
        ):
            with self.subTest(arguments=arguments):
                result, log = self.run_runner(*arguments)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("--confirm-guest-install", result.stderr)
                self.assertFalse(log.exists())
        result, log = self.run_runner(
            "package-install", "--confirm-guest-install", "--vm-acceptance-live-handoff"
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("SPLINTERM_TESTBED_VM_ACCEPTANCE=true", log.read_text())
        result, log = self.run_runner("package-install", "--confirm-guest-install")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("SPLINTERM_TESTBED_VM_ACCEPTANCE=false", log.read_text())
        runner = RUNNER.read_text(encoding="utf-8")
        install = runner.split("  package-install)", 1)[1].split("  package-launch)", 1)[0]
        self.assertIn(
            'vm_acceptance_arguments=(--allow-vm-acceptance-live-handoff)', install
        )
        self.assertIn(
            './tools/package/upgrade-local-package.sh --yes "${vm_acceptance_arguments[@]}"',
            install,
        )
        status = runner.split("  package-status)", 1)[1].split("  package-install)", 1)[0]
        self.assertIn("TEST-ONLY VM acceptance build", status)
        self.assertIn("splinterm-vm-acceptance-live-handoff:", status)
        build = runner.split("  package-build)", 1)[1].split("  package-status)", 1)[0]
        self.assertIn("build_arguments=(--vm-acceptance-live-handoff)", build)
        self.assertIn(
            'build-local-package.sh $(quote_command "${build_arguments[@]}")', build
        )

    def test_packaged_acceptance_uses_clean_head_and_private_guest_state(self) -> None:
        runner = RUNNER.read_text(encoding="utf-8")
        self.assertIn('git -C "$repo_root" bundle create "$bundle" HEAD', runner)
        self.assertIn("package-build requires a clean committed checkout", runner)
        self.assertIn("./tools/package/upgrade-local-package.sh --yes", runner)
        self.assertIn("/usr/bin/splinterm launch", runner)
        self.assertIn('XDG_STATE_HOME="$state"', runner)
        self.assertIn('XDG_CONFIG_HOME="$config"', runner)
        self.assertIn('${SPLINTERM_TESTBED_RUNTIME_LEAF}-package-install-check', runner)
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
            package_launch.index('mkdir -m 700 "$runtime"'),
        )

    def test_package_runtime_is_checkout_scoped_and_stop_checks_owner(self) -> None:
        runner = RUNNER.read_text(encoding="utf-8")
        install = runner.split("  package-install)", 1)[1].split("  package-launch)", 1)[0]
        launch = runner.split("  package-launch)", 1)[1].split("  package-stop)", 1)[0]
        stop = runner.split("  package-stop)", 1)[1].split("  exec)", 1)[0]
        self.assertIn('SPLINTERM_TESTBED_RUNTIME_LEAF=', runner)
        for action in (install, launch, stop):
            self.assertIn('${SPLINTERM_TESTBED_RUNTIME_LEAF}-package-', action)
            self.assertNotIn('"/run/user/$(id -u)/splinterm-package-', action)
        self.assertLess(
            install.index('[[ ! -e $runtime && ! -L $runtime ]]'),
            install.index('./tools/package/upgrade-local-package.sh --yes'),
        )
        self.assertLess(
            launch.index('[[ ! -e $runtime && ! -L $runtime ]]'),
            launch.index('mkdir -m 700 "$runtime"'),
        )
        self.assertIn('printf \'%s\\n\' "$package_root" >"$runtime/testbed-root"', launch)
        self.assertLess(stop.index('testbed-root'), stop.index('stop_matching client'))
        self.assertLess(stop.index('testbed-root'), stop.index('rm -rf "$runtime"'))
        result, log = self.run_runner(
            "package-stop",
            remote_root="/home/omarchy/Projects/splinterm-testbed-rc3_review",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(
            "SPLINTERM_TESTBED_RUNTIME_LEAF=splinterm-testbed-rc3_review",
            log.read_text(),
        )

    def test_package_launch_cleans_only_its_own_state_on_failures(self) -> None:
        launch = RUNNER.read_text(encoding="utf-8").split("  package-launch)", 1)[
            1
        ].split("  package-stop)", 1)[0]
        section = launch[
            launch.index("daemon_pid=\nclient_pid=\n") : launch.index(
                'python "$package_root/source/tools/testbed/guest-window.py" prepare'
            )
        ]
        self.assertLess(
            section.index("trap cleanup_failed_launch ERR"),
            section.index('mkdir -m 700 "$runtime"'),
        )
        self.assertLess(
            section.index("trap cleanup_failed_launch ERR"),
            section.index("instance=$(hyprctl instances -j"),
        )
        self.assertLess(
            section.index('guest-window.py" restore'),
            section.index('rm -rf -- "$runtime"'),
        )
        self.assertLess(
            section.index("return 1"),
            section.index('rm -rf -- "$runtime"'),
        )
        script = "\n".join(
            (
                "set -euo pipefail",
                'package_root="$TEST_PACKAGE_ROOT"',
                'runtime="$TEST_RUNTIME"',
                'state="$TEST_STATE"',
                'config="$TEST_CONFIG"',
                'window_state="$runtime/guest-window.json"',
                section,
                'if [[ $TEST_FAILURE == late || $TEST_FAILURE == restore ]]; then printf "prepared\\n" >"$window_state"; fi',
                "false",
            )
        )
        validation = ROOT / ".validation"
        validation.mkdir(exist_ok=True)
        for failure in ("early", "late", "partial", "restore"):
            with self.subTest(failure=failure):
                with tempfile.TemporaryDirectory(dir=validation) as temporary:
                    base = Path(temporary)
                    package_root = base / "package"
                    package_root.mkdir()
                    fake_bin = base / "bin"
                    fake_bin.mkdir()
                    hyprctl = fake_bin / "hyprctl"
                    hyprctl.write_text(
                        "#!/bin/sh\n"
                        'if [ "$TEST_FAILURE" = early ]; then exit 17; fi\n'
                        "printf '%s\\n' '[{\"wl_socket\":\"wayland-0\",\"instance\":\"test\"}]'\n"
                    )
                    hyprctl.chmod(0o755)
                    python = fake_bin / "python"
                    python.write_text(
                        "#!/bin/sh\n"
                        'test -f "$TEST_RUNTIME/guest-window.json" || exit 1\n'
                        'printf "%s\\n" "$@" >"$TEST_RESTORE_LOG"\n'
                        'if [ "$TEST_FAILURE" = restore ]; then exit 17; fi\n'
                    )
                    python.chmod(0o755)
                    runtime = base / "runtime"
                    state = package_root / "acceptance-state"
                    config = package_root / "acceptance-config"
                    restore_log = base / "restore.log"
                    if failure == "partial":
                        state.mkdir()
                        (state / "existing").write_text("preserve me")
                    environment = os.environ.copy()
                    environment.update(
                        {
                            "PATH": f"{fake_bin}:{environment['PATH']}",
                            "TEST_PACKAGE_ROOT": str(package_root),
                            "TEST_RUNTIME": str(runtime),
                            "TEST_STATE": str(state),
                            "TEST_CONFIG": str(config),
                            "TEST_RESTORE_LOG": str(restore_log),
                            "TEST_FAILURE": failure,
                        }
                    )
                    result = subprocess.run(
                        ["bash", "-c", script],
                        cwd=ROOT,
                        env=environment,
                        text=True,
                        capture_output=True,
                        check=False,
                    )
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                    if failure == "restore":
                        self.assertIn("private state retained", result.stderr)
                        self.assertTrue((runtime / "guest-window.json").exists())
                        self.assertEqual(
                            (runtime / "testbed-root").read_text().strip(),
                            str(package_root),
                        )
                        self.assertTrue(state.exists())
                        self.assertTrue(config.exists())
                    else:
                        self.assertFalse(runtime.exists(), result.stderr)
                        self.assertFalse(config.exists(), result.stderr)
                        if failure == "partial":
                            self.assertEqual((state / "existing").read_text(), "preserve me")
                        else:
                            self.assertFalse(state.exists(), result.stderr)
                    if failure in ("late", "restore"):
                        self.assertIn("restore", restore_log.read_text())
                    else:
                        self.assertFalse(restore_log.exists())

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
        self.assertEqual(runner.count('--client-pid "$client_pid"'), 3)
        self.assertEqual(runner.count('--client-token "$client_token"'), 3)
        self.assertEqual(runner.count('SPLINTERM_TEST_WINDOW_TOKEN="$client_token"'), 3)
        development_launch = runner.split("  launch)", 1)[1].split("  stop)", 1)[0]
        build = development_launch.index("case ${SPLINTERM_TEST_PROFILE:-release} in")
        prepare = development_launch.index("guest-window.py prepare")
        self.assertLess(build, development_launch.index("trap cleanup_failed_launch ERR"))
        self.assertLess(build, prepare)
        self.assertIn(
            "cargo build -q --release -p splinterd -p splinterm -p splinterm-pty",
            development_launch,
        )
        self.assertIn(
            "cargo build -q -p splinterd -p splinterm -p splinterm-pty",
            development_launch,
        )
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

    def test_explicit_root_override_wins_over_config_for_package_actions(self) -> None:
        unique = "/home/omarchy/Projects/splinterm-testbed-rc3_0ba40003"
        result, log = self.run_runner("package-stop", root_override=unique)
        self.assertEqual(result.returncode, 0, result.stderr)
        call = log.read_text()
        self.assertIn(f"SPLINTERM_TESTBED_PACKAGE_ROOT={unique}/.testbed-package", call)
        self.assertIn("SPLINTERM_TESTBED_RUNTIME_LEAF=splinterm-testbed-rc3_0ba40003", call)
        self.assertNotIn("splinterm-testbed-review", call)

    def test_desktop_exec_discovers_guest_wayland_environment(self) -> None:
        result, log = self.run_runner("desktop-exec", "wtype", "--", "two words")
        self.assertEqual(result.returncode, 0, result.stderr)
        call = log.read_text()
        self.assertIn("hyprctl instances -j", call)
        self.assertIn("if length == 1 then .[0]", call)
        self.assertIn("expected exactly one Hyprland instance", call)
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

    def action(self, name: str) -> str:
        runner = RUNNER.read_text(encoding="utf-8")
        return runner.split(f"  {name})", 1)[1].split("\n    ;;\n", 1)[0]

    def test_package_config_rejects_unsafe_paths_before_remote_commands(self) -> None:
        for relpath in (
            "../escape",
            "splinterm/../escape",
            "/etc/fonts/fonts.conf",
            "other/config.ini",
            "splinterm",
            "splinterm/.hidden",
            "fontconfig/a//b",
            "splinterm/" + "x" * 130,
            "splinterm/caf\u00e9.ini",
        ):
            with self.subTest(relpath=relpath):
                result, log = self.run_runner(
                    "package-config", relpath, input_text="x", locale="en_US.UTF-8"
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(log.exists())
        result, log = self.run_runner("package-config")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(log.exists())

    def test_package_config_sends_content_on_stdin_and_script_as_argument(self) -> None:
        result, log = self.run_runner(
            "package-config", "fontconfig/fonts.conf", input_text="<fontconfig/>\n"
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        call = log.read_text()
        self.assertIn("SPLINTERM_TESTBED_CONFIG_PATH=fontconfig/fonts.conf", call)
        self.assertIn(
            "SPLINTERM_TESTBED_PACKAGE_ROOT=/home/omarchy/Projects/"
            "splinterm-testbed-review/.testbed-package",
            call,
        )
        self.assertIn("bash -c", call)
        result, log = self.run_runner("package-config", "--clear")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("SPLINTERM_TESTBED_PACKAGE_ROOT=", log.read_text())

    def test_package_config_script_seeds_bounded_private_files(self) -> None:
        runner = RUNNER.read_text(encoding="utf-8")
        script = runner.split("remote_script=$(cat <<'REMOTE'\n", 1)[1].split(
            "\nREMOTE\n)", 1
        )[0]
        # The live config is written only for a session this checkout owns.
        self.assertLess(script.index('$(<"$owner") == "$package_root"'), script.rindex(
            'write_into "$config"'
        ))
        with tempfile.TemporaryDirectory() as temporary:
            package_root = Path(temporary) / "package"
            package_root.mkdir()
            environment = os.environ.copy()
            environment.update(
                {
                    "SPLINTERM_TESTBED_PACKAGE_ROOT": str(package_root),
                    "SPLINTERM_TESTBED_RUNTIME_LEAF": "splinterm-testbed-unit-absent",
                    "SPLINTERM_TESTBED_CONFIG_PATH": "splinterm/config.ini",
                }
            )
            result = subprocess.run(
                ["bash", "-c", script],
                env=environment,
                input="[main]\nstartup=dojo-picker\n",
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            seeded = package_root / "acceptance-seed/splinterm/config.ini"
            self.assertEqual(seeded.read_text(), "[main]\nstartup=dojo-picker\n")
            self.assertEqual(seeded.stat().st_mode & 0o777, 0o600)
            self.assertFalse((package_root / "acceptance-config").exists())
            self.assertEqual(list(package_root.glob(".config-content.*")), [])
            self.assertEqual(list(seeded.parent.glob(".package-config.*")), [])

            result = subprocess.run(
                ["bash", "-c", script],
                env=environment,
                input="x" * 65537,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("exceeds 64 KiB", result.stderr)
            self.assertEqual(seeded.read_text(), "[main]\nstartup=dojo-picker\n")

    def test_package_launch_copies_the_seed_before_any_process_starts(self) -> None:
        launch = self.action("package-launch")
        seed_copy = launch.index('cp -R -- "$seed/." "$config/"')
        self.assertLess(launch.index("created_config=true"), seed_copy)
        self.assertLess(seed_copy, launch.index("nohup /usr/bin/splinterd"))
        self.assertIn("[[ -d $seed && ! -L $seed ]]", launch)
        stop = self.action("package-stop")
        self.assertIn('"$SPLINTERM_TESTBED_PACKAGE_ROOT/font-fixtures"', stop)
        self.assertNotIn("acceptance-seed", stop)

    def test_package_launch_extra_requires_a_program_and_quotes_arguments(self) -> None:
        for arguments in ((), ("bogus",), ("--picker",)):
            with self.subTest(arguments=arguments):
                result, log = self.run_runner("package-launch-extra", *arguments)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(log.exists())
        result, log = self.run_runner("package-launch-extra", "launch", "--picker", "two words")
        self.assertEqual(result.returncode, 0, result.stderr)
        call = log.read_text()
        self.assertIn("SPLINTERM_TESTBED_EXTRA_PROGRAM=launch", call)
        self.assertIn("SPLINTERM_TESTBED_EXTRA_ARGUMENTS=", call)
        self.assertIn("--picker", call)
        self.assertIn("two", call)

    def test_package_launch_extra_is_guarded_by_the_owned_session(self) -> None:
        extra = self.action("package-launch-extra")
        owner_check = extra.index('$(<"$owner") == "$package_root"')
        allocate = extra.index('mkdir -m 700 "$runtime/extra-$index"')
        prepare = extra.index('guest-window.py" prepare')
        launch = extra.index('nohup "${program[@]}"')
        place = extra.index('guest-window.py" place')
        self.assertLess(owner_check, allocate)
        self.assertLess(allocate, prepare)
        self.assertLess(prepare, launch)
        self.assertLess(launch, place)
        trap = extra.index("trap cleanup_failed_extra ERR")
        self.assertLess(extra.index("instance=$(hyprctl instances -j"), trap)
        self.assertLess(trap, allocate)
        self.assertIn("[[ -n $extra ]] || return 0", extra)
        self.assertIn('--owner-state "$window_state"', extra)
        self.assertIn('SPLINTERM_SOCKET="$socket"', extra)
        self.assertIn('XDG_CONFIG_HOME="$config"', extra)
        self.assertIn("for index in 1 2 3 4; do", extra)
        self.assertIn("(/usr/bin/splinterm launch)", extra)
        self.assertIn("(/usr/bin/splinterm-xdg-terminal-exec)", extra)

    def test_package_close_extra_stops_only_its_own_token_and_restores(self) -> None:
        for identifier in ("0", "5", "x", "1 2"):
            with self.subTest(identifier=identifier):
                result, log = self.run_runner("package-close-extra", identifier)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(log.exists())
        result, log = self.run_runner("package-close-extra", "2")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("SPLINTERM_TESTBED_EXTRA_ID=2", log.read_text())
        close = self.action("package-close-extra")
        self.assertIn('grep -Fqx "SPLINTERM_TEST_WINDOW_TOKEN=$token"', close)
        token_check = close.index("if [[ $token =~ ^[0-9a-f]{32}$ ]]; then")
        self.assertLess(token_check, close.index('kill "${process##*/}"'))
        self.assertIn("[[ -f $extra/client-token && ! -L $extra/client-token ]]", close)
        self.assertIn('grep -Fqx "SPLINTERM_SOCKET=$socket"', close)
        self.assertLess(close.index('kill "${process##*/}"'), close.index('guest-window.py" restore'))
        self.assertLess(close.index('guest-window.py" restore'), close.index('rm -rf -- "$extra"'))

    def test_font_fixture_action_builds_in_the_package_root(self) -> None:
        for arguments in ((), ("other",), ("invalid-candidate", "--force")):
            with self.subTest(arguments=arguments):
                result, log = self.run_runner("font-fixture", *arguments)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(log.exists())
        result, log = self.run_runner("font-fixture", "invalid-candidate")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("SPLINTERM_TESTBED_INSTALL_FIXTURE=false", log.read_text())
        result, log = self.run_runner("font-fixture", "invalid-candidate", "--install")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("SPLINTERM_TESTBED_INSTALL_FIXTURE=true", log.read_text())
        fixture = self.action("font-fixture")
        self.assertIn("tools/testbed/font-fixture.py\" invalid-candidate", fixture)
        self.assertLess(
            fixture.index('$(<"$owner") == "$package_root"'),
            fixture.index('install -m 600 -- "$output/fonts.conf" "$staged"'),
        )
        self.assertIn('"$destination.before-fixture"', fixture)
        self.assertIn('mv -f -- "$staged" "$destination"', fixture)
        self.assertIn('generation=$(mktemp -d "$fixtures/invalid-candidate.XXXXXX")', fixture)
        self.assertNotIn('rm -rf -- "$output"', fixture)
        self.assertLess(
            fixture.index("font-fixture.py\" invalid-candidate"), fixture.index("built=true")
        )

    def test_font_fixture_rebuilds_beside_the_live_generation(self) -> None:
        fixture = self.action("font-fixture")
        script = fixture.split("<<'REMOTE'\n", 1)[1].split("\nREMOTE", 1)[0]
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            package_root = base / "package"
            (package_root / "source/.git").mkdir(parents=True)
            fake_bin = base / "bin"
            fake_bin.mkdir()
            # Stands in for font-fixture.py: a fixture names its own directory.
            python = fake_bin / "python"
            python.write_text(
                "#!/bin/sh\n"
                '[ -z "$FAKE_FIXTURE_FAIL" ] || exit 1\n'
                'mkdir -p "$4/fonts"\n'
                "printf '<dir>%s/fonts</dir>\\n' \"$4\" >\"$4/fonts.conf\"\n"
            )
            python.chmod(0o755)
            environment = os.environ.copy()
            environment.update(
                {
                    "PATH": f"{fake_bin}:{environment['PATH']}",
                    "SPLINTERM_TESTBED_PACKAGE_ROOT": str(package_root),
                    "SPLINTERM_TESTBED_RUNTIME_LEAF": "splinterm-testbed-unit-absent",
                    "SPLINTERM_TESTBED_INSTALL_FIXTURE": "false",
                }
            )

            def build(fail: bool = False) -> subprocess.CompletedProcess[str]:
                env = dict(environment)
                if fail:
                    env["FAKE_FIXTURE_FAIL"] = "1"
                return subprocess.run(
                    ["bash", "-c", script], env=env, text=True, capture_output=True, check=False
                )

            fixtures = package_root / "font-fixtures"
            result = build()
            self.assertEqual(result.returncode, 0, result.stderr)
            (live,) = fixtures.glob("invalid-candidate.*")
            # Point the session's fonts.conf at that generation, as --install does.
            destination = package_root / "acceptance-config/fontconfig/fonts.conf"
            destination.parent.mkdir(parents=True)
            destination.write_text((live / "fixture/fonts.conf").read_text())

            # A failed rebuild leaves the live generation and discards its own.
            result = build(fail=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(list(fixtures.glob("invalid-candidate.*")), [live])
            self.assertTrue((live / "fixture/fonts").is_dir())

            # A rebuild never deletes the generation the live config names.
            result = build()
            self.assertEqual(result.returncode, 0, result.stderr)
            generations = set(fixtures.glob("invalid-candidate.*"))
            self.assertEqual(len(generations), 2)
            self.assertIn(live, generations)
            (fresh,) = generations - {live}
            self.assertIn(str(fresh / "fixture"), result.stdout)

            # Once the live config moves on, unreferenced generations are pruned.
            destination.write_text((fresh / "fixture/fonts.conf").read_text())
            result = build()
            self.assertEqual(result.returncode, 0, result.stderr)
            generations = set(fixtures.glob("invalid-candidate.*"))
            self.assertEqual(len(generations), 2)
            self.assertIn(fresh, generations)
            self.assertNotIn(live, generations)


if __name__ == "__main__":
    unittest.main()
