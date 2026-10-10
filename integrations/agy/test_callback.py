"""Contract checks: neutral hooks and byte-for-byte previous-command passthrough."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

CALLBACK = Path(__file__).with_name("callback.py")
spec = importlib.util.spec_from_file_location("callback", CALLBACK)
assert spec and spec.loader
callback = importlib.util.module_from_spec(spec)
spec.loader.exec_module(callback)


class CallbackTests(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
