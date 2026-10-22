#!/usr/bin/env python3
"""Bounded, strict allowlist checker for P0 loading evidence, never support admission."""
import argparse
import json
from pathlib import Path
import re

HERE = Path(__file__).resolve().parent
SHAPES = {"session_get", "session_active", "session_list", "message_list", "permission_requests",
          "session_forms", "session_inbox", "typed_events", "enveloped_events", "family_cache"}
BLOCKERS = ["bounded_transport", "terminal_identity", "fresh_complete_family"]


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def keys(value, expected):
    require(type(value) is dict and set(value) == set(expected.split()), "unapproved or missing fields")


def bounded_json(raw, maximum=128 * 1024):
    require(len(raw) <= maximum, "evidence byte bound")
    # Bound depth and structural tokens before JSON allocation. Over-counting is
    # deliberate: separators/container starts conservatively bound node count.
    depth, nodes, quoted, escaped = 0, 1, False, False
    for byte in raw:
        if quoted:
            if escaped:
                escaped = False
            elif byte == 92:
                escaped = True
            elif byte == 34:
                quoted = False
        elif byte == 34:
            quoted = True
        elif byte in (91, 123):
            depth += 1
            nodes += 1
            require(depth <= 32, "evidence depth bound")
        elif byte in (93, 125):
            depth -= 1
        elif byte in (44, 58):
            nodes += 1
        require(nodes <= 16384, "evidence structural bound")
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, "duplicate evidence field")
            result[key] = value
        return result
    return json.loads(raw, object_pairs_hook=unique)


def read_report(path):
    with path.open("rb") as stream:
        return bounded_json(stream.read(128 * 1024 + 1))


def validate_frames(frames, client, passed):
    require(type(frames) is list and len(frames) <= 64, "frame bound")
    for index, frame in enumerate(frames):
        require(type(frame) is dict, "frame shape")
        kind = frame.get("kind")
        require(kind in ("setup", "route", "cleanup"), "frame kind")
        fields = {"setup": "version_matches options_match shape", "route": "route selected", "cleanup": ""}[kind]
        keys(frame, "kind index pid " + fields)
        require(type(frame["index"]) is int and frame["index"] == index, "frame ordering")
        require(type(frame["pid"]) is int and 0 < frame["pid"] <= 2**31 - 1, "frame pid")
        require(len(json.dumps(frame).encode()) <= 2048, "frame byte bound")
        if client:
            require(frame["pid"] == client["pid"], "client frame identity")
        if kind == "setup":
            keys(frame["shape"], " ".join(SHAPES))
            require(all(type(value) is bool for value in frame["shape"].values()), "shape booleans")
            require(type(frame["version_matches"]) is bool and type(frame["options_match"]) is bool, "setup booleans")
        elif kind == "route":
            require(frame["route"] in ("home", "session", "plugin", "unknown"), "route enum")
            require(frame["selected"] is None or (type(frame["selected"]) is str and
                    re.fullmatch(r"[0-9a-f]{20}", frame["selected"])), "unhashed session identity")
            require(frame["route"] == "session" or frame["selected"] is None, "route authority")
    if passed:
        require([frame["kind"] for frame in frames] == ["setup", "route", "cleanup"], "loading lifecycle")
        require(frames[0]["version_matches"] and frames[0]["options_match"] and all(frames[0]["shape"].values()), "contract shape")
        require(frames[1]["route"] == "home" and frames[1]["selected"] is None, "Home expected before prompt")


def validate(report, require_pass=True):
    keys(report, "schema target artifact_hashes status production_admission blockers runs")
    require(type(report["schema"]) is int and report["schema"] == 1, "evidence schema")
    require(report["production_admission"] is False, "P0 cannot admit production support")
    require(report["blockers"] == BLOCKERS, "P0 blockers must remain explicit")
    pin = read_report(HERE / "pin.json")
    require(report["target"] == pin, "target pin mismatch")
    from smoke import ARTIFACTS, sha
    keys(report["artifact_hashes"], " ".join(ARTIFACTS))
    require(all(report["artifact_hashes"][name] == sha(HERE / name) for name in ARTIFACTS), "stale fixture hashes")
    require(report["status"] in ("SMOKE_PASS", "INCOMPLETE"), "report status")
    require(type(report["runs"]) is list and len(report["runs"]) == 2, "mode matrix")
    require([run.get("mode") for run in report["runs"]] == ["standalone", "server"], "mode matrix")
    passing = []
    for run in report["runs"]:
        keys(run, "mode status failure frames client listener_owned layout cleanup")
        require(run["status"] in ("SMOKE_PASS", "INCOMPLETE"), "run status")
        require(run["failure"] in (None, "OSError", "ValueError", "RuntimeError", "TimeoutError",
                "PermissionError", "FileNotFoundError", "ProcessLookupError", "SubprocessError", "TimeoutExpired"), "failure enum")
        require(type(run["cleanup"]) is bool, "cleanup boolean")
        require(run["listener_owned"] is None or run["listener_owned"] is True, "listener evidence")
        client = run["client"]
        if client is not None:
            keys(client, "pid ppid pgrp session tty tpgid start")
            require(all(type(value) is int and 0 < value <= 2**63 - 1 for value in client.values()), "birth numbers")
            require(client["pid"] == client["pgrp"] == client["session"] == client["tpgid"], "foreground TTY ownership")
        require(type(run["layout"]) is list and len(run["layout"]) <= 256, "process layout bound")
        seen = set()
        for process in run["layout"]:
            keys(process, "role birth")
            require(process["role"] in ("client", "server", "helper"), "process role")
            birth = process["birth"]
            keys(birth, "pid ppid pgrp session tty tpgid start")
            require(all(type(value) is int and -1 <= value <= 2**63 - 1 for value in birth.values()), "layout numbers")
            require(all(birth[key] > 0 for key in ("pid", "ppid", "pgrp", "session", "start")), "layout birth")
            require(birth["pid"] not in seen, "duplicate process")
            seen.add(birth["pid"])
        passed = run["status"] == "SMOKE_PASS"
        validate_frames(run["frames"], client, passed)
        if passed:
            require(client is not None and run["failure"] is None, "missing live client evidence")
            require([process["birth"] for process in run["layout"] if process["role"] == "client"] == [client], "client process layout")
            require(any(process["role"] == "server" for process in run["layout"]), "private server process layout")
            require(run["listener_owned"] is (True if run["mode"] == "server" else None), "private listener")
        passing.append(passed and run["cleanup"])
    require((report["status"] == "SMOKE_PASS") == all(passing), "inconsistent aggregate status")
    if require_pass:
        require(all(passing), "incomplete smoke or cleanup")
    return {"status": report["status"], "modes": 2, "production_admission": False, "p0_gate_passed": False}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    print(json.dumps(validate(read_report(parser.parse_args().report)), indent=2))
