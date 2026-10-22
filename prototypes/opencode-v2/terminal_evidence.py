#!/usr/bin/env python3
"""Independent bounded allowlist checker for private terminal-event evidence."""
import argparse
import json
from pathlib import Path

from cli_evidence import validate_frames
from smoke import HERE, sha
from verify_evidence import keys, read_report, require

ERROR_TYPES = {"provider.rate-limit", "provider.auth", "provider.quota", "provider.content-filter", "provider.transport",
               "provider.internal", "provider.invalid-output", "provider.invalid-request", "provider.unsupported-operation",
               "provider.no-route", "provider.unknown", "provider.timeout", "permission.rejected", "tool.execution", "unknown", "other"}
REASONS = {"user", "shutdown", "superseded", "inactivity"}


def checked_frames(frames, client):
    require(type(frames) is list and len(frames) <= 96, "frame bound")
    stripped = []
    for frame in frames:
        require(type(frame) is dict, "frame object")
        if frame.get("kind") == "event":
            keys(frame, "kind generation index pid channel event session event_id aggregate seq version route selected error_type interruption_reason")
            error, reason = frame["error_type"], frame["interruption_reason"]
            require(error is None or (type(error) is str and error in ERROR_TYPES), "error type enum")
            require(reason is None or (type(reason) is str and reason in REASONS), "interruption reason enum")
            require((frame["event"] == "session.execution.failed") == (error is not None), "error/event mismatch")
            require((frame["event"] == "session.execution.interrupted") == (reason is not None), "reason/event mismatch")
            require(len(json.dumps(frame).encode()) <= 2048, "frame byte bound")
            frame = {k: v for k, v in frame.items() if k not in ("error_type", "interruption_reason")}
        stripped.append(frame)
    return validate_frames(stripped, False, client)


def validate_layout(run):
    client = run["client"]
    if client is not None:
        keys(client, "pid ppid pgrp session tty tpgid start")
        require(all(type(n) is int and 0 < n <= 2**63 - 1 for n in client.values()), "client birth")
        require(client["pid"] == client["pgrp"] == client["session"] == client["tpgid"], "foreground TTY")
    require(type(run["layout"]) is list and len(run["layout"]) <= 256, "layout bound")
    seen = set()
    for item in run["layout"]:
        keys(item, "role birth")
        require(item["role"] in ("client", "server", "helper"), "process role")
        keys(item["birth"], "pid ppid pgrp session tty tpgid start")
        require(all(type(n) is int and -1 <= n <= 2**63 - 1 for n in item["birth"].values()), "process birth")
        require(all(item["birth"][k] > 0 for k in ("pid", "ppid", "pgrp", "session", "start")), "process identity")
        require(item["birth"]["pid"] not in seen, "duplicate process")
        seen.add(item["birth"]["pid"])
    require(run["listener_owned"] is None or run["listener_owned"] is True, "listener ownership")


def validate(report, require_pass=True):
    from terminal_contract import ARTIFACTS, CASES, CHECKS
    keys(report, "schema target artifact_hashes status production_admission connected_transport_qualified runs")
    require(type(report["schema"]) is int and report["schema"] == 1, "schema")
    require(report["target"] == read_report(HERE / "pin.json"), "target pin")
    keys(report["artifact_hashes"], " ".join(ARTIFACTS))
    require(all(report["artifact_hashes"][name] == sha(HERE / name) for name in ARTIFACTS), "stale artifact hashes")
    require(report["production_admission"] is False and report["connected_transport_qualified"] is False, "no support or transport admission")
    require(report["status"] in ("TERMINAL_PASS", "INCOMPLETE"), "status enum")
    require(type(report["runs"]) is list and len(report["runs"]) == len(CASES), "case matrix")
    passing = []
    for run, case in zip(report["runs"], CASES):
        keys(run, "mode case status failure frames client listener_owned layout cleanup stage checks provider_requests")
        require((run["mode"], run["case"]) == case, "case order")
        require(run["status"] in ("TERMINAL_PASS", "INCOMPLETE"), "run status")
        require(run["stage"] in ("start", "execution", "interruption", "exit", "complete"), "stage enum")
        require(run["failure"] in (None, "OSError", "RuntimeError", "ValueError", "KeyError", "TypeError", "TimeoutError", "SubprocessError"), "failure enum")
        require(type(run["cleanup"]) is bool, "cleanup boolean")
        require(type(run["checks"]) is list and run["checks"] == list(CHECKS[:len(run["checks"])]), "check order")
        keys(run["provider_requests"], "success failure blocked")
        require(all(type(n) is int and 0 <= n <= 100 for n in run["provider_requests"].values()), "provider count bound")
        validate_layout(run)
        incarnations = checked_frames(run["frames"], run["client"])
        passed = run["status"] == "TERMINAL_PASS"
        if passed:
            require(run["stage"] == "complete" and run["failure"] is None and run["checks"] == list(CHECKS), "incomplete checks")
            require(run["client"] is not None and [item["birth"] for item in run["layout"] if item["role"] == "client"] == [run["client"]]
                    and any(item["role"] == "server" for item in run["layout"]), "private process layout")
            require(run["listener_owned"] is (True if run["mode"] == "server" else None), "private listener")
            require(len(incarnations) == 1 and incarnations[0][0]["generation"] == 1
                    and incarnations[0][-1]["kind"] == "cleanup"
                    and incarnations[0][0]["version_matches"] and incarnations[0][0]["options_match"], "observer lifecycle")
            require(not any(f["kind"] == "fault" for f in run["frames"]), "observer fault")
            routes = [f for f in run["frames"] if f["kind"] == "route"]
            require(len(routes) == 2 and routes[0]["route"] == "home" and routes[1]["route"] == "session", "Home/root matrix")
            events = [f for f in run["frames"] if f["kind"] == "event"]
            require(len(events) == 4 and all(f["session"] == f["selected"] == routes[1]["selected"] for f in events), "selected event matrix")
            normalize = lambda channel: [{k: v for k, v in f.items() if k not in ("channel", "index")}
                                         for f in events if f["channel"] == channel]
            typed, envelope = normalize("typed"), normalize("envelope")
            terminal = "session.execution.failed" if run["case"] == "failure" else "session.execution.interrupted"
            require(typed == envelope and [f["event"] for f in typed] == ["session.execution.started", terminal], "typed/listen identity mismatch")
            require(typed[0]["seq"] < typed[1]["seq"] and typed[0]["event_id"] != typed[1]["event_id"], "aggregate event order")
            require(typed[-1]["error_type"] == ("provider.invalid-request" if run["case"] == "failure" else None)
                    and typed[-1]["interruption_reason"] == ("user" if run["case"] == "user_abort" else None), "terminal metadata")
            counts = run["provider_requests"]
            require(counts["success"] == 0 and (1 <= counts["failure"] <= 3 and counts["blocked"] == 0
                    if run["case"] == "failure" else counts["failure"] == 0 and 1 <= counts["blocked"] <= 3), "provider fixture control")
        passing.append(passed and run["cleanup"])
    require((report["status"] == "TERMINAL_PASS") == all(passing), "aggregate status")
    if require_pass:
        require(all(passing), "incomplete run or cleanup")
    return {"status": report["status"], "runs": len(CASES), "checks_per_run": len(CHECKS),
            "production_admission": False, "connected_transport_qualified": False, "p0_gate_passed": False}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    print(json.dumps(validate(read_report(parser.parse_args().report)), indent=2))
