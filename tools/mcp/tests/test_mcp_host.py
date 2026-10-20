#!/usr/bin/env python3
"""Subprocess tests for the bounded MCP stdio host driver."""

from __future__ import annotations

import json
import os
from pathlib import Path
import sys
import time
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

from mcp_host import HostError, McpHost, parse_tool_result  # noqa: E402

FAKE = HERE / "fake_adapter.py"


def start(mode: str, **kwargs) -> McpHost:
    return McpHost([sys.executable, str(FAKE), "--mode", mode], {"PATH": os.environ.get("PATH", "")}, **kwargs)


class HostTestCase(unittest.TestCase):
    def host(self, mode: str, **kwargs) -> McpHost:
        host = start(mode, **kwargs)
        self.addCleanup(host.close, 2, 1)
        return host


class ResponseMatchingTests(HostTestCase):
    def test_out_of_order_responses_match_their_ids(self) -> None:
        host = self.host("reorder")
        first = host.send_request("a", {}, enforce_lifecycle=False)
        second = host.send_request("b", {}, enforce_lifecycle=False)
        self.assertEqual(host.wait_response(second, 5)["result"], {"order": 2})
        self.assertEqual(host.wait_response(first, 5)["result"], {"order": 1})
        self.assertIsNone(host.failure)

    def test_ids_are_monotonic(self) -> None:
        host = self.host("echo")
        ids = [host.send_request("x", {}, enforce_lifecycle=False) for _ in range(3)]
        self.assertEqual(ids, sorted(ids))
        self.assertEqual(len(set(ids)), 3)
        for request_id in ids:
            host.wait_response(request_id, 5)

    def test_interleaved_notifications_are_retained(self) -> None:
        host = self.host("notify")
        response = host.request("x", {}, enforce_lifecycle=False, timeout=5)
        self.assertEqual(response["result"], {})
        taken = host.take_notifications(lambda item: item["method"] == "notifications/resources/updated")
        self.assertEqual(len(taken), 1)
        self.assertEqual(host.take_notifications(), [])

    def test_notification_overflow_is_explicit(self) -> None:
        host = self.host("notify-many", max_notifications=2)
        host.request("x", {}, enforce_lifecycle=False, timeout=5)
        with self.assertRaises(HostError) as caught:
            host.take_notifications()
        self.assertEqual(caught.exception.code, "notification_overflow")

    def test_unknown_response_id_is_a_protocol_failure(self) -> None:
        host = self.host("unknown-id")
        with self.assertRaises(HostError) as caught:
            host.request("x", {}, enforce_lifecycle=False, timeout=5)
        self.assertEqual(caught.exception.code, "protocol")

    def test_duplicate_response_id_is_a_protocol_failure(self) -> None:
        host = self.host("duplicate")
        host.request("x", {}, enforce_lifecycle=False, timeout=5)
        with self.assertRaises(HostError) as caught:
            host.request("y", {}, enforce_lifecycle=False, timeout=5)
        self.assertEqual(caught.exception.code, "protocol")

    def test_server_request_is_never_acted_on(self) -> None:
        host = self.host("peer-request")
        with self.assertRaises(HostError) as caught:
            host.request("x", {}, enforce_lifecycle=False, timeout=5)
        self.assertEqual(caught.exception.code, "protocol")


class LifecycleTests(HostTestCase):
    def test_initialize_then_initialized_ordering(self) -> None:
        host = self.host("initialize")
        with self.assertRaises(HostError) as caught:
            host.request("tools/list", {})
        self.assertEqual(caught.exception.code, "lifecycle")
        result = host.initialize(timeout=5)
        self.assertEqual(result["protocolVersion"], "2025-11-25")
        self.assertEqual(host.state, "initialized")
        with self.assertRaises(HostError) as caught:
            host.initialize(timeout=5)
        self.assertEqual(caught.exception.code, "lifecycle")

    def test_initialize_can_defer_initialized(self) -> None:
        host = self.host("initialize")
        host.initialize(send_initialized=False, timeout=5)
        self.assertEqual(host.state, "initialize_accepted")
        with self.assertRaises(HostError):
            host.request("tools/list", {})
        host.send_initialized()
        self.assertEqual(host.state, "initialized")

    def test_version_echo_must_be_exact(self) -> None:
        host = self.host("wrong-version")
        with self.assertRaises(HostError) as caught:
            host.initialize(timeout=5)
        self.assertEqual(caught.exception.code, "protocol")
        self.assertEqual(host.state, "rejected")


