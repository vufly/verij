#!/usr/bin/env python3
"""Fail-closed boundary regressions for the experimental control helper."""
import unittest
from unittest.mock import patch

from verify import blob, completion_wire, fields, process_identity


class BoundaryTests(unittest.TestCase):
    def test_unreadable_process_never_establishes_liveness(self):
        for error in (PermissionError(), ProcessLookupError(), FileNotFoundError()):
            with self.subTest(error=type(error).__name__), patch("pathlib.Path.read_text", side_effect=error):
                self.assertIsNone(process_identity(123))

    def test_malformed_proc_stat_never_establishes_liveness(self):
        for value in ("", "123 (short) R", "123 no-comm R"):
            with self.subTest(value=value), patch("pathlib.Path.read_text", return_value=value):
                self.assertIsNone(process_identity(123))

    def test_truncated_length_delimited_field_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "truncated"):
            fields(blob(1, b"correlation")[:-1])

    def test_overflowing_uint64_varint_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "uint64"):
            fields(b"\x08" + b"\xff" * 9 + b"\x02")

    def test_out_of_range_identity_is_not_sent(self):
        record = {"client_id": 2**16, "connection_id": "generation"}
        with self.assertRaisesRegex(ValueError, "client_id"):
            completion_wire(record, 0, 0, True, "query")

    def test_optional_zero_ids_and_max_sequence_remain_encodable(self):
        record = {"client_id": 1, "connection_id": "generation"}
        wire = completion_wire(record, 0, 2**64 - 1, False, "last")
        payload = dict(fields(dict(fields(wire))[30]))
        self.assertEqual(payload[4], 0)
        self.assertEqual(payload[5], 2**64 - 1)


if __name__ == "__main__":
    unittest.main()
