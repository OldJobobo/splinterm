#!/usr/bin/env python3
"""Non-graphical contract tests for the Splinterm-owned Omarchy launcher."""

from __future__ import annotations

import os
import shutil
from pathlib import Path
import subprocess
import tempfile
import time
import unittest


ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "dist/omarchy/omarchy-launch-screensaver"
SESSION = object()
# Real tools the launcher uses unstubbed, found where this host keeps them.
REAL_TOOLS = ("bash", "readlink", "tr", "grep", "sleep")
DEFAULT_MONITORS = "DP-1 special:scratchpad\nDP-2 \n"


def real_tool_dirs() -> list[str]:
    directories: list[str] = []
    for tool in REAL_TOOLS:
        found = shutil.which(tool)
        if found is not None and str(Path(found).parent) not in directories:
            directories.append(str(Path(found).parent))
    return directories


def write_launcher(path: Path, label: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        f"#!/bin/sh\nprintf '{label}:%s\\n' \"$*\" >>\"$CALL_LOG\"\n",
        encoding="utf-8",
    )
    path.chmod(0o755)


class OmarchyScreensaverIntegrationTest(unittest.TestCase):
    def harness(
        self,
        directory: Path,
        terminal: str,
        events: str | None = None,
        process_token: str = "0123456789abcdef0123456789abcdef",
        omarchy_path: object = SESSION,
        monitors: str = DEFAULT_MONITORS,
    ) -> tuple[Path, dict[str, str], Path]:
        bin_dir = directory / "bin"
        bin_dir.mkdir()
        log = directory / "calls.log"
        log.touch()
        profile = directory / "screensaver.ini"
        profile.write_text("[main]\nfont-point-size=18\n", encoding="utf-8")
        random_uuid = directory / "random-uuid"
        random_uuid.write_text(
            "01234567-89ab-cdef-0123-456789abcdef\n", encoding="utf-8"
        )
        proc_root = directory / "proc"
        process = proc_root / "111"
        process.mkdir(parents=True)
        (process / "environ").write_bytes(
            f"SPLINTERM_SCREENSAVER_INVOCATION={process_token}\0".encode()
        )
        # The session's checkout ($OMARCHY_PATH) and the packaged fallback.
        write_launcher(
            directory / "session-omarchy/bin/omarchy-launch-screensaver", "session"
        )
        packaged = directory / "packaged-omarchy"
        write_launcher(packaged / "bin/omarchy-launch-screensaver", "packaged")
        launcher = directory / "launcher"
        source = (
            SOURCE.read_text(encoding="utf-8")
            .replace("SECONDS + 5", "SECONDS + 1")
            .replace("readonly PROC_ROOT=/proc", f"readonly PROC_ROOT={proc_root}")
            .replace(
                "readonly RANDOM_UUID=/proc/sys/kernel/random/uuid",
                f"readonly RANDOM_UUID={random_uuid}",
            )
            .replace(
                "readonly DEFAULT_OMARCHY_PATH=/usr/share/omarchy",
                f"readonly DEFAULT_OMARCHY_PATH={packaged}",
            )
            .replace(
                "readonly SPLINTERM_PROFILE=/usr/share/splinterm/omarchy/screensaver.ini",
                f"readonly SPLINTERM_PROFILE={profile}",
            )
        )
        self.assertNotIn("/usr/share/omarchy", source)
        launcher.write_text(source, encoding="utf-8")
        launcher.chmod(0o755)

        commands = {
            "xdg-terminal-exec": f"#!/bin/sh\nif [ \"$1\" = --print-id ]; then printf '%s\\n' '{terminal}'; else printf 'xdg:%s\\n' \"$*\" >>\"$CALL_LOG\"; fi\n",
            "omarchy-hyprland-monitor-focused": "#!/bin/sh\nprintf 'DP-3\\n'\n",
            "omarchy-toggle-enabled": "#!/bin/sh\nexit 1\n",
            "pgrep": "#!/bin/sh\nexit 1\n",
            "hyprctl": (
                "#!/bin/sh\n"
                "printf 'hyprctl:%s\\n' \"$*\" >>\"$CALL_LOG\"\n"
                "if [ \"$1\" = monitors ]; then printf '[]\\n'; fi\n"
                "case $2 in 'hl.dsp.exec_cmd('*) [ -z \"${HYPR_LUA_EXEC_FAILS:-}\" ] || exit 1 ;; esac\n"
                "exit 0\n"
            ),
            # -e --arg: one launched window still exists until it is closed.
            # -e alone: whether any screensaver window is still mapped.
            "jq": (
                "#!/bin/sh\n"
                "case $1 in\n"
                "  -er) printf '111\\n' ;;\n"
                "  -e)\n"
                "    if [ \"$2\" = --arg ]; then\n"
                "      address=$4; grep -Fq \"window = \\\"address:$address\\\"\" \"$CALL_LOG\" && exit 1; exit 0\n"
                "    fi\n"
                "    [ \"$SCREENSAVER\" = org.omarchy.screensaver ] || exit 2\n"
                "    exit \"${SCREENSAVERS_MAPPED:-1}\" ;;\n"
                "  -r) printf '%b' \"$MONITORS\" ;;\n"
                "esac\n"
            ),
            "socat": "#!/bin/sh\nprintf '%s' \"$SCREENSAVER_EVENTS\"\n",
        }
        for name, body in commands.items():
            command = bin_dir / name
            command.write_text(body, encoding="utf-8")
            command.chmod(0o755)
        environment = os.environ.copy()
        environment.pop("OMARCHY_PATH", None)
        environment.update(
            {
                "PATH": ":".join([str(bin_dir), *real_tool_dirs(), "/usr/bin"]),
                "CALL_LOG": str(log),
                "XDG_RUNTIME_DIR": str(directory / "runtime"),
                "HYPRLAND_INSTANCE_SIGNATURE": "test",
                "MONITORS": monitors,
                "SCREENSAVER_EVENTS": events
                or "openwindow>>0xa,org.omarchy.screensaver,title\nopenwindow>>0xb,org.omarchy.screensaver,title\n",
            }
        )
        if omarchy_path is SESSION:
            environment["OMARCHY_PATH"] = str(directory / "session-omarchy")
        elif omarchy_path is not None:
            environment["OMARCHY_PATH"] = str(omarchy_path)
        return launcher, environment, log

    def focus_calls(self, log: Path, expected: int) -> list[str]:
        """Wait for the detached focus return, which outlives the launcher."""
        deadline = time.monotonic() + 3
        while True:
            calls = [
                line
                for line in log.read_text(encoding="utf-8").splitlines()
                if "hl.dsp.focus" in line
            ]
            if len(calls) >= expected or time.monotonic() > deadline:
                return calls
            time.sleep(0.02)

    def delegate(self, omarchy_path: object, prepare=None) -> str:
        with tempfile.TemporaryDirectory(prefix="splinterm-omarchy-helper-") as value:
            root = Path(value)
            launcher, environment, log = self.harness(
                root, "foot.desktop", omarchy_path=omarchy_path
            )
            if prepare is not None:
                prepare(root, launcher)
            result = subprocess.run(
                [launcher, "force"],
                env=environment,
                text=True,
                capture_output=True,
                check=False,
                timeout=5,
            )
            return f"{result.returncode}\n{log.read_text(encoding='utf-8')}{result.stderr}"

    def test_other_terminal_delegates_to_the_session_omarchy_path(self) -> None:
        self.assertEqual(self.delegate(SESSION), "0\nsession:force\n")

    def test_other_terminal_falls_back_to_packaged_omarchy(self) -> None:
        self.assertEqual(self.delegate(None), "0\npackaged:force\n")
        self.assertEqual(self.delegate(""), "0\npackaged:force\n")
        self.assertEqual(self.delegate("relative/omarchy"), "0\npackaged:force\n")
        self.assertEqual(self.delegate("/nonexistent/omarchy"), "0\npackaged:force\n")

    def test_delegate_is_never_the_adapter_itself(self) -> None:
        def link_session_to_adapter(root: Path, launcher: Path) -> None:
            session = root / "session-omarchy/bin/omarchy-launch-screensaver"
            session.unlink()
            session.symlink_to(launcher)

        self.assertEqual(
            self.delegate(SESSION, link_session_to_adapter), "0\npackaged:force\n"
        )

    def test_missing_omarchy_launcher_fails_clearly(self) -> None:
        def remove_launchers(root: Path, _launcher: Path) -> None:
            for name in ("session-omarchy", "packaged-omarchy"):
                (root / name / "bin/omarchy-launch-screensaver").unlink()

        result = self.delegate(SESSION, remove_launchers)
        self.assertTrue(result.startswith("1\n"), result)
        self.assertIn("screensaver launcher is unavailable under $OMARCHY_PATH", result)

    def test_splinterm_launches_each_monitor_on_its_special_workspace(self) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-omarchy-helper-") as value:
            root = Path(value)
            launcher, environment, log = self.harness(
                root, "com.oldjobobo.splinterm.desktop"
            )
            subprocess.run([launcher, "force"], env=environment, check=True, timeout=5)
            focus_calls = self.focus_calls(log, 4)
            calls = log.read_text(encoding="utf-8").splitlines()
            exec_calls = [line for line in calls if "hl.dsp.exec_cmd" in line]
            self.assertEqual(len(exec_calls), 2)
            # A special workspace already shown (the scratchpad) is shared;
            # otherwise each monitor gets its own.
            self.assertIn(
                "hl.dsp.exec_cmd([[[workspace special:scratchpad] env ", exec_calls[0]
            )
            self.assertIn(
                "hl.dsp.exec_cmd([[[workspace special:screensaver-DP-2] env ",
                exec_calls[1],
            )
            for line in exec_calls:
                self.assertIn(f"SPLINTERM_CONFIG={root / 'screensaver.ini'}", line)
                self.assertIn(
                    "SPLINTERM_SCREENSAVER_INVOCATION=0123456789abcdef0123456789abcdef",
                    line,
                )
                self.assertIn("xdg-terminal-exec --app-id=org.omarchy.screensaver", line)
                self.assertIn("-- omarchy-screensaver", line)
                self.assertTrue(line.endswith("]])"), line)
            # Each monitor, the original monitor, then the original monitor
            # again once every screensaver window has closed.
            self.assertEqual(len(focus_calls), 4, focus_calls)
            self.assertIn("DP-1", focus_calls[0])
            self.assertIn("DP-2", focus_calls[1])
            self.assertIn("DP-3", focus_calls[2])
            self.assertIn("DP-3", focus_calls[3])
            self.assertFalse(
                any(line.startswith(("session:", "packaged:")) for line in calls)
            )

    def test_focus_returns_only_after_every_screensaver_closes(self) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-omarchy-helper-") as value:
            launcher, environment, log = self.harness(
                Path(value), "com.oldjobobo.splinterm.desktop"
            )
            environment["SCREENSAVERS_MAPPED"] = "0"
            subprocess.run([launcher, "force"], env=environment, check=True, timeout=5)
            time.sleep(0.3)
            focus_calls = self.focus_calls(log, 3)
            self.assertEqual(len(focus_calls), 3, focus_calls)

    def test_exec_rule_survives_the_legacy_dispatcher_fallback(self) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-omarchy-helper-") as value:
            root = Path(value)
            launcher, environment, log = self.harness(
                root, "com.oldjobobo.splinterm.desktop", monitors="DP-2\n"
            )
            environment["HYPR_LUA_EXEC_FAILS"] = "1"
            environment["SCREENSAVER_EVENTS"] = (
                "openwindow>>0xa,org.omarchy.screensaver,title\n"
            )
            subprocess.run([launcher, "force"], env=environment, check=True, timeout=5)
            calls = log.read_text(encoding="utf-8").splitlines()
            fallback = [line for line in calls if "dispatch exec --" in line]
            self.assertEqual(len(fallback), 1, calls)
            self.assertTrue(
                fallback[0].startswith(
                    "hyprctl:dispatch exec -- [workspace special:screensaver-DP-2] env "
                    f"SPLINTERM_CONFIG={root / 'screensaver.ini'} "
                ),
                fallback[0],
            )
            self.assertIn(
                "--app-id=org.omarchy.screensaver -- omarchy-screensaver", fallback[0]
            )

    def test_second_monitor_timeout_closes_only_the_window_launched_first(self) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-omarchy-helper-") as value:
            launcher, environment, log = self.harness(
                Path(value),
                "com.oldjobobo.splinterm.desktop",
                "openwindow>>0xa,org.omarchy.screensaver,title\n",
            )
            result = subprocess.run(
                [launcher, "force"],
                env=environment,
                text=True,
                capture_output=True,
                check=False,
                timeout=5,
            )
            self.assertEqual(result.returncode, 1, result)
            self.assertIn("did not map on DP-2", result.stderr)
            calls = log.read_text(encoding="utf-8").splitlines()
            close_calls = [line for line in calls if "hl.dsp.window.close" in line]
            self.assertEqual(len(close_calls), 1)
            self.assertIn('address:0xa', close_calls[0])
            self.assertFalse(any('address:0xb' in line for line in close_calls))
            self.assertIn("DP-3", calls[-1])
            # A failed launch hands focus back once, without the watcher.
            time.sleep(0.3)
            self.assertEqual(len(self.focus_calls(log, 3)), 3)

    def test_foreign_matching_app_id_event_is_not_attributed_or_closed(self) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-omarchy-helper-") as value:
            launcher, environment, log = self.harness(
                Path(value),
                "com.oldjobobo.splinterm.desktop",
                "openwindow>>0xc,org.omarchy.screensaver,title\n",
                process_token="foreign-invocation",
            )
            result = subprocess.run(
                [launcher, "force"],
                env=environment,
                text=True,
                capture_output=True,
                check=False,
                timeout=5,
            )
            self.assertEqual(result.returncode, 1, result)
            calls = log.read_text(encoding="utf-8").splitlines()
            self.assertFalse(any("hl.dsp.window.close" in line for line in calls))
            self.assertIn("DP-3", calls[-1])


if __name__ == "__main__":
    unittest.main()
