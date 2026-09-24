#!/usr/bin/env python3
"""Unit tests for bounded QMP input encoding."""

from __future__ import annotations

import importlib.util
import io
from pathlib import Path
import unittest


MODULE_PATH = Path(__file__).with_name("qmp-input.py")
SPEC = importlib.util.spec_from_file_location("qmp_input", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
qmp_input = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(qmp_input)


class QmpInputTests(unittest.TestCase):
    def test_ascii_encoder_releases_shift_and_keys(self) -> None:
        encoded = list(qmp_input.ascii_events("A-1\n"))
        self.assertEqual(
            [event["data"]["key"]["data"] for event in encoded[0]],
            ["shift", "a", "a", "shift"],
        )
        self.assertEqual(
            [event["data"]["down"] for event in encoded[0]],
            [True, True, False, False],
        )
        self.assertEqual(encoded[1], qmp_input.tap_events("minus"))
        self.assertEqual(encoded[2], qmp_input.tap_events("1"))
        self.assertEqual(encoded[3], qmp_input.tap_events("ret"))

    def test_ascii_encoder_rejects_unbounded_unicode(self) -> None:
        with self.assertRaisesRegex(ValueError, "U\\+2603"):
            list(qmp_input.ascii_events("snowman ☃"))

    def test_ascii_encoder_enforces_exact_byte_limit(self) -> None:
        accepted = "a" * qmp_input.MAX_TYPED_TEXT_BYTES
        self.assertEqual(len(list(qmp_input.ascii_events(accepted))), len(accepted))
        with self.assertRaisesRegex(ValueError, "exceeds"):
            list(qmp_input.ascii_events(accepted + "a"))

    def test_qmp_reader_bounds_and_requires_newline(self) -> None:
        client = qmp_input.QmpClient(Path("/unused"))
        client.stream = io.BytesIO(b'{"return": {}}\n')
        self.assertEqual(client._read_message(), {"return": {}})

        client.stream = io.BytesIO(b"x" * (qmp_input.MAX_QMP_LINE_BYTES + 1))
        with self.assertRaisesRegex(RuntimeError, "exceeds"):
            client._read_message()

        client.stream = io.BytesIO(b'{"return": {}}')
        with self.assertRaisesRegex(RuntimeError, "unterminated"):
            client._read_message()

    def test_absolute_pointer_coordinates_cover_qmp_range(self) -> None:
        self.assertEqual(qmp_input.absolute_value(0, 1920), 0)
        self.assertEqual(qmp_input.absolute_value(1919, 1920), 32767)
        self.assertEqual(qmp_input.absolute_value(959, 1920), 16375)

    def test_absolute_pointer_coordinates_reject_out_of_bounds(self) -> None:
        with self.assertRaises(ValueError):
            qmp_input.absolute_value(-1, 1080)
        with self.assertRaises(ValueError):
            qmp_input.absolute_value(1080, 1080)
        with self.assertRaises(ValueError):
            qmp_input.absolute_value(0, 1)


if __name__ == "__main__":
    unittest.main()
