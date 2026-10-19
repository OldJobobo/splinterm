"""Reduced resource limits and read-only Nix runtime support; no remote jobs."""

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from test_remote import command_result, host, remote, task


class HostRuntimeTests(unittest.TestCase):
    def test_legacy_defaults_are_unchanged(self):
        config = host(Path("/tmp/test"))
        self.assertEqual(remote.service_path(config), "/usr/local/bin:/usr/bin:/bin")
        self.assertEqual(
            remote.service_limits(config), {"CPUQuota": "200%", "MemoryMax": "4G"}
        )

    def test_reduced_limits_and_nix_paths_are_valid(self):
        config = host(Path("/tmp/test"))
        config.update(
            python="/etc/profiles/per-user/splinterm-worker/bin/python3",
            service_path="/etc/profiles/per-user/splinterm-worker/bin:/run/current-system/sw/bin",
            cpu_percent=50,
            memory_mib=1024,
        )
        self.assertEqual(remote.validate_host(config), config)
        self.assertEqual(
            remote.service_limits(config), {"CPUQuota": "50%", "MemoryMax": "1024M"}
        )

    def test_invalid_runtime_settings_fail_closed(self):
        for key, values in {
            "cpu_percent": [True, "100", 0, 24, 201],
            "memory_mib": [True, "1024", 0, 255, 4097],
            "python": ["python3", "/tmp/../python", "/python\n"],
            "service_path": [
                "",
                "bin",
                "/bin:",
                ":/bin",
                "/bin::/usr/bin",
                "/tmp/../bin",
                "/bin;%HOME%",
                "/bin\n/usr/bin",
                ":".join(["/bin"] * 9),
            ],
        }.items():
            for value in values:
                with self.subTest(key=key, value=value):
                    config = host(Path("/tmp/test"))
                    config[key] = value
                    with self.assertRaises(ValueError):
                        remote.validate_host(config)

    def test_runtime_path_is_used_by_passive_doctor(self):
        config = host(Path("/tmp/test"))
        config["service_path"] = "/run/current-system/sw/bin"
        calls = []

        def fake_run(argv, **kwargs):
            calls.append((argv, kwargs))
            if "--no-refresh" in argv:
                text = json.dumps({"status": "ready"})
            elif "--value" in argv:
                text = "yes\n"
            elif argv[0] == config["pi"]:
                text = "1.3.0\n"
            else:
                text = ""
            return command_result(stdout=text)

        with patch.object(remote, "run", side_effect=fake_run):
            result = remote.doctor(config)
        self.assertEqual(result["state"], "ready")
        self.assertEqual(result["service_path"], config["service_path"])
        self.assertEqual(result["limits"]["MemoryMax"], "4G")
        self.assertTrue(all(k["path"] == config["service_path"] for _, k in calls))
        self.assertTrue(any(a == ["systemd-run", "--version"] for a, _ in calls))
        self.assertTrue(any("--no-refresh" in a for a, _ in calls))

    def test_service_uses_lowered_limits_and_configured_path(self):
        with tempfile.TemporaryDirectory() as directory:
            config = host(Path(directory))
            config.update(
                service_path="/run/current-system/sw/bin",
                cpu_percent=100,
                memory_mib=1536,
            )
            with (
                patch.object(remote, "doctor", return_value={"state": "ready"}),
                patch.object(remote, "run", return_value=command_result()) as command,
            ):
                self.assertEqual(remote.submit(config, task())["state"], "queued")
            argv = command.call_args.args[0]
            self.assertIn("--property=CPUQuota=100%", argv)
            self.assertIn("--property=MemoryMax=1536M", argv)
            self.assertIn("--setenv=PATH=/run/current-system/sw/bin", argv)
            self.assertIn("--property=KillMode=control-group", argv)
            self.assertIn("--property=TasksMax=64", argv)

    def test_transport_uses_explicit_remote_python(self):
        config = host(Path("/tmp/test"))
        config["python"] = "/etc/profiles/per-user/splinterm-worker/bin/python3"
        with patch.object(
            remote.subprocess,
            "run",
            return_value=command_result(stdout='{"state":"not_ready"}'),
        ) as command:
            remote.transport(config, {"action": "doctor"})
        argv = command.call_args.args[0]
        self.assertTrue(argv[-1].startswith(config["python"] + " -c "))
        self.assertIn("StrictHostKeyChecking=yes", argv)

    def test_run_overrides_only_requested_path(self):
        with patch.dict(
            os.environ, {"PATH": "/not-the-service-path", "TEST_MARKER": "kept"}
        ):
            result = remote.run(
                [
                    sys.executable,
                    "-c",
                    "import os; print(os.environ['PATH']); print(os.environ['TEST_MARKER'])",
                ],
                path="/run/current-system/sw/bin",
            )
        self.assertEqual(
            result.stdout.splitlines(), ["/run/current-system/sw/bin", "kept"]
        )


