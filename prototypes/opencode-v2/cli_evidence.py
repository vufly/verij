#!/usr/bin/env python3
"""Strict bounded checker for private CLI event/reload evidence, not admission."""
import argparse
import json
from pathlib import Path

from marker_evidence import identity
from verify_evidence import keys, read_report, require

HERE = Path(__file__).resolve().parent
EVENTS = ("session.execution.started", "session.execution.succeeded", "session.execution.failed", "session.execution.interrupted")


def validate_frames(frames, passed, client=None):
    require(type(frames) is list and len(frames) <= 96, "frame bound")
    incarnations = []
    current = None
    for frame in frames:
        require(type(frame) is dict and frame.get("kind") in ("setup", "route", "event", "fault", "cleanup"), "frame kind")
        kind = frame["kind"]
        fields = {"setup": "version_matches options_match", "route": "route selected",
                  "event": "channel event session event_id aggregate seq version route selected", "fault": "reason", "cleanup": ""}[kind]
        keys(frame, "kind generation index pid " + fields)
        require(type(frame["pid"]) is int and 0 < frame["pid"] <= 2**31 - 1, "frame process identity")
        require(client is None or frame["pid"] == client["pid"], "frame/client mismatch")
        require(type(frame["generation"]) is int and frame["generation"] in (1, 2, 3), "generation enum")
        if kind == "setup":
            require(current is None or current[-1]["kind"] == "cleanup", "overlapping incarnations")
            current = []
            incarnations.append(current)
            require(type(frame["version_matches"]) is bool and type(frame["options_match"]) is bool, "setup booleans")
        require(current is not None and (not current or current[-1]["kind"] != "cleanup"), "frame after cleanup")
        require(type(frame["index"]) is int and frame["index"] == len(current), "incarnation ordering")
        require(not current or frame["generation"] == current[0]["generation"], "mixed generation")
        current.append(frame)
        if kind in ("route", "event"):
            require(frame["route"] in ("home", "session", "plugin", "unknown"), "route enum")
            require(frame["selected"] is None or identity(frame["selected"]), "unhashed selected session")
            require((frame["route"] == "session") == (frame["selected"] is not None), "route authority")
        if kind == "fault":
            require(frame["reason"] in ("schema", "frame_bound", "subscription"), "fault enum")
        if kind == "event":
            require(frame["channel"] in ("typed", "envelope") and frame["event"] in EVENTS, "event enum")
            require(all(identity(frame[name]) for name in ("session", "event_id", "aggregate")), "unhashed event identity")
            require(frame["aggregate"] == frame["session"], "event aggregate mismatch")
            require(type(frame["seq"]) is int and 0 <= frame["seq"] <= 2**53 - 1, "aggregate sequence")
            require(type(frame["version"]) is int and frame["version"] == 1, "durable version")
        require(len(json.dumps(frame).encode()) <= 2048, "frame byte bound")
    if passed:
        require(not any(frame["kind"] == "fault" for frame in frames), "observer fault")
        require([items[0]["generation"] for items in incarnations] == [1, 2, 2, 3], "reload matrix")
        require(all(items[-1]["kind"] == "cleanup" and items[0]["version_matches"] and items[0]["options_match"]
                    for items in incarnations), "setup/cleanup matrix")
        routes = [frame for frame in frames if frame["kind"] == "route"]
        require(routes[0]["route"] == "home" and routes[0]["selected"] is None, "initial Home")
        selected = {frame["selected"] for frame in routes if frame["selected"] is not None}
        require(len(selected) == 1 and all(frame["route"] == "session" for frame in routes[1:]), "selected route continuity")
        session = next(iter(selected))
        for index, items in enumerate(incarnations):
            events = [frame for frame in items if frame["kind"] == "event"]
            if index in (1, 2):
                require(not events, "execution leaked into reload/disabled interval")
                continue
            require(len(events) == 4 and all(frame["session"] == frame["selected"] == session for frame in events), "execution event matrix")
            typed = [{k: v for k, v in frame.items() if k not in ("channel", "index")} for frame in events if frame["channel"] == "typed"]
            envelope = [{k: v for k, v in frame.items() if k not in ("channel", "index")} for frame in events if frame["channel"] == "envelope"]
            require(typed == envelope and [frame["event"] for frame in typed] == ["session.execution.started", "session.execution.succeeded"], "typed/listen identity mismatch")
            require(typed[0]["seq"] < typed[1]["seq"] and typed[0]["event_id"] != typed[1]["event_id"], "aggregate event order")
        first = [f for f in incarnations[0] if f["kind"] == "event" and f["channel"] == "typed"]
        last = [f for f in incarnations[3] if f["kind"] == "event" and f["channel"] == "typed"]
        require(first[-1]["seq"] < last[0]["seq"] and not {f["event_id"] for f in first} & {f["event_id"] for f in last}, "event sequence/identity replay")
    return incarnations


