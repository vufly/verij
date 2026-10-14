"""Contract checks: neutral hooks and byte-for-byte previous-command passthrough."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

CALLBACK = Path(__file__).with_name("callback.py")
spec = importlib.util.spec_from_file_location("callback", CALLBACK)
assert spec and spec.loader
callback = importlib.util.module_from_spec(spec)
spec.loader.exec_module(callback)


class CallbackTests(unittest.TestCase):
    def test_running_agent_survives_executable_replacement(self):
        stat = "999 (agy) S 1 999 999 34816 999 " + "0 " * 13 + "12345"
        with patch("os.getppid", return_value=999), \
             patch("os.readlink", return_value="/bin/agy (deleted)"), \
             patch("pathlib.Path.read_text", return_value=stat):
            self.assertEqual(callback.agent_parent(), (999, 12345))

    def invoke(self, event, config, raw=b'{"private":"do not persist"}\n'):
        with tempfile.TemporaryDirectory() as root:
            file = Path(root) / "adapter.json"
            file.write_text(json.dumps(config))
            return subprocess.run([sys.executable, str(CALLBACK), event, str(file)],
                                  input=raw, capture_output=True)

    def test_neutral_even_with_missing_reporter_or_invalid_payload(self):
        for event, expected in (("PreInvocation", {}), ("PostInvocation", {}), ("Stop", {"decision": ""})):
            result = self.invoke(event, {"reporter": "/missing", "version": "1.3.2"})
            self.assertEqual(result.returncode, 0)
            self.assertEqual(json.loads(result.stdout), expected)
            self.assertEqual(result.stderr, b"")

    def test_existing_stdout_stderr_stdin_environment_and_nonzero_exit(self):
        command = f"{sys.executable} -c 'import sys; sys.stdout.buffer.write(sys.stdin.buffer.read()); sys.stderr.write(\"marker\"); sys.exit(7)'"
        result = self.invoke("ui", {"previous_command": command})
        self.assertEqual(result.stdout, b'{"private":"do not persist"}\n')
        self.assertEqual(result.stderr, b"marker")
        self.assertEqual(result.returncode, 7)
        empty = self.invoke("ui", {})
        self.assertEqual(empty.stdout, b"")
        self.assertEqual(empty.returncode, 0)
        raw = b"x" * (callback.LIMIT * 2)
        large = self.invoke("ui", {"previous_command": command}, raw)
        self.assertEqual(large.stdout, raw)
        self.assertEqual(large.returncode, 7)

    def test_payload_whitelist_discards_text_and_account_data(self):
        result = callback.snapshot({"conversationId":"c", "fullyIdle":True,"terminationReason":"NO_TOOL_CALL",
                                    "error":"private body","email":"private","transcriptPath":"private"}, "Stop", "1.3.2", 4)
        self.assertTrue(result["error_present"])
        self.assertNotIn("private", json.dumps(result))

    @patch.dict(os.environ, {"ZELLIJ_PANE_ID": "terminal_0"}, clear=True)
    def test_upgraded_binary_reports_with_existing_adapter(self):
        config = {"enabled": True, "reporter": "/bin/verij", "version": "1.3.3",
                  "runtime": "/tmp/runtime", "states": "/tmp/states",
                  "binary_dev": 12, "binary_ino": 34}
        value = {"version": "1.3.5", "agent_state": "working"}
        with patch.object(callback, "agent_parent", return_value=(999, 12345)), \
             patch("pathlib.Path.stat", side_effect=AssertionError("binary inode must not be pinned")), \
             patch.object(callback.subprocess, "run") as reporter:
            callback.report(config, value, "ui", 5000)
        reporter.assert_called_once()
        args, kwargs = reporter.call_args
        self.assertEqual(args[0], ["/bin/verij", "agent", "agy", "--pane", "0",
                                   "--pid", "999", "--birth", "12345"])
        payload = json.loads(kwargs["input"])
        self.assertEqual(payload["version"], "1.3.5")
        self.assertEqual(payload["agent_state"], "working")


if __name__ == "__main__":
    unittest.main()
