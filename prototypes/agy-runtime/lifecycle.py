#!/usr/bin/env python3
"""Observe non-gating Agy lifecycle hooks; preserve default execution/stop behavior."""
import hashlib
import json
import os
from pathlib import Path
import sys
import time


def main():
    phase = sys.argv[1]
    value = json.load(sys.stdin)
    record = {"time_ns": time.time_ns(), "phase": phase, "keys": sorted(value),
              "callback_pid": os.getpid(), "error_present": bool(value.get("error"))}
    for name in ("invocationNum", "initialNumSteps", "executionNum", "terminationReason", "fullyIdle", "stepIdx"):
        if name in value:
            record[name] = value[name]
    if value.get("conversationId"):
        record["conversation"] = hashlib.sha256(value["conversationId"].encode()).hexdigest()[:20]
    if phase == "PreToolUse":
        call = value.get("toolCall", {})
        arguments = call.get("args", {})
        record["tool_name"] = call.get("name")
        command = arguments.get("CommandLine")
        record["exact_permission_probe_command"] = command == "printf VJ_PERMISSION"
        record["exact_background_probe_command"] = command == "sleep 8; printf VJ_BACKGROUND"
        if "WaitMsBeforeAsync" in arguments:
            record["wait_ms_before_async"] = arguments["WaitMsBeforeAsync"]
    with Path(os.environ["VJ_LIFECYCLE_EVENTS"]).open("a") as output:
        output.write(json.dumps(record) + "\n")
    # Optional PreToolUse experiment emits empty JSON, never allow/ask/deny,
    # overrides or argument changes. Runtime neutrality must be verified.
    print(json.dumps({"decision": ""} if phase == "Stop" else {}))


if __name__ == "__main__":
    main()
