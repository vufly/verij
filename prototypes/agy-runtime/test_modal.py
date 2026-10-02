#!/usr/bin/env python3
"""Regression boundaries for the operator's one-time approval in private probes."""
import unittest
from verify_interactive import exact_native_once_modal


class ModalTests(unittest.TestCase):
    def test_exact_known_command_and_native_one_time_option(self):
        frame = b"\xe2\x97\x8f bash(printf vj_permission) (ctrl+o to expand)\n\nprintf vj_permission\n\n> 1. yes, run command\n2. yes, and always allow\n"
        self.assertTrue(exact_native_once_modal(frame, "printf vj_permission"))

    def test_extra_command_is_never_approved(self):
        frame = b"bash(printf vj_permission)\nprintf vj_permission\nadditional_command\n> 1. yes, run command\n"
        self.assertFalse(exact_native_once_modal(frame, "printf vj_permission"))

    def test_fixed_native_confirmation_label_is_not_command_content(self):
        frame = b"bash(printf vj_permission)\nprintf vj_permission\n\nRun this command?\n> 1. yes, run command\n".lower()
        self.assertTrue(exact_native_once_modal(frame, "printf vj_permission"))

    def test_always_allow_is_not_one_time_choice(self):
        frame = b"bash(printf vj_permission)\nprintf vj_permission\n> 1. yes, and always allow\n"
        self.assertFalse(exact_native_once_modal(frame, "printf vj_permission"))

    def test_unexpected_command_summary_is_not_approved(self):
        frame = b"bash(other_command)\nprintf vj_permission\n> 1. yes, run command\n"
        self.assertFalse(exact_native_once_modal(frame, "printf vj_permission"))


if __name__ == "__main__":
    unittest.main()
