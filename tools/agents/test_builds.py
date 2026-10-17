"""Build contracts without network, models, or a real service manager."""

import hashlib
import json
import time
import unittest
from pathlib import Path
from unittest.mock import patch

import test_remote as fixtures
from test_remote import command_result, events, host, remote, task


def build_task():
    value = task()
    value.update(
        role="validator", commands=[["cargo", "check", "-p", "splinterm-protocol"]]
    )
    return value


class BuildContractTests(unittest.TestCase):
    def test_valid_build_and_host(self):
        self.assertEqual(remote.validate_task(build_task()), build_task())
        config = host(Path("/tmp/test"))
        config["build_uid"] = 1001
        self.assertEqual(remote.validate_host(config), config)

    def test_doctor_reports_build_readiness_and_failure(self):
        config = {**host(Path("/tmp/test")), "build_uid": 1001}

        def fake_run(argv, **kwargs):
            if argv[0] == config["pi"]:
                text = "1.3.0" if argv[1] == "--version" else '{"status":"ready"}'
            else:
                text = "yes" if argv[0] == "loginctl" else ""
            return command_result(stdout=text)

        with patch.object(remote, "run", side_effect=fake_run):
            with patch.object(remote, "build_preflight"):
                result = remote.doctor(config)
            self.assertEqual(result["state"], "ready")
            self.assertEqual(result["build"]["state"], "ready")
            with patch.object(
                remote,
                "build_preflight",
                side_effect=ValueError("namespace unavailable"),
            ):
                result = remote.doctor(config)
            self.assertEqual(result["state"], "not_ready")
            self.assertIn("namespace unavailable", result["build"]["detail"])

    def test_bad_commands_and_opt_in(self):
        for commands in (
            [],
            [["bash", "-c", "true"]],
            [["cargo", "install", "foo"]],
            [["cargo", "fmt"]],
            [["cargo", "check", None]],
            ["cargo check"],
        ):
            with self.subTest(commands=commands), self.assertRaises(ValueError):
                remote.validate_task({**build_task(), "commands": commands})
        with self.assertRaises(ValueError):
            remote.validate_task({**task(), "commands": [["cargo", "check"]]})
        for uid in (True, 0, "1001"):
            with self.subTest(uid=uid), self.assertRaises(ValueError):
                remote.validate_host({**host(Path("/tmp/test")), "build_uid": uid})
        with (
            patch.object(remote.os, "getuid", return_value=1001),
            self.assertRaisesRegex(ValueError, "restricted worker"),
        ):
            remote.build_preflight(host(Path("/tmp/test")))

    def test_sandbox_has_no_host_root_home_runtime_or_env(self):
        argv = remote.sandbox_base()
        self.assertIn("--unshare-all", argv)
        self.assertIn("--clearenv", argv)
        self.assertIn("--new-session", argv)
        self.assertNotIn("--share-net", argv)
        mounts = [argv[i + 1] for i, value in enumerate(argv) if value == "--ro-bind"]
        for forbidden in ("/", "/home", "/run", "/dev", "/proc"):
            self.assertNotIn(forbidden, mounts)
        self.assertIn("--share-net", remote.sandbox_base(network=True))

    def test_public_arch_trust_bundle_target_is_mounted_without_private_keys(self):
        with patch.object(remote.Path, "exists", return_value=True):
            argv = remote.sandbox_base(network=True)
        self.assertIn("/etc/ca-certificates/extracted", argv)
        self.assertIn("/etc/ssl/certs", argv)
        self.assertNotIn("/etc/ssl", argv)
        self.assertNotIn("/etc/ssl/private", argv)

    def test_analyst_has_no_shell_authority(self):
        prompt = remote.prompt_for(build_task())
        self.assertIn("already executed", prompt)
        self.assertIn("Do not execute commands", prompt)
        self.assertIn("../checks.json", prompt)