class DeadlineTests(HostTestCase):
    def test_timeout_is_distinct_from_eof(self) -> None:
        host = self.host("silent")
        started = time.monotonic()
        with self.assertRaises(HostError) as caught:
            host.request("x", {}, enforce_lifecycle=False, timeout=0.3)
        self.assertEqual(caught.exception.code, "timeout")
        self.assertLess(time.monotonic() - started, 3)

    def test_deadline_holds_despite_notification_traffic(self) -> None:
        host = self.host("chatty-silent", max_notifications=10_000)
        started = time.monotonic()
        with self.assertRaises(HostError) as caught:
            host.request("x", {}, enforce_lifecycle=False, timeout=0.4)
        self.assertEqual(caught.exception.code, "timeout")
        self.assertLess(time.monotonic() - started, 2)

    def test_late_reply_cannot_satisfy_another_request(self) -> None:
        host = self.host("late")
        with self.assertRaises(HostError):
            host.request("x", {}, enforce_lifecycle=False, timeout=0.1)
        response = host.request("y", {}, enforce_lifecycle=False, timeout=5)
        self.assertEqual(response["id"], 2)
        self.assertEqual(host.late_responses, 1)
        self.assertIsNone(host.failure)

    def test_cancellation_targets_the_original_id(self) -> None:
        host = self.host("late")
        cancelled = host.send_request("x", {}, enforce_lifecycle=False)
        host.cancel(cancelled)
        with self.assertRaises(HostError) as caught:
            host.wait_response(cancelled, 1)
        self.assertEqual(caught.exception.code, "cancelled")
        # The fake still answers the cancelled ID late; that reply is
        # discarded and the next request gets its own reply.
        response = host.request("y", {}, enforce_lifecycle=False, timeout=5)
        self.assertNotEqual(response["id"], cancelled)
        self.assertEqual(host.late_responses, 1)

    def test_stalled_stdin_write_times_out(self) -> None:
        host = self.host("no-read")
        with self.assertRaises(HostError) as caught:
            host.send_bytes(b"x" * (8 * 1024 * 1024), timeout=0.3)
        self.assertEqual(caught.exception.code, "write_timeout")


class MalformedOutputTests(HostTestCase):
    def assert_failure(self, mode: str, code: str, **kwargs) -> None:
        host = self.host(mode, **kwargs)
        with self.assertRaises(HostError) as caught:
            host.request("x", {}, enforce_lifecycle=False, timeout=5)
        self.assertEqual(caught.exception.code, code)
        self.assertEqual(host.failure, code)

    def test_invalid_json(self) -> None:
        self.assert_failure("garbage", "malformed_output")

    def test_invalid_utf8(self) -> None:
        self.assert_failure("badutf8", "malformed_output")

    def test_duplicate_keys(self) -> None:
        self.assert_failure("dupkey", "malformed_output")

    def test_wrong_jsonrpc_version(self) -> None:
        self.assert_failure("envelope", "invalid_envelope")

    def test_result_and_error_together(self) -> None:
        self.assert_failure("both", "invalid_envelope")

    def test_oversized_line(self) -> None:
        self.assert_failure("big", "oversized_output", max_line_bytes=1024)

    def test_oversized_partial_line(self) -> None:
        self.assert_failure("big-partial", "oversized_output", max_line_bytes=1024)


