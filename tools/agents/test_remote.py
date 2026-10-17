"""Non-graphical runner tests; no SSH, model calls, or real systemd jobs."""

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "remote", Path(__file__).with_name("remote.py")
)
remote = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(remote)
remote.SOURCE = Path(remote.__file__).read_text()


def task():
    return {
        "id": "scout-01",
        "coordinator": "test-parent/session-1",
        "role": "scout",
        "base_commit": "a" * 40,
        "authority": "main",
        "objective": "Inspect the docs",
        "read_paths": ["AGENTS.md"],
        "acceptance": "Report evidence",
        "timeout_seconds": 30,
    }


def host(root):
    return {
        "ssh_target": "wintermute",
        "state_dir": str(root / "jobs"),
        "repo_url": "https://github.com/OldJobobo/splinterm.git",
        "pi": "/usr/bin/pi",
        "pi_version": "1.3.0",
        "provider": "openai-codex",
        "model": "test-model",
    }


def command_result(argv=(), code=0, stdout=""):
    return subprocess.CompletedProcess(argv, code, stdout, "")


def events(text="Evidence-backed report", reason="stop", aborted=False):
    return [
        {"type": "agent_start"},
        {
            "type": "message_end",
            "message": {
                "role": "assistant",
                "stopReason": reason,
                "content": [{"type": "text", "text": text}],
            },
        },
        {"type": "agent_end", "willRetry": False},
        {"type": "agent_settled", "aborted": aborted},
    ]


class ValidationTests(unittest.TestCase):
    def test_valid_task(self):
        self.assertEqual(remote.validate_task(task()), task())

    def test_reject_unsupported_authority_and_bad_paths(self):
        cases = [
            ("role", "worker"),
            ("role", "validator"),
            ("id", "../job"),
            ("base_commit", "main"),
            ("authority", "feat/test"),
            ("timeout_seconds", True),
            ("timeout_seconds", 1801),
            ("read_paths", ["../secret"]),
            ("read_paths", ["/etc/passwd"]),
            ("read_paths", []),
            ("objective", "bad\x1btext"),
        ]
        for key, value in cases:
            with self.subTest(key=key, value=value):
                candidate = task()
                candidate[key] = value
                with self.assertRaises(ValueError):
                    remote.validate_task(candidate)

    def test_unknown_fields_fail_closed(self):
        candidate = task()
        candidate["allow_write"] = True
        with self.assertRaises(ValueError):
            remote.validate_task(candidate)

    def test_host_rejects_shell_and_credentials(self):
        for key, value in [
            ("ssh_target", "-oProxyCommand=evil"),
            ("ssh_target", "host;sh"),
            ("pi", "pi"),
            ("state_dir", "/tmp/../repo"),
            ("repo_url", "https://token@github.com/org/repo"),
            ("repo_url", "/existing/checkout"),
            ("model", "-x"),
        ]:
            with self.subTest(key=key):
                candidate = host(Path("/tmp/test"))
                candidate[key] = value
                with self.assertRaises(ValueError):
                    remote.validate_host(candidate)

    def test_prompt_carries_authority_and_no_shell_boundary(self):
        prompt = remote.prompt_for(task())
        self.assertIn("no shell or test execution tool", prompt)
        self.assertIn("Do not delegate", prompt)
        self.assertIn("test-parent/session-1", prompt)


class PrivateStateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.host = host(self.root)

    def test_rejects_public_directory_and_symlink(self):
        public = self.root / "public"
        public.mkdir(mode=0o755)
        with self.assertRaises(ValueError):
            remote.private_directory(public)
        link = self.root / "link"
        link.symlink_to(self.root, target_is_directory=True)
        with self.assertRaises(ValueError):
            remote.private_directory(link / "child", create=True)
        self.assertFalse((self.root / "child").exists())

    def submit(self):
        with (
            patch.object(remote, "doctor", return_value={"state": "ready"}),
            patch.object(remote, "run", return_value=command_result()) as commands,
        ):
            result = remote.submit(self.host, task())
        return result, commands

    def test_idempotent_submission_only_dispatches_once(self):
        result, commands = self.submit()
        self.assertEqual(result["state"], "queued")
        self.assertEqual(commands.call_count, 1)
        with patch.object(remote, "run") as duplicate:
            self.assertEqual(remote.submit(self.host, task()), result)
            duplicate.assert_not_called()
        argv = commands.call_args.args[0]
        self.assertIn("--property=KillMode=control-group", argv)
        self.assertIn("--property=MemoryMax=4G", argv)
        self.assertIn("--no-block", argv)
        self.assertNotIn("--scope", argv)
        self.assertTrue((Path(self.host["state_dir"]) / "scout-01/runner.py").exists())

    def test_changed_contract_conflicts(self):
        self.submit()
        changed = task()
        changed["objective"] = "Different objective"
        with self.assertRaisesRegex(ValueError, "different task"):
            remote.submit(self.host, changed)
        with (
            patch.object(remote, "SOURCE", "different runner"),
            self.assertRaises(ValueError),
        ):
            remote.submit(self.host, task())

    def test_ambiguous_dispatch_is_not_retried(self):
        with (
            patch.object(remote, "doctor", return_value={"state": "ready"}),
            patch.object(
                remote, "run", side_effect=subprocess.TimeoutExpired("systemd-run", 30)
            ),
        ):
            result = remote.submit(self.host, task())
        self.assertEqual(result["state"], "dispatch_unknown")
        with patch.object(remote, "run") as command:
            self.assertEqual(
                remote.submit(self.host, task())["state"], "dispatch_unknown"
            )
            command.assert_not_called()

    def test_failed_preflight_does_not_dispatch(self):
        with (
            patch.object(remote, "doctor", return_value={"state": "not_ready"}),
            patch.object(remote, "run") as commands,
        ):
            result = remote.submit(self.host, task())
        self.assertEqual(result["state"], "failed")
        commands.assert_not_called()

    def test_status_missing_service_is_not_success(self):
        self.submit()
        job = Path(self.host["state_dir"]) / "scout-01"
        with patch.object(
            remote,
            "service_state",
            return_value={
                "ActiveState": "inactive",
                "LoadState": "not-found",
                "Job": "",
            },
        ):
            result = remote.status(job)
        self.assertEqual(result["state"], "interrupted")

    def test_manager_outage_does_not_rewrite_status(self):
        self.submit()
        job = Path(self.host["state_dir"]) / "scout-01"
        with (
            patch.object(
                remote, "service_state", side_effect=ValueError("unavailable")
            ),
            self.assertRaises(ValueError),
        ):
            remote.status(job)
        self.assertEqual(remote.read_json(job / "status.json")["state"], "queued")

    def test_cancel_targets_only_owned_unit_and_retains_artifacts(self):
        receipt, _ = self.submit()
        job = Path(self.host["state_dir"]) / "scout-01"
        with patch.object(remote, "run", return_value=command_result()) as commands:
            result = remote.cancel(job)
        self.assertEqual(result["state"], "cancelled")
        self.assertEqual(
            commands.call_args.args[0], ["systemctl", "--user", "stop", receipt["unit"]]
        )
        self.assertTrue((job / "request.json").exists())
        with patch.object(remote, "run") as commands:
            remote.cancel(job)
            commands.assert_not_called()

    def test_cancel_reconciles_status_poll_after_successful_stop(self):
        self.submit()
        job = Path(self.host["state_dir"]) / "scout-01"

        def stopped_then_polled(argv, **kwargs):
            self.assertEqual(argv[:3], ["systemctl", "--user", "stop"])
            with patch.object(
                remote,
                "service_state",
                return_value={"ActiveState": "inactive", "Job": ""},
            ):
                self.assertEqual(remote.status(job)["state"], "interrupted")
            return command_result()

        with patch.object(remote, "run", side_effect=stopped_then_polled):
            self.assertEqual(remote.cancel(job)["state"], "cancelled")
        self.assertEqual(remote.read_json(job / "status.json")["state"], "cancelled")

    def test_cancel_preserves_terminal_receipt_written_during_stop(self):
        self.submit()
        job = Path(self.host["state_dir"]) / "scout-01"
        for terminal in ("completed", "failed", "cancelled"):
            with self.subTest(state=terminal):
                remote.update(job, "running")

                def stopped_with_receipt(argv, state=terminal, **kwargs):
                    remote.update(job, state)
                    return command_result()

                with patch.object(remote, "run", side_effect=stopped_with_receipt):
                    self.assertEqual(remote.cancel(job)["state"], terminal)

    def test_terminal_result_is_stable(self):
        self.submit()
        job = Path(self.host["state_dir"]) / "scout-01"
        remote.update(job, "completed")
        with patch.object(remote, "service_state") as service:
            self.assertEqual(remote.status(job)["state"], "completed")
            service.assert_not_called()

    def test_unit_namespace_separates_hosts_and_roots(self):
        self.assertNotEqual(
            remote.unit_name(self.root / "a", "job"),
            remote.unit_name(self.root / "b", "job"),
        )

    def test_existing_unowned_state_is_untouched(self):
        root = Path(self.host["state_dir"])
        root.mkdir(mode=0o700)
        (root / "precious.txt").write_text("preserve me")
        with self.assertRaisesRegex(ValueError, "unowned"):
            remote.submit(self.host, task())
        self.assertEqual(sorted(p.name for p in root.iterdir()), ["precious.txt"])

    def test_pending_service_job_is_not_interrupted(self):
        self.submit()
        job = Path(self.host["state_dir"]) / "scout-01"
        with patch.object(
            remote,
            "service_state",
            return_value={
                "ActiveState": "inactive",
                "Job": "42 /org/freedesktop/systemd1/job/42",
            },
        ):
            self.assertEqual(remote.status(job)["state"], "queued")

    def test_failed_cancel_does_not_claim_stopped(self):
        self.submit()
        job = Path(self.host["state_dir"]) / "scout-01"
        with (
            patch.object(remote, "run", side_effect=ValueError("manager unreachable")),
            self.assertRaises(ValueError),
        ):
            remote.cancel(job)
        self.assertEqual(remote.read_json(job / "status.json")["state"], "cancelling")

    def test_completed_report_integrity_is_checked(self):
        self.submit()
        job = Path(self.host["state_dir"]) / "scout-01"
        remote.update(job, "completed", report_sha256="invalid")
        (job / "report.md").write_text("tampered")
        with (
            patch.object(remote.os, "umask"),
            self.assertRaisesRegex(ValueError, "does not match"),
        ):
            remote.serve({"host": self.host, "action": "result", "id": "scout-01"})

    def test_git_does_not_inherit_worktree_or_global_hooks(self):
        with (
            patch.dict(
                remote.os.environ,
                {"GIT_DIR": "/production/.git", "GIT_CONFIG_COUNT": "1"},
            ),
            patch.object(
                remote.subprocess, "run", return_value=command_result()
            ) as command,
        ):
            remote.run(["git", "status"])
        environment = command.call_args.kwargs["env"]
        self.assertNotIn("GIT_DIR", environment)
        self.assertNotIn("GIT_CONFIG_COUNT", environment)
        self.assertEqual(environment["GIT_CONFIG_GLOBAL"], remote.os.devnull)


