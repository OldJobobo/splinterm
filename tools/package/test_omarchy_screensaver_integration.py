#!/usr/bin/env python3
"""Non-graphical contract tests for the Splinterm-owned Omarchy launcher."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "dist/omarchy/omarchy-launch-screensaver"


class OmarchyScreensaverIntegrationTest(unittest.TestCase):
    def harness(
        self,
        directory: Path,
        terminal: str,
        events: str | None = None,
        process_token: str = "0123456789abcdef0123456789abcdef",
    ) -> tuple[Path, dict[str, str], Path]:
        bin_dir = directory / "bin"
        bin_dir.mkdir()
        log = directory / "calls.log"
        profile = directory / "screensaver.ini"
        profile.write_text("[main]\nfont-point-size=18\n", encoding="utf-8")
        invocation_token = "0123456789abcdef0123456789abcdef"
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
        canonical = directory / "canonical-launcher"
        canonical.write_text(
            "#!/bin/sh\nprintf 'canonical:%s\\n' \"$*\" >>\"$CALL_LOG\"\n",
            encoding="utf-8",
        )
        canonical.chmod(0o755)
        launcher = directory / "launcher"
        source = (
            SOURCE.read_text(encoding="utf-8")
            .replace("SECONDS + 5", "SECONDS + 1")
            .replace("readonly PROC_ROOT=/proc", f"readonly PROC_ROOT={proc_root}")
            .replace(
                "readonly RANDOM_UUID=/proc/sys/kernel/random/uuid",
                f"readonly RANDOM_UUID={random_uuid}",
            )
        )
        source = source.replace(
            "readonly OMARCHY_LAUNCHER=/usr/share/omarchy/bin/omarchy-launch-screensaver",
            f"readonly OMARCHY_LAUNCHER={canonical}",
        ).replace(
            "readonly SPLINTERM_PROFILE=/usr/share/splinterm/omarchy/screensaver.ini",
            f"readonly SPLINTERM_PROFILE={profile}",
        )
        launcher.write_text(source, encoding="utf-8")
        launcher.chmod(0o755)

        commands = {
            "xdg-terminal-exec": f"#!/bin/sh\nif [ \"$1\" = --print-id ]; then printf '%s\\n' '{terminal}'; else printf 'xdg:%s\\n' \"$*\" >>\"$CALL_LOG\"; fi\n",
            "omarchy-hyprland-monitor-focused": "#!/bin/sh\nprintf 'DP-3\\n'\n",
            "omarchy-toggle-enabled": "#!/bin/sh\nexit 1\n",
            "pgrep": "#!/bin/sh\nexit 1\n",
            "hyprctl": "#!/bin/sh\nprintf 'hyprctl:%s\\n' \"$*\" >>\"$CALL_LOG\"\nif [ \"$1\" = monitors ]; then printf '[]\\n'; fi\n",
            "jq": "#!/bin/sh\ncase $1 in\n  -er) printf '111\\n' ;;\n  -e) address=$4; grep -Fq \"window = \\\"address:$address\\\"\" \"$CALL_LOG\" && exit 1; exit 0 ;;\n  -r) printf 'DP-1\\nDP-2\\n' ;;\nesac\n",
            "socat": "#!/bin/sh\nprintf '%s' \"$SCREENSAVER_EVENTS\"\n",
        }
        for name, body in commands.items():
            command = bin_dir / name
            command.write_text(body, encoding="utf-8")
            command.chmod(0o755)
        environment = os.environ.copy()
        environment.update(
            {
                "PATH": f"{bin_dir}:/usr/bin",
                "CALL_LOG": str(log),
                "XDG_RUNTIME_DIR": str(directory / "runtime"),
                "HYPRLAND_INSTANCE_SIGNATURE": "test",
                "SCREENSAVER_EVENTS": events
                or "openwindow>>0xa,org.omarchy.screensaver,title\nopenwindow>>0xb,org.omarchy.screensaver,title\n",
            }
        )
        return launcher, environment, log

    def test_other_terminal_delegates_exact_arguments_without_hyprland_work(self) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-omarchy-helper-") as value:
            launcher, environment, log = self.harness(Path(value), "foot.desktop")
            subprocess.run(
                [launcher, "force"], env=environment, check=True, timeout=5
            )
            self.assertEqual(log.read_text(encoding="utf-8"), "canonical:force\n")

    def test_splinterm_launches_each_monitor_and_restores_original_focus(self) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-omarchy-helper-") as value:
            root = Path(value)
            launcher, environment, log = self.harness(
                root, "com.oldjobobo.splinterm.desktop"
            )
            subprocess.run([launcher, "force"], env=environment, check=True, timeout=5)
            calls = log.read_text(encoding="utf-8").splitlines()
            exec_calls = [line for line in calls if "hl.dsp.exec_cmd" in line]
            self.assertEqual(len(exec_calls), 2)
            for line in exec_calls:
                self.assertIn(f"SPLINTERM_CONFIG={root / 'screensaver.ini'}", line)
                self.assertIn(
                    "SPLINTERM_SCREENSAVER_INVOCATION=0123456789abcdef0123456789abcdef",
                    line,
                )
                self.assertIn("--app-id=org.omarchy.screensaver", line)
                self.assertIn("-- omarchy-screensaver", line)
            focus_calls = [line for line in calls if "hl.dsp.focus" in line]
            self.assertEqual(len(focus_calls), 3)
            self.assertIn("DP-1", focus_calls[0])
            self.assertIn("DP-2", focus_calls[1])
            self.assertIn("DP-3", focus_calls[2])
            self.assertFalse(any(line.startswith("canonical:") for line in calls))

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