class RequestPathTests(unittest.TestCase):
    """Real disposable tool lookup, never a production systemd manager."""

    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.bin = self.root / "tools"
        self.bin.mkdir()
        self.log = self.root / "calls.jsonl"
        self.config = host(self.root)
        self.config["service_path"] = str(self.bin)
        program = "\n".join(
            [
                "#!" + sys.executable,
                "import json, os, sys",
                f"with open({str(self.log)!r}, 'a') as stream:",
                "    stream.write(json.dumps({'tool': os.path.basename(sys.argv[0]), 'args': sys.argv[1:], 'path': os.environ.get('PATH')}) + '\\n')",
                "if 'show' in sys.argv:",
                "    print('LoadState=loaded\\nActiveState=active\\nSubState=running\\nJob=0')",
                "",
            ]
        )
        for name in ("systemd-run", "systemctl"):
            path = self.bin / name
            path.write_text(program)
            path.chmod(0o755)

    def request(self, action, **fields):
        return remote.serve({"host": self.config, "action": action, **fields})

    def queued_fixture(self):
        with (
            patch.object(remote, "doctor", return_value={"state": "ready"}),
            patch.object(remote, "run", return_value=command_result()),
        ):
            return self.request("submit", task=task())

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def test_request_dispatch_uses_service_path_not_ambient_path(self):
        with (
            patch.dict(os.environ, {"PATH": "/missing-ambient-tools"}),
            patch.object(remote, "doctor", return_value={"state": "ready"}),
        ):
            result = self.request("submit", task=task())
            self.assertEqual(result["state"], "queued")
            self.assertEqual(os.environ["PATH"], "/missing-ambient-tools")
        self.assertEqual(self.calls()[0]["tool"], "systemd-run")
        self.assertEqual(self.calls()[0]["path"], str(self.bin))

    def test_request_status_uses_service_path_not_ambient_path(self):
        self.queued_fixture()
        with patch.dict(os.environ, {"PATH": "/missing-ambient-tools"}):
            result = self.request("status", id=task()["id"])
            self.assertEqual(result["service"]["ActiveState"], "active")
            self.assertEqual(os.environ["PATH"], "/missing-ambient-tools")
        self.assertEqual(self.calls()[0]["tool"], "systemctl")
        self.assertIn("show", self.calls()[0]["args"])
        self.assertEqual(self.calls()[0]["path"], str(self.bin))

    def test_request_cancel_uses_service_path_not_ambient_path(self):
        self.queued_fixture()
        with patch.dict(os.environ, {"PATH": "/missing-ambient-tools"}):
            result = self.request("cancel", id=task()["id"])
            self.assertEqual(result["state"], "cancelled")
            self.assertEqual(os.environ["PATH"], "/missing-ambient-tools")
        self.assertEqual(self.calls()[0]["tool"], "systemctl")
        self.assertIn("stop", self.calls()[0]["args"])
        self.assertEqual(self.calls()[0]["path"], str(self.bin))

    def test_request_restores_present_or_absent_path_after_exception(self):
        for original in ("/caller-only-path", None):
            with self.subTest(original=original), patch.dict(os.environ):
                if original is None:
                    os.environ.pop("PATH", None)
                else:
                    os.environ["PATH"] = original
                seen = []

                def fail_doctor(config, seen=seen):
                    seen.append(os.environ.get("PATH"))
                    raise ValueError("controlled doctor failure")

                with (
                    patch.object(remote, "doctor", side_effect=fail_doctor),
                    self.assertRaisesRegex(ValueError, "controlled doctor failure"),
                ):
                    self.request("doctor")
                self.assertEqual(seen, [str(self.bin)])
                self.assertEqual(os.environ.get("PATH"), original)

    def test_invalid_host_does_not_change_process_path(self):
        self.config["service_path"] = "relative"
        with patch.dict(os.environ, {"PATH": "/caller-only-path"}):
            with self.assertRaises(ValueError):
                self.request("doctor")
            self.assertEqual(os.environ["PATH"], "/caller-only-path")


if __name__ == "__main__":
    unittest.main()
