#!/usr/bin/env python3
"""Metadata-only Agy callback probe and controlled old-command composition."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time


def main():
    raw = sys.stdin.buffer.read()
    value = json.loads(raw)
    record = {"time_ns": time.time_ns(), "keys": sorted(value), "callback_pid": os.getpid()}
    for name in ("agent_state", "tool_confirmation_pending", "task_count", "pending_input_count", "version", "execution_mode"):
        if name in value:
            record[name] = value[name]
    if isinstance(value.get("sandbox"), dict):
        record["sandbox"] = {"enabled": bool(value["sandbox"].get("enabled"))}
    conversation = value.get("conversation_id")
    if conversation:
        record["conversation"] = hashlib.sha256(conversation.encode()).hexdigest()[:20]
    exit_code = 0
    if os.environ.get("VJ_STATUSLINE_WRAP") == "1":
        previous = subprocess.run([sys.executable, "-c", 'import sys; sys.stdin.buffer.read(); print("OLD_STATUS_MARKER", end="")'],
                                  input=raw, capture_output=True, timeout=3)
        record["prior_command_exit"] = previous.returncode
        record["prior_stdout_preserved"] = previous.stdout == b"OLD_STATUS_MARKER"
        sys.stdout.buffer.write(previous.stdout)
        sys.stderr.buffer.write(previous.stderr)
        exit_code = previous.returncode
    else:
        sys.stdout.write("PROBE_STATUS_MARKER")
    path = Path(os.environ["VJ_STATUSLINE_EVENTS"])
    with path.open("a") as output:
        output.write(json.dumps(record) + "\n")
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
