#!/usr/bin/env python3
"""Strict independent allowlist checker for private event/marker reconciliation."""
import argparse
import json
from pathlib import Path

from marker_evidence import identity
from smoke import HERE, sha
from terminal_evidence import checked_frames, validate_layout
from verify_evidence import keys, read_report, require


def validate_sample(sample):
    keys(sample, "label session outcome marker marker_outcome error_type has_interruption_reason active")
    require(sample["label"] in ("terminal", "repeated", "restart", "missed", "missed_repeated"), "sample label")
    require(identity(sample["session"]) and identity(sample["marker"]), "unhashed recovery identity")
    require(sample["outcome"] in ("failed", "interrupted") and sample["marker_outcome"] in ("failed", "interrupted"), "recovered outcome")
    require(sample["error_type"] in (None, "provider.invalid-request"), "recovered error enum")
    require(type(sample["active"]) is bool and type(sample["has_interruption_reason"]) is bool, "sample booleans")


def validate(report, require_pass=True):
    from reconciliation_contract import ARTIFACTS, CASES, CHECKS, LABELS
    keys(report, "schema target artifact_hashes status production_admission connected_transport_qualified runs")
    require(type(report["schema"]) is int and report["schema"] == 1, "schema")
    require(report["target"] == read_report(HERE / "pin.json"), "target pin")
    keys(report["artifact_hashes"], " ".join(ARTIFACTS))
    require(all(report["artifact_hashes"][name] == sha(HERE / name) for name in ARTIFACTS), "stale artifact hashes")
    require(report["production_admission"] is False and report["connected_transport_qualified"] is False, "no admission")
    require(report["status"] in ("INCOMPLETE", "RECONCILIATION_PASS"), "status enum")
    require(type(report["runs"]) is list and len(report["runs"]) == len(CASES), "case matrix")
    passing = []
    for run, case in zip(report["runs"], CASES):
        keys(run, "mode case status failure frames client listener_owned layout cleanup stage checks provider_requests samples control_calls observer_unchanged")
        require(run["mode"] == "server" and run["case"] == case, "explicit server case order")
        require(run["status"] in ("INCOMPLETE", "RECONCILIATION_PASS"), "run status")
        require(run["stage"] in ("start", "execution", "interruption", "exit", "recovery", "missed", "complete"), "stage enum")
        require(run["failure"] in (None, "OSError", "RuntimeError", "ValueError", "KeyError", "TypeError", "TimeoutError", "SubprocessError"), "failure enum")
        require(type(run["cleanup"]) is bool and type(run["observer_unchanged"]) is bool, "cleanup/observer boolean")
        require(type(run["control_calls"]) is int and 0 <= run["control_calls"] <= 128, "read call bound")
        require(type(run["checks"]) is list and run["checks"] == list(CHECKS[:len(run["checks"])]), "check order")
        keys(run["provider_requests"], "success failure blocked")
        require(all(type(n) is int and 0 <= n <= 100 for n in run["provider_requests"].values()), "provider counts")
        validate_layout(run)
        require(type(run["samples"]) is list and len(run["samples"]) <= len(LABELS), "sample bound")
        for sample, label in zip(run["samples"], LABELS):
            validate_sample(sample)
            require(sample["label"] == label, "sample order")
        require(type(run["frames"]) is list and len(run["frames"]) <= 96, "frame bound")
        stripped = []
        for frame in run["frames"]:
            require(type(frame) is dict, "frame object")
            if frame.get("kind") == "event":
                require("marker_from_event" in frame, "missing derived marker")
                derived = frame["marker_from_event"]
                require(derived is None if frame["event"] == "session.execution.started" else identity(derived), "derived marker identity")
                frame = {k: v for k, v in frame.items() if k != "marker_from_event"}
            stripped.append(frame)
        incarnations = checked_frames(stripped, run["client"])
        passed = run["status"] == "RECONCILIATION_PASS"
        if passed:
            require(run["stage"] == "complete" and run["failure"] is None and run["checks"] == list(CHECKS), "incomplete checks")
            require(run["listener_owned"] is True and run["client"] is not None and
                    [item["birth"] for item in run["layout"] if item["role"] == "client"] == [run["client"]]
                    and any(item["role"] == "server" for item in run["layout"]), "private process layout")
            require(len(incarnations) == 1 and incarnations[0][-1]["kind"] == "cleanup"
                    and incarnations[0][0]["generation"] == 1 and incarnations[0][0]["version_matches"]
                    and incarnations[0][0]["options_match"] and not any(f["kind"] == "fault" for f in run["frames"]), "observer lifecycle")
            routes = [f for f in run["frames"] if f["kind"] == "route"]
            require(len(routes) == 2 and routes[0]["route"] == "home" and routes[1]["route"] == "session", "Home/root matrix")
            events = [f for f in run["frames"] if f["kind"] == "event"]
            require(len(events) == 4 and all(f["session"] == f["selected"] == routes[1]["selected"] for f in events), "selected event matrix")
            normalize = lambda channel: [{k: v for k, v in f.items() if k not in ("channel", "index")} for f in events if f["channel"] == channel]
            typed, envelope = normalize("typed"), normalize("envelope")
            terminal = "session.execution.failed" if case == "failure" else "session.execution.interrupted"
            require(typed == envelope and [f["event"] for f in typed] == ["session.execution.started", terminal], "typed/envelope mismatch")
            require(typed[0]["seq"] < typed[1]["seq"] and typed[0]["event_id"] != typed[1]["event_id"], "event order/identity")
            error, reason = ("provider.invalid-request", None) if case == "failure" else (None, "user")
            require((typed[-1]["error_type"], typed[-1]["interruption_reason"]) == (error, reason), "terminal metadata")
            require(len(run["samples"]) == len(LABELS) and run["observer_unchanged"] and 16 <= run["control_calls"] <= 128, "recovery coverage")
            first, repeated, restart, missed, again = run["samples"]
            require(first["session"] == typed[-1]["session"] and first["marker"] == typed[-1]["marker_from_event"], "event/marker mismatch")
            require(first["marker"] != typed[-1]["event_id"], "event ID is not marker ID")
            require(repeated == {**first, "label": "repeated"} and restart == {**first, "label": "restart"}, "stable recovery identity")
            require(missed["session"] == first["session"] and missed["marker"] != first["marker"]
                    and again == {**missed, "label": "missed_repeated"}, "missed identity")
            expected = "failed" if case == "failure" else "interrupted"
            require(all(s["outcome"] == s["marker_outcome"] == expected and s["error_type"] == error and not s["has_interruption_reason"] and not s["active"] for s in run["samples"]), "recovered terminal semantics")
            counts = run["provider_requests"]
            require(counts["success"] == 0 and (2 <= counts["failure"] <= 6 and counts["blocked"] == 0 if case == "failure"
                    else counts["failure"] == 0 and 2 <= counts["blocked"] <= 6), "fixture execution control")
        passing.append(passed and run["cleanup"])
    require((report["status"] == "RECONCILIATION_PASS") == all(passing), "aggregate status")
    if require_pass:
        require(all(passing), "incomplete execution or cleanup")
    return {"status": report["status"], "runs": len(CASES), "checks_per_run": len(CHECKS),
            "production_admission": False, "connected_transport_qualified": False, "p0_gate_passed": False}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    print(json.dumps(validate(read_report(parser.parse_args().report)), indent=2))