def validate(report, require_pass=True):
    from cli_contract import ARTIFACTS, CHECKS
    from smoke import sha
    keys(report, "schema target artifact_hashes status production_admission connected_transport_qualified runs")
    require(type(report["schema"]) is int and report["schema"] == 1, "schema")
    require(report["target"] == read_report(HERE / "pin.json"), "target pin")
    keys(report["artifact_hashes"], " ".join(ARTIFACTS))
    require(all(report["artifact_hashes"][name] == sha(HERE / name) for name in ARTIFACTS), "stale artifact hashes")
    require(report["production_admission"] is False and report["connected_transport_qualified"] is False, "CLI fixture cannot admit transport/support")
    require(report["status"] in ("CLI_PASS", "INCOMPLETE"), "status enum")
    require(type(report["runs"]) is list and len(report["runs"]) == 2, "mode matrix")
    passing = []
    for index, run in enumerate(report["runs"]):
        keys(run, "mode status failure frames client listener_owned layout cleanup stage checks initial_provider_requests disabled_provider_requests provider_requests")
        require(run["mode"] == ("standalone", "server")[index], "mode order")
        require(run["status"] in ("CLI_PASS", "INCOMPLETE"), "run status")
        require(run["stage"] in ("start", "execution", "reload", "exit", "complete"), "stage enum")
        require(run["failure"] in (None, "OSError", "RuntimeError", "ValueError", "KeyError", "TypeError", "TimeoutError", "SubprocessError"), "failure enum")
        require(type(run["cleanup"]) is bool, "cleanup boolean")
        require(type(run["checks"]) is list and run["checks"] == list(CHECKS[:len(run["checks"])]), "check order")
        require(type(run["initial_provider_requests"]) is int and 0 <= run["initial_provider_requests"] <= 100, "initial request bound")
        require(type(run["disabled_provider_requests"]) is int and 0 <= run["disabled_provider_requests"] <= 100, "disabled request bound")
        keys(run["provider_requests"], "success failure blocked")
        require(all(type(count) is int and 0 <= count <= 100 for count in run["provider_requests"].values()), "provider count bound")
        require(run["listener_owned"] is None or run["listener_owned"] is True, "listener ownership")
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
        passed = run["status"] == "CLI_PASS"
        validate_frames(run["frames"], passed, client)
        if passed:
            require(run["stage"] == "complete" and run["failure"] is None and run["checks"] == list(CHECKS), "incomplete checks")
            require(client is not None and [item["birth"] for item in run["layout"] if item["role"] == "client"] == [client]
                    and any(item["role"] == "server" for item in run["layout"]), "private process layout")
            require(run["listener_owned"] is (True if run["mode"] == "server" else None), "private listener")
            require(1 <= run["initial_provider_requests"] <= 3
                    and run["provider_requests"] == {"success": run["initial_provider_requests"] + 2, "failure": 0, "blocked": 0}
                    and run["disabled_provider_requests"] == 1, "disabled execution control")
        passing.append(passed and run["cleanup"])
    require((report["status"] == "CLI_PASS") == all(passing), "aggregate status")
    if require_pass:
        require(all(passing), "incomplete run or cleanup")
    return {"status": report["status"], "modes": 2, "checks_per_mode": len(CHECKS),
            "production_admission": False, "connected_transport_qualified": False, "p0_gate_passed": False}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    print(json.dumps(validate(read_report(parser.parse_args().report)), indent=2))
