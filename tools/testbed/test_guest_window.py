#!/usr/bin/env python3
"""Unit tests for the guarded guest-window lifecycle."""

from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock


MODULE_PATH = Path(__file__).with_name("guest-window.py")
SPEC = importlib.util.spec_from_file_location("guest_window", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
guest_window = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(guest_window)


class GuestWindowTests(unittest.TestCase):
    def state_path(self) -> Path:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        return Path(temporary.name) / "guest-window.json"

    def test_prepare_records_exact_environment_and_requires_empty_workspace(self) -> None:
        path = self.state_path()
        initial_clients = [
            {"address": "0xabc", "workspace": {"id": 2}, "initialClass": "other"}
        ]

        active_workspaces = iter(
            (
                {"id": 2, "monitor": "Virtual-1"},
                {"id": 8, "monitor": "Virtual-1"},
            )
        )

        def hypr_json(name: str, *_: str) -> object:
            if name == "activeworkspace":
                return next(active_workspaces)
            return {
                "activewindow": {"address": "0xabc"},
                "cursorpos": {"x": 12, "y": 34},
                "workspaces": [{"id": 8, "monitor": "Virtual-1"}],
            }[name]

        with (
            mock.patch.object(guest_window, "clients", return_value=initial_clients),
            mock.patch.object(guest_window, "hypr_json", side_effect=hypr_json),
            mock.patch.object(
                guest_window,
                "monitor_record",
                return_value={
                    "id": 0,
                    "name": "Virtual-1",
                    "scale": 1.0,
                    "transform": 0,
                },
            ),
            mock.patch.object(guest_window, "command") as command,
        ):
            guest_window.prepare(path)

        calls = [call.args for call in command.call_args_list]
        self.assertIn('monitor = "Virtual-1"', calls[0][2])
        self.assertIn('workspace = "8"', calls[1][2])

        state = json.loads(path.read_text(encoding="utf-8"))
        self.assertEqual(state["active_window"], "0xabc")
        self.assertEqual(state["active_workspace"], 2)
        self.assertEqual(state["before_addresses"], ["0xabc"])
        self.assertEqual(state["cursor"], {"x": 12, "y": 34})
        self.assertIsNone(state["target_address"])

    def test_prepare_rejects_an_occupied_target_workspace(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "workspace 8 is not empty"):
            guest_window.require_empty_target_workspace(
                [{"address": "0xabc", "workspace": {"id": 8}}]
            )

    def test_place_targets_one_fresh_window_by_exact_address(self) -> None:
        path = self.state_path()
        guest_window.write_state(
            path,
            {
                "before_addresses": ["0xabc"],
                "target_address": None,
            },
        )
        candidate = {
            "address": "0xdef",
            "initialClass": guest_window.APP_ID,
            "pid": 1234,
            "monitor": 0,
            "workspace": {"id": 8},
            "at": [10, 20],
            "size": [800, 600],
        }
        with (
            mock.patch.object(guest_window, "fresh_candidates", return_value=[candidate]),
            mock.patch.object(guest_window, "clients", return_value=[candidate]),
            mock.patch.object(
                guest_window,
                "monitor_record",
                return_value={
                    "id": 0,
                    "name": "Virtual-1",
                    "scale": 1.0,
                    "transform": 0,
                },
            ),
            mock.patch.object(guest_window, "command") as command,
        ):
            guest_window.place(path)

        calls = [call.args for call in command.call_args_list]
        self.assertEqual(len(calls), 2)
        self.assertTrue(all("address:0xdef" in call[2] for call in calls))
        self.assertIn('monitor = "Virtual-1"', calls[0][2])
        self.assertIn('workspace = "8"', calls[1][2])
        state = json.loads(path.read_text(encoding="utf-8"))
        self.assertEqual(state["target_address"], "0xdef")
        self.assertEqual(
            state["target_initial"],
            {
                "address": "0xdef",
                "pid": 1234,
                "workspace": 8,
                "monitor": 0,
                "at": [10, 20],
                "size": [800, 600],
            },
        )
        self.assertEqual(state["target_final"], state["target_initial"])

    def test_place_rejects_multiple_fresh_splinterm_windows(self) -> None:
        path = self.state_path()
        guest_window.write_state(path, {"before_addresses": [], "target_address": None})
        with (
            mock.patch.object(
                guest_window,
                "fresh_candidates",
                return_value=[{"address": "0x1"}, {"address": "0x2"}],
            ),
            self.assertRaisesRegex(RuntimeError, "expected one fresh"),
        ):
            guest_window.place(path)

    def test_restore_verifies_cleanup_and_restores_workspace_focus_and_cursor(self) -> None:
        path = self.state_path()
        monitor = {
            "id": 0,
            "name": "Virtual-1",
            "scale": 1.0,
            "transform": 0,
        }
        guest_window.write_state(
            path,
            {
                "active_window": "0xabc",
                "active_workspace": 2,
                "before_addresses": ["0xabc"],
                "cursor": {"x": 12, "y": 34},
                "monitor": monitor,
                "target_address": "0xdef",
            },
        )
        remaining = [{"address": "0xabc", "workspace": {"id": 2}}]
        with (
            mock.patch.object(guest_window, "clients", return_value=remaining),
            mock.patch.object(guest_window, "hypr_json", return_value=[]),
            mock.patch.object(guest_window, "monitor_record", return_value=monitor),
            mock.patch.object(Path, "is_socket", return_value=True),
            mock.patch.object(guest_window, "command") as command,
            mock.patch.dict(guest_window.os.environ, {"YDOTOOL_SOCKET": "/run/socket"}),
        ):
            guest_window.restore(path)

        self.assertFalse(path.exists())
        calls = [call.args for call in command.call_args_list]
        self.assertIn('workspace = "2"', calls[0][2])
        self.assertIn("address:0xabc", calls[1][2])
        self.assertEqual(
            calls[2],
            ("ydotool", "mousemove", "--absolute", "-x", "12", "-y", "34"),
        )


if __name__ == "__main__":
    unittest.main()
