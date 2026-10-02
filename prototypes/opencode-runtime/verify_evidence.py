#!/usr/bin/env python3
"""Validate retained native callback/HTTP snapshots without running models."""
import argparse
import json
from pathlib import Path


def terminal(snapshot, error=None):
    messages = [m for m in snapshot["sessions"][0]["messages"] if m["role"] == "assistant"]
    assert messages and messages[-1]["completed"]
    if error:
        assert messages[-1]["error_name"] == error and messages[-1]["finish"] is None
    else:
        assert messages[-1]["finish"] == "stop" and messages[-1]["error_name"] is None


def verify(path):
    report = json.loads(path.read_text())
    assert report["status"] == "PASS" and report["probe_processes_exited"] and report["server_and_host_exited"]
    checks = report["checks"]
    assert len(checks) == 9
    assert checks[0]["private_home_xdg_database_and_verified_listener"]
    assert report["artifacts_dir"] in checks[0]["database_path"]
    assert checks[1]["two_native_panes_foreground_tpgid_and_birth_verified"]
    assert all(t["birth"]["pgrp"] == t["birth"]["tpgid"] == t["pid"] for t in checks[1]["tuis"])
    terminal(checks[2]["snapshot"])
    terminal(checks[3]["snapshot"], "APIError")
    assert checks[4]["retry"]["status"] == "retry" and checks[4]["retry"]["retry_attempt"] > 0
    assert not any(m["completed"] and m["finish"] == "stop" for m in checks[4]["retry_snapshot"]["sessions"][0]["messages"])
    terminal(checks[5]["snapshot"], "MessageAbortedError")
    recovery = checks[6]
    assert recovery["provider_requests"] == report["provider_requests"]["recovery"] == 1
    assert recovery["before"]["sessions"][0]["status"] == recovery["recovered"]["sessions"][0]["status"] == "busy"
    assert recovery["before"]["pid"] != recovery["recovered"]["pid"]
    terminal(recovery["terminal"])
    family = checks[7]
    assert family["owned"]["sessions"][1]["parent_session"] == family["owned"]["sessions"][0]["session"]
    assert all(s["status"] == "busy" and s["in_family"] for s in family["owned"]["sessions"])
    assert all(not s["in_family"] for s in family["foreign"]["sessions"])
    for session in family["final"]["sessions"]:
        terminal({"sessions": [session]})
    assert any(m["finish"] == "tool-calls" for m in family["final"]["sessions"][0]["messages"])
    events = [json.loads(line) for line in (Path(report["artifacts_dir"]) / "tui-events.ndjson").read_text().splitlines()]
    for snapshot in (checks[2]["snapshot"], checks[3]["snapshot"], recovery["recovered"], family["owned"], family["foreign"]):
        assert snapshot in events, "reported snapshot missing from original callback stream"
    print(json.dumps({"report": str(path), "validated_checks": 9, "original_callback_records": len(events),
                      "provider_output_synthetic": True, "runtime_snapshots_genuine": True,
                      "h0_approved": False}, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    verify(parser.parse_args().report)