class CompletionTests(unittest.TestCase):
    def parse(self, data, code=0):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "events.jsonl"
            path.write_text(
                "\n".join(json.dumps(e, ensure_ascii=False) for e in data) + "\n"
            )
            return remote.completed_text(path, code)

    def test_success_and_unicode_line_separators(self):
        self.assertEqual(self.parse(events("A\u2028B")), "A\u2028B")

    def test_error_abort_length_and_empty_are_not_success(self):
        for data in [
            events(reason="error"),
            events(reason="aborted"),
            events(reason="length"),
            events(aborted=True),
            events(text=""),
            events()[:-1],
            [],
        ]:
            with self.subTest(data=data), self.assertRaises(ValueError):
                self.parse(data)
        with self.assertRaises(ValueError):
            self.parse(events(), code=1)

    def test_recovery_requires_new_settlement(self):
        data = events() + [{"type": "agent_start"}]
        with self.assertRaises(ValueError):
            self.parse(data)
        self.assertEqual(
            self.parse(events(reason="error") + events()), "Evidence-backed report"
        )

    def test_malformed_json_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "events.jsonl"
            path.write_text("not json\n")
            with self.assertRaises(ValueError):
                remote.completed_text(path, 0)


class ProcessTests(unittest.TestCase):
    def test_streams_stdout_and_stderr(self):
        with tempfile.TemporaryDirectory() as directory:
            job = Path(directory)
            code = remote.stream_process(
                [
                    sys.executable,
                    "-c",
                    "import sys; print('out'); print('err',file=sys.stderr)",
                ],
                job,
                job,
                5,
            )
            self.assertEqual(code, 0)
            self.assertEqual((job / "events.jsonl").read_text(), "out\n")
            self.assertEqual((job / "stderr.log").read_text(), "err\n")

    def test_timeout_stops_owned_process(self):
        with (
            tempfile.TemporaryDirectory() as directory,
            self.assertRaisesRegex(ValueError, "runtime"),
        ):
            remote.stream_process(
                [sys.executable, "-c", "import time; time.sleep(20)"],
                Path(directory),
                Path(directory),
                0.1,
            )

    def test_log_cap(self):
        with (
            tempfile.TemporaryDirectory() as directory,
            patch.object(remote, "MAX_LOG", 100),
            self.assertRaisesRegex(ValueError, "log size"),
        ):
            remote.stream_process(
                [sys.executable, "-c", "print('x'*200)"],
                Path(directory),
                Path(directory),
                5,
            )


