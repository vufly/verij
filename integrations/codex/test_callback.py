"""Contract checks: non-gating neutral hook guarantees and payload filtering for Codex."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch, MagicMock

CALLBACK = Path(__file__).with_name("callback.py")
spec = importlib.util.spec_from_file_location("callback", CALLBACK)
assert spec and spec.loader
callback = importlib.util.module_from_spec(spec)
spec.loader.exec_module(callback)


class CallbackTests(unittest.TestCase):
    def invoke(self, event, config, raw=b'{"hook_event_name":"Stop","private":"do not persist"}\n'):
        env = dict(os.environ)
        env.pop("MAGY_WATCH_ID", None)
        with tempfile.TemporaryDirectory() as root:
            file = Path(root) / "adapter.json"
            file.write_text(json.dumps(config))
            return subprocess.run(
                [sys.executable, str(CALLBACK), event, str(file)],
                input=raw,
                capture_output=True,
                env=env,
                timeout=5,
            )

    def test_neutral_even_with_missing_reporter_or_invalid_payload(self):
        for event in callback.SUPPORTED_EVENTS:
            result = self.invoke(event, {"reporter": "/missing", "version": "0.162.1"})
            self.assertEqual(result.returncode, 0)
            self.assertEqual(json.loads(result.stdout), {})
            self.assertEqual(result.stderr, b"")

    def test_invalid_arguments_do_not_touch_stdin(self):
        # Open pipe that never has EOF written; callback must exit immediately on arg check
        r_fd, w_fd = os.pipe()
        try:
            env = dict(os.environ)
            env.pop("MAGY_WATCH_ID", None)
            proc = subprocess.Popen(
                [sys.executable, str(CALLBACK)],
                stdin=r_fd,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                env=env,
            )
            # Must exit without waiting for stdin EOF
            proc.wait(timeout=5)
            stdout, stderr = proc.communicate(timeout=5)
            self.assertEqual(proc.returncode, 0)
            self.assertEqual(json.loads(stdout), {})
            self.assertEqual(stderr, b"")
        finally:
            os.close(r_fd)
            os.close(w_fd)

    def test_unsupported_event_or_missing_config_exits_zero(self):
        env = dict(os.environ)
        env.pop("MAGY_WATCH_ID", None)
        result = subprocess.run(
            [sys.executable, str(CALLBACK), "UnknownEvent", "/nonexistent.json"],
            input=b'{"hook_event_name":"UnknownEvent"}',
            capture_output=True,
            env=env,
            timeout=5,
        )
        self.assertEqual(result.returncode, 0)
        self.assertEqual(json.loads(result.stdout), {})

    def test_oversized_stdin_rejected_neutrally(self):
        raw = b"x" * (callback.LIMIT + 10)
        result = self.invoke("Stop", {"reporter": "/missing", "version": "0.162.1"}, raw)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(json.loads(result.stdout), {})

    def test_payload_whitelist_discards_text_and_private_fields(self):
        raw_payload = {
            "hook_event_name": "Stop",
            "session_id": "sess-123",
            "turn_id": "turn-456",
            "stop_hook_active": True,
            "last_assistant_message": "super secret response text",
            "prompt": "super secret prompt",
            "transcript": "/path/to/transcript",
            "model": "gpt-5",
            "provider": "openai",
            "tool_input": {"cmd": "rm -rf /"},
            "tool_response": "deleted",
            "error": "internal failure details",
        }
        result = callback.snapshot(raw_payload, "Stop", "0.162.1", 1000)
        self.assertEqual(result["schema_version"], 1)
        self.assertEqual(result["version"], "0.162.1")
        self.assertEqual(result["event"], "Stop")
        self.assertEqual(result["sample"], 1000)
        self.assertEqual(result["session_id"], "sess-123")
        self.assertEqual(result["turn_id"], "turn-456")
        self.assertTrue(result["stop_hook_active"])
        serialized = json.dumps(result)
        self.assertNotIn("secret", serialized)
        self.assertNotIn("prompt", serialized)
        self.assertNotIn("transcript", serialized)
        self.assertNotIn("tool_input", serialized)
        self.assertNotIn("tool_response", serialized)
        self.assertNotIn("internal failure", serialized)

    def test_session_start_source_forwarding(self):
        raw = {"hook_event_name": "SessionStart", "session_id": "sess-1", "source": "compact"}
        res = callback.snapshot(raw, "SessionStart", "0.162.1", 1000)
        self.assertEqual(res["source"], "compact")

        raw_default = {"hook_event_name": "SessionStart", "session_id": "sess-1"}
        with self.assertRaises(ValueError):
            callback.snapshot(raw_default, "SessionStart", "0.162.1", 1000)

        raw_invalid = {"hook_event_name": "SessionStart", "session_id": "sess-1", "source": "unknown"}
        with self.assertRaises(ValueError):
            callback.snapshot(raw_invalid, "SessionStart", "0.162.1", 1000)

    def test_control_character_or_oversized_ids_rejected(self):
        payload_control = {
            "hook_event_name": "Stop",
            "session_id": "sess\n123",
            "turn_id": "turn-1",
        }
        with self.assertRaises(ValueError):
            callback.snapshot(payload_control, "Stop", "0.162.1", 1000)

        payload_oversized = {
            "hook_event_name": "Stop",
            "session_id": "s" * 300,
            "turn_id": "turn-1",
        }
        with self.assertRaises(ValueError):
            callback.snapshot(payload_oversized, "Stop", "0.162.1", 1000)

    def test_mismatched_event_name_rejected(self):
        payload = {
            "hook_event_name": "SessionStart",
            "session_id": "sess-1",
        }
        with self.assertRaises(ValueError):
            callback.snapshot(payload, "Stop", "0.162.1", 1000)

    @patch("callback.subprocess.run")
    @patch.dict(os.environ, {"ZELLIJ_PANE_ID": "terminal_42"}, clear=True)
    def test_report_invokes_reporter_with_correct_arguments(self, mock_run):
        config = {
            "enabled": True,
            "reporter": "/bin/verij",
            "version": "0.162.1",
            "runtime": "/tmp/runtime",
            "states": "/tmp/states",
            "binary_dev": 12,
            "binary_ino": 34,
        }
        value = {
            "hook_event_name": "SessionStart",
            "session_id": "sess-abc",
            "source": "startup",
        }
        with patch.object(callback, "agent_parent", return_value=(999, 12345)):
            with patch("pathlib.Path.stat", side_effect=AssertionError("binary inode must not be pinned")):
                callback.report(config, value, "SessionStart", 5000)

        mock_run.assert_called_once()
        args, kwargs = mock_run.call_args
        cmd = args[0]
        self.assertEqual(cmd, ["/bin/verij", "agent", "codex", "--pane", "42", "--pid", "999", "--birth", "12345"])
        payload = json.loads(kwargs["input"].decode())
        self.assertEqual(payload["event"], "SessionStart")
        self.assertEqual(payload["version"], "0.162.1")
        self.assertEqual(payload["session_id"], "sess-abc")
        self.assertEqual(payload["source"], "startup")
        self.assertEqual(payload["sample"], 5000)
        self.assertEqual(kwargs["timeout"], 1.5)

    @patch("callback.subprocess.run", side_effect=subprocess.TimeoutExpired(cmd="verij", timeout=1.5))
    @patch.dict(os.environ, {"ZELLIJ_PANE_ID": "terminal_42"}, clear=True)
    def test_report_handles_subprocess_timeout_cleanly(self, mock_run):
        config = {
            "enabled": True,
            "reporter": "/bin/verij",
            "version": "0.162.1",
            "runtime": "/tmp/runtime",
            "states": "/tmp/states",
            "binary_dev": 12,
            "binary_ino": 34,
        }
        value = {"hook_event_name": "Stop", "session_id": "sess-abc"}
        mock_stat = MagicMock()
        mock_stat.st_dev = 12
        mock_stat.st_ino = 34

        with patch.object(callback, "agent_parent", return_value=(999, 12345)):
            with patch("pathlib.Path.stat", return_value=mock_stat):
                try:
                    callback.report(config, value, "Stop", 5000)
                except Exception:
                    self.fail("report raised exception on subprocess timeout")

    def test_process_chain_climbs_past_piped_app_server_child(self):
        # Child 200 is "codex app-server" with piped stdin (not a tty)
        # Parent 100 is "codex" with tty stdin/stdout
        def mock_readlink(path):
            p = str(path)
            if p == "/proc/200/exe":
                return "/bin/codex"
            if p == "/proc/200/fd/0":
                return "pipe:[12345]"  # Not a tty
            if p == "/proc/200/fd/1":
                return "pipe:[12346]"
            if p == "/proc/100/exe":
                return "/bin/codex (deleted)"  # Running old inode after upgrade
            if p == "/proc/100/fd/0":
                return "/dev/pts/3"
            if p == "/proc/100/fd/1":
                return "/dev/pts/3"
            raise FileNotFoundError(p)

        def mock_read_stat(pid):
            if pid == 200:
                # state R, ppid 100, pgrp 200, session 100, tty_nr 0, tpgid 0 ... starttime 8888
                return "200 (codex) S 100 200 100 0 0 " + "0 " * 13 + "8888"
            if pid == 100:
                # state S, ppid 1, pgrp 100, session 100, tty_nr 34816, tpgid 100 ... starttime 7777
                return "100 (codex) S 1 100 100 34816 100 " + "0 " * 13 + "7777"
            raise FileNotFoundError(f"/proc/{pid}/stat")

        def mock_read_cmdline(pid):
            if pid == 200:
                return b"codex\x00app-server\x00"
            if pid == 100:
                return b"codex\x00"
            raise FileNotFoundError(f"/proc/{pid}/cmdline")

        with patch("os.getppid", return_value=200):
            with patch("os.readlink", side_effect=mock_readlink):
                with patch.object(callback, "_read_stat", side_effect=mock_read_stat):
                    with patch.object(callback, "_read_cmdline", side_effect=mock_read_cmdline):
                        pid, birth = callback.agent_parent()
                        self.assertEqual(pid, 100)
                        self.assertEqual(birth, 7777)


    def test_read_cmdline_bounds(self):
        with tempfile.NamedTemporaryFile() as tf:
            tf.write(b"a" * 65537)
            tf.flush()
            with patch("builtins.open", return_value=open(tf.name, "rb")):
                with self.assertRaises(ValueError):
                    callback._read_cmdline(123)

if __name__ == "__main__":
    unittest.main()
