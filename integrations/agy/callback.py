#!/usr/bin/env python3
"""Bounded metadata reporting; preserve existing status-line and neutral hooks."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import tempfile
import shutil

LIMIT = 262144


def agent_parent():
    pid = os.getppid()
    for _ in range(128):
        stat = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        executable = Path(os.readlink(f"/proc/{pid}/exe")).name
        if executable == "agy" or executable.startswith("agy."):
            return pid, int(stat[19])
        pid = int(stat[1])
        if pid <= 1:
            break
    raise ValueError("no pane-local Agy ancestor")


def snapshot(value, event, version, sample):
    result = {"schema_version": 1, "version": version, "event": event, "sample": sample}
    fields = (("conversation_id", "agent_state", "tool_confirmation_pending", "task_count")
              if event == "ui" else
              ("conversationId", "invocationNum", "initialNumSteps", "executionNum", "fullyIdle", "terminationReason"))
    result.update({key: value[key] for key in fields if key in value})
    # Never pass provider error text, commands, transcript paths, quota or identity data.
    if event == "Stop":
        result["error_present"] = bool(value.get("error"))
    return result


def report(config, value, event, sample):
    if config.get("enabled") is False:
        return
    # Pane-less runs and watches use the separate runner observer, never inherited UI ownership.
    if os.environ.get("MAGY_WATCH_ID"):
        return
    pane = os.environ.get("ZELLIJ_PANE_ID", "").removeprefix("terminal_")
    if not pane.isdigit():
        return
    pid, birth = agent_parent()
    executable = Path(f"/proc/{pid}/exe").stat()
    if (executable.st_dev, executable.st_ino) != (config["binary_dev"], config["binary_ino"]):
        return  # An upgraded binary requires version-qualified setup again.
    payload = snapshot(value, event, value.get("version") or config["version"], sample)
    env = dict(os.environ)
    env.setdefault("VERIJ_AGENT_STATE_DIR", config["runtime"])
    env.setdefault("VERIJ_STATES_DIR", config["states"])
    subprocess.run([config["reporter"], "agent", "agy", "--pane", pane,
                    "--pid", str(pid), "--birth", str(birth)],
                   input=json.dumps(payload).encode(), stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL, env=env, timeout=1.5, check=False)


def main():
    event, file = sys.argv[1:3]
    sample = time.monotonic_ns()
    raw = sys.stdin.buffer.read(LIMIT + 1)
    config = {}
    try:
        config = json.loads(Path(file).read_text())
        if len(raw) <= LIMIT:
            report(config, json.loads(raw), event, sample)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError):
        pass  # Observation must never gate execution.
    if event == "ui":
        previous = config.get("previous_command")
        if previous:
            # Agy invokes status commands through its shell. Preserve cwd/env,
            # original stdin and visible stdout/stderr, including nonzero exit.
            if len(raw) <= LIMIT:
                return subprocess.run(previous, shell=True, input=raw).returncode
            # Oversized observations are ignored; predecessor still receives
            # its complete original stdin without unbounded memory allocation.
            with tempfile.TemporaryFile() as spool:
                spool.write(raw)
                shutil.copyfileobj(sys.stdin.buffer, spool)
                spool.seek(0)
                return subprocess.run(previous, shell=True, stdin=spool).returncode
        return 0
    print(json.dumps({"decision": ""} if event == "Stop" else {}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