class TransportTests(unittest.TestCase):
    def test_fixed_ssh_command_stdin_payload_and_host_key_check(self):
        config = host(Path("/tmp/test"))
        request = {"action": "submit", "task": task()}
        request["task"]["objective"] = "Quote ' ; $(touch /tmp/no) is just task text"
        with patch.object(
            remote.subprocess,
            "run",
            return_value=command_result(stdout='{"state":"queued"}'),
        ) as command:
            remote.transport(config, request)
        argv = command.call_args.args[0]
        self.assertIn("StrictHostKeyChecking=yes", argv)
        self.assertNotIn("$(touch", " ".join(argv))
        payload = json.loads(command.call_args.kwargs["input"])
        self.assertEqual(payload["task"], request["task"])
        self.assertIn("def serve", payload["source"])
        self.assertNotIn("shell", command.call_args.kwargs)

    def test_ssh_timeout_warns_against_duplicate_job(self):
        with (
            patch.object(
                remote.subprocess,
                "run",
                side_effect=subprocess.TimeoutExpired("ssh", 150),
            ),
            self.assertRaisesRegex(ValueError, "same job id"),
        ):
            remote.transport(host(Path("/tmp/test")), {"action": "doctor"})

    def test_bootstrap_runs_source_and_returns_error_without_mutating(self):
        config = host(Path("/tmp/unused"))
        config["pi"] = "relative-invalid"
        payload = remote.canonical(
            {"source": remote.SOURCE, "host": config, "action": "doctor"}
        )
        result = subprocess.run(
            [sys.executable, "-c", remote.BOOTSTRAP],
            input=payload,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("absolute", json.loads(result.stdout)["error"])


class ExecuteTests(unittest.TestCase):
    setUp = PrivateStateTests.setUp
    submit = PrivateStateTests.submit

    def test_full_read_only_execution_with_fake_tools(self):
        self.submit()
        job = Path(self.host["state_dir"]) / "scout-01"
        seen = []

        def fake_run(argv, **kwargs):
            if "checkout" in argv:
                (job / "repo/AGENTS.md").write_text("# Guardrails\n")
            return command_result()

        def fake_pi(argv, cwd, directory, timeout):
            seen.extend(argv)
            (directory / "events.jsonl").write_text(
                "\n".join(json.dumps(e) for e in events())
            )
            return 0

        with (
            patch.object(remote, "doctor", return_value={"state": "ready"}),
            patch.object(remote, "run", side_effect=fake_run),
            patch.object(remote, "stream_process", side_effect=fake_pi),
        ):
            self.assertEqual(remote.execute(job), 0)
        self.assertEqual(remote.read_json(job / "status.json")["state"], "completed")
        for flag in ["--no-extensions", "--no-mcp", "--no-approve", "--no-skills"]:
            self.assertIn(flag, seen)
        self.assertEqual(seen[seen.index("--tools") + 1], "read,grep,find,ls")
        self.assertIn("Evidence-backed", (job / "report.md").read_text())

    def test_duplicate_execute_never_restarts_completed_job(self):
        self.submit()
        job = Path(self.host["state_dir"]) / "scout-01"
        remote.update(job, "completed")
        with patch.object(remote, "doctor") as doctor:
            self.assertEqual(remote.execute(job), 1)
            doctor.assert_not_called()


if __name__ == "__main__":
    unittest.main()