class ExitAndCleanupTests(HostTestCase):
    def test_exit_before_initialization(self) -> None:
        host = self.host("exit-now")
        time.sleep(0.2)
        with self.assertRaises(HostError) as caught:
            host.initialize(timeout=5)
        self.assertIn(caught.exception.code, ("eof", "write_failed"))
        record = host.close(2, 1)
        self.assertEqual(record.returncode, 3)
        self.assertFalse(record.forced)

    def test_exit_during_request_is_eof_not_timeout(self) -> None:
        host = self.host("exit-on-request")
        started = time.monotonic()
        with self.assertRaises(HostError) as caught:
            host.request("x", {}, enforce_lifecycle=False, timeout=10)
        self.assertEqual(caught.exception.code, "eof")
        self.assertLess(time.monotonic() - started, 5)
        self.assertEqual(host.close(2, 1).returncode, 4)

    def test_empty_stdin_close_is_clean(self) -> None:
        record = self.host("echo").close(5, 1)
        self.assertTrue(record.clean)
        self.assertTrue(record.pipes_closed)

    def test_eof_with_work_in_flight(self) -> None:
        host = self.host("late")
        host.send_request("x", {}, enforce_lifecycle=False)
        record = host.close(5, 1)
        self.assertEqual(record.returncode, 0)
        self.assertFalse(record.forced)

    def test_stderr_is_drained_and_bounded(self) -> None:
        host = self.host("stderr-flood", stderr_limit=1024)
        self.assertIn("echo", host.request("x", {}, enforce_lifecycle=False, timeout=10)["result"])
        record = host.close(5, 1)
        self.assertEqual(record.stderr_bytes, 1024 * 1024)
        self.assertTrue(record.stderr_truncated)
        self.assertEqual(len(host.stderr_snapshot()), 1024)

    def test_trailing_partial_frame_is_counted(self) -> None:
        host = self.host("trailing")
        self.assertTrue(host.wait_for_output(5))
        record = host.close(2, 1)
        self.assertGreater(record.stdout_trailing_bytes, 0)

    def test_close_is_idempotent(self) -> None:
        host = self.host("echo")
        self.assertIs(host.close(5, 1), host.close(5, 1))
        with self.assertRaises(HostError) as caught:
            host.send_bytes(b"{}\n")
        self.assertEqual(caught.exception.code, "closed")

    def test_escalates_to_sigterm_for_exact_child(self) -> None:
        host = self.host("ignore-eof")
        record = host.close(0.3, 2)
        self.assertTrue(record.forced)
        self.assertEqual(record.signal_sent, "SIGTERM")
        self.assertEqual(record.returncode, -15)

    def test_escalates_to_sigkill_when_sigterm_is_ignored(self) -> None:
        host = self.host("ignore-term")
        host.request("x", {}, enforce_lifecycle=False, timeout=5)  # handler installed
        record = host.close(0.3, 0.3)
        self.assertTrue(record.forced)
        self.assertEqual(record.signal_sent, "SIGKILL")
        self.assertEqual(record.returncode, -9)
        with self.assertRaises(ProcessLookupError):
            os.kill(host.pid, 0)


class EventPrivacyTests(HostTestCase):
    def test_events_carry_no_bodies(self) -> None:
        events: list[dict] = []
        host = self.host("echo", event_sink=events.append)
        response = host.request("tools/call", {"text": "SECRET-INPUT-BODY"}, enforce_lifecycle=False, timeout=5)
        self.assertIn("FAKE-TERMINAL-SECRET", json.dumps(response))
        host.close(5, 1)
        serialized = json.dumps(events)
        self.assertNotIn("SECRET-INPUT-BODY", serialized)
        self.assertNotIn("FAKE-TERMINAL-SECRET", serialized)
        allowed = {"t_ms", "pid", "direction", "kind", "method", "id", "outcome", "rpc_code", "tool", "tool_error_code", "bytes"}
        for event in events:
            self.assertLessEqual(set(event), allowed)
        self.assertEqual([event["kind"] for event in events][0], "spawn")
        self.assertEqual([event["kind"] for event in events][-1], "exit")

    def test_untrusted_method_names_are_redacted(self) -> None:
        events: list[dict] = []
        host = self.host("echo", event_sink=events.append)
        host.notify("method with spaces and $ecrets")
        host.close(5, 1)
        self.assertIn("<redacted>", json.dumps(events))


class ToolResultTests(unittest.TestCase):
    def test_structured_content_preferred(self) -> None:
        outcome = parse_tool_result({
            "isError": True,
            "content": [],
            "structuredContent": {"error": {"code": "unauthorized"}},
        })
        self.assertTrue(outcome.is_error)
        self.assertEqual(outcome.error_code, "unauthorized")

    def test_text_content_fallback(self) -> None:
        outcome = parse_tool_result({
            "content": [{"type": "text", "text": json.dumps({"ok": True})}],
        })
        self.assertFalse(outcome.is_error)
        self.assertEqual(outcome.structured, {"ok": True})

    def test_invalid_envelopes(self) -> None:
        for result in (None, [], {"isError": "yes"}, {"content": {}}, {"structuredContent": []}):
            with self.subTest(result=result):
                with self.assertRaises(HostError):
                    parse_tool_result(result)

    def test_untrusted_error_code_is_not_recorded_verbatim(self) -> None:
        outcome = parse_tool_result({"isError": True, "structuredContent": {"error": {"code": "a b c\n"}}})
        self.assertIsNone(outcome.error_code)


if __name__ == "__main__":
    unittest.main()