class BuildExecutionTests(unittest.TestCase):
    setUp = fixtures.PrivateStateTests.setUp
    submit = fixtures.PrivateStateTests.submit

    def test_command_failure_cannot_be_laundered_by_model_report(self):
        self.submit()
        job = Path(self.host["state_dir"]) / "scout-01"
        remote.atomic_json(
            job / "request.json", {"host": self.host, "task": build_task()}
        )

        def fake_run(argv, **kwargs):
            if "checkout" in argv:
                (job / "repo/AGENTS.md").write_text("# Rules\n")
            return command_result()

        def fake_pi(argv, cwd, directory, timeout):
            (directory / "events.jsonl").write_text(
                "\n".join(json.dumps(e) for e in events("Looks good"))
            )
            return 0

        with (
            patch.object(remote, "doctor", return_value={"state": "ready"}),
            patch.object(remote, "run", side_effect=fake_run),
            patch.object(
                remote,
                "execute_checks",
                return_value={"state": "failed", "checks": [{"exit_code": 1}]},
            ),
            patch.object(remote, "stream_process", side_effect=fake_pi),
        ):
            self.assertEqual(remote.execute(job), 1)
        receipt = remote.read_json(job / "status.json")
        self.assertEqual(receipt["state"], "failed")
        self.assertEqual(receipt["validation"]["state"], "failed")
        with patch.object(remote.os, "umask"):
            result = remote.serve(
                {"host": self.host, "action": "result", "id": "scout-01"}
            )
        self.assertIn("Looks good", result["report"])
        self.assertEqual(result["state"], "failed")

    def test_post_check_checkpoint_preserves_cancellation_and_terminal_states(self):
        for state in ("cancelling", "cancelled", "interrupted"):
            with self.subTest(state=state):
                candidate = {**task(), "id": f"transition-{state}"}
                with (
                    patch.object(remote, "doctor", return_value={"state": "ready"}),
                    patch.object(remote, "run", return_value=command_result()),
                ):
                    remote.submit(self.host, candidate)
                job = Path(self.host["state_dir"]) / candidate["id"]
                remote.atomic_json(
                    job / "request.json",
                    {
                        "host": self.host,
                        "task": {**build_task(), "id": candidate["id"]},
                    },
                )

                def fake_run(argv, job=job, **kwargs):
                    if "checkout" in argv:
                        (job / "repo/AGENTS.md").write_text("# Rules\n")
                    return command_result()

                def finish_checks(*args, job=job, state=state):
                    remote.update(job, state, detail="preserve this receipt")
                    return {"state": "passed", "checks": []}

                with (
                    patch.object(remote, "doctor", return_value={"state": "ready"}),
                    patch.object(remote, "run", side_effect=fake_run),
                    patch.object(remote, "execute_checks", side_effect=finish_checks),
                    patch.object(remote, "stream_process") as pi,
                ):
                    result = remote.execute(job)
                pi.assert_not_called()
                self.assertEqual(result, 1)
                receipt = remote.read_json(job / "status.json")
                self.assertEqual(receipt["state"], state)
                self.assertEqual(receipt["detail"], "preserve this receipt")
                self.assertFalse((job / "prompt.txt").exists())
                self.assertFalse((job / "report.md").exists())

    def run_checks(self, codes, *, commands=None, error=None):
        self.submit()
        job = Path(self.host["state_dir"]) / "scout-01"
        (job / "repo").mkdir()
        calls = []
        candidate = build_task()
        if commands is not None:
            candidate["commands"] = commands

        def stream(argv, cwd, directory, timeout, **kwargs):
            calls.append(argv)
            self.assertEqual(kwargs["env"], {"PATH": "/usr/bin:/bin"})
            (directory / "stdout.log").write_text("compiler output\n")
            (directory / "stderr.log").write_text("diagnostics\n")
            if error:
                raise error
            return codes[len(calls) - 1]

        with (
            patch.object(remote, "build_preflight"),
            patch.object(remote, "stream_process", side_effect=stream),
        ):
            result = remote.execute_checks(
                self.host, candidate, job / "repo", job, time.monotonic() + 30
            )
        return result, calls, job

    def test_fetch_is_only_networked_step_and_build_is_locked_offline(self):
        result, calls, job = self.run_checks([0, 0])
        self.assertEqual(result["state"], "passed")
        self.assertIn("--share-net", calls[0])
        self.assertNotIn("--share-net", calls[1])
        self.assertEqual(
            calls[1][-6:],
            [
                "/usr/bin/cargo",
                "check",
                "--locked",
                "--offline",
                "-p",
                "splinterm-protocol",
            ],
        )
        self.assertIn("--ro-bind", calls[1])
        self.assertEqual(
            result["checks"][1]["stdout.log_sha256"],
            hashlib.sha256(b"compiler output\n").hexdigest(),
        )
        self.assertEqual(remote.read_json(job / "checks.json")["state"], "passed")

    def test_failed_fetch_stops_before_compilation(self):
        result, calls, _ = self.run_checks([1])
        self.assertEqual(result["state"], "failed")
        self.assertEqual(len(calls), 1)
        self.assertEqual(result["planned_checks"], 2)

    def test_fmt_does_not_fetch_or_get_unsupported_cargo_flags(self):
        result, calls, _ = self.run_checks(
            [0], commands=[["cargo", "fmt", "--all", "--check"]]
        )
        self.assertEqual(result["state"], "passed")
        self.assertEqual(len(calls), 1)
        self.assertNotIn("--share-net", calls[0])
        self.assertEqual(calls[0][-4:], ["/usr/bin/cargo", "fmt", "--all", "--check"])

    def test_timeout_is_recorded_before_propagating(self):
        with self.assertRaisesRegex(ValueError, "runtime"):
            self.run_checks([], error=ValueError("runtime exhausted"))
        receipt = remote.read_json(
            Path(self.host["state_dir"]) / "scout-01/checks.json"
        )
        self.assertEqual(receipt["state"], "failed")
        self.assertIn("runtime", receipt["checks"][0]["error"])


if __name__ == "__main__":
    unittest.main()
