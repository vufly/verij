#!/usr/bin/env python3
"""Check sanitized headless marker evidence; never grant CLI/production support."""
import argparse
import json
from pathlib import Path
import re

from verify_evidence import keys, read_report, require

HERE = Path(__file__).resolve().parent
LABELS = ("success", "recovered", "next_turn", "failure", "after_restart", "fork", "queue", "steer", "abort")


def identity(value):
    return type(value) is str and re.fullmatch(r"[0-9a-f]{20}", value) is not None


def validate(report, require_pass=True):
    from marker_probe import ARTIFACTS, CHECKS, sha
    keys(report, "schema target artifact_hashes evidence_kind production_admission connected_transport_qualified node_sha256 run")
    require(type(report["schema"]) is int and report["schema"] == 1, "marker evidence schema")
    require(report["target"] == read_report(HERE / "pin.json"), "target pin")
    keys(report["artifact_hashes"], " ".join(ARTIFACTS))
    require(all(report["artifact_hashes"][name] == sha(HERE / name) for name in ARTIFACTS), "stale artifact hashes")
    require(type(report["node_sha256"]) is str and re.fullmatch(r"[0-9a-f]{64}", report["node_sha256"]), "node provenance")
    require(report["evidence_kind"] == "private-headless-runtime", "evidence kind")
    require(report["production_admission"] is False and report["connected_transport_qualified"] is False, "headless experiment cannot qualify CLI transport")
    run = report["run"]
    keys(run, "status stage failure checks samples provider_requests cleanup")
    require(run["status"] in ("CONTRACT_PASS", "INCOMPLETE"), "run status")
    require(run["stage"] in ("start", "markers", "pending_and_family", "restart_and_fork", "inbox_and_abort", "complete"), "stage enum")
    require(run["failure"] in (None, "RuntimeError", "ValueError", "OSError", "KeyError", "TypeError", "TimeoutError",
                              "FileNotFoundError", "PermissionError", "ProcessLookupError", "URLError", "TimeoutExpired", "SubprocessError"), "failure enum")
    require(type(run["cleanup"]) is bool, "cleanup boolean")
    require(type(run["checks"]) is list and run["checks"] == list(CHECKS[:len(run["checks"])]), "check ordering")
    require(type(run["samples"]) is list and len(run["samples"]) <= len(LABELS), "sample bound")
    for index, sample in enumerate(run["samples"]):
        keys(sample, "label session outcome marker marker_outcome marker_count message_ids message_types")
        require(sample["label"] == LABELS[index], "sample ordering")
        require(identity(sample["session"]), "unhashed session")
        require(sample["marker"] is None or identity(sample["marker"]), "unhashed marker")
        require(sample["outcome"] in (None, "succeeded", "failed", "interrupted") and
                sample["marker_outcome"] in (None, "succeeded", "failed", "interrupted"), "outcome enum")
        require(type(sample["marker_count"]) is int and 0 <= sample["marker_count"] <= 32, "marker count")
        require(type(sample["message_ids"]) is list and len(sample["message_ids"]) <= 32 and
                all(identity(item) for item in sample["message_ids"]), "message identity bound")
        require(type(sample["message_types"]) is list and len(sample["message_types"]) == len(sample["message_ids"])
                and all(kind in ("agent-switched", "model-switched", "location-switched", "user", "synthetic", "system",
                                 "skill", "shell", "assistant", "compaction", "idle") for kind in sample["message_types"]), "message type schema")
        require(sample["message_types"].count("idle") == sample["marker_count"], "marker count mismatch")
        require(len(set(sample["message_ids"])) == len(sample["message_ids"]), "duplicate message identity")
        require(sample["marker_count"] <= len(sample["message_ids"]), "marker count mismatch")
        require((sample["marker_count"] == 0) == (sample["marker"] is None) == (sample["marker_outcome"] is None),
                "missing marker identity/outcome")
        require(sample["marker"] is None or (sample["marker"] == sample["message_ids"][0] and sample["message_types"][0] == "idle"),
                "terminal marker must be newest history entry")
    keys(run["provider_requests"], "success failure blocked")
    require(all(type(value) is int and 0 <= value <= 100 for value in run["provider_requests"].values()), "request count bound")
    passed = run["status"] == "CONTRACT_PASS"
    if passed:
        require(run["failure"] is None and run["stage"] == "complete" and run["checks"] == list(CHECKS), "incomplete contract checks")
        require(len(run["samples"]) == len(LABELS), "incomplete sample matrix")
        first, recovered, second, failed, resumed, forked, queued, steered, aborted = run["samples"]
        require(all(sample["outcome"] == sample["marker_outcome"] == "succeeded" for sample in (first, recovered, second, resumed)), "successful root marker")
        require(first["marker_count"] == recovered["marker_count"] == 1, "first marker count")
        require(first["marker"] == recovered["marker"] and first["message_ids"] == recovered["message_ids"], "recovery identity changed")
        require(second["marker"] != first["marker"] and second["marker_count"] == resumed["marker_count"] == 2, "new turn identity")
        require(failed["outcome"] == failed["marker_outcome"] == "failed", "failure became success")
        require(resumed["marker"] == second["marker"] and resumed["message_ids"] == second["message_ids"], "restart reminted history")
        require(len({sample["session"] for sample in (first, recovered, second, resumed)}) == 1, "root session changed")
        require(failed["session"] != first["session"] and forked["session"] not in (first["session"], failed["session"]), "fork/foreign namespace")
        require(forked["outcome"] is None and forked["marker_outcome"] == "succeeded" and forked["marker_count"] == 2
                and not set(forked["message_ids"]) & set(second["message_ids"]), "fork historical markers cannot become Done")
        require(queued["outcome"] == queued["marker_outcome"] == steered["outcome"] == steered["marker_outcome"] == "succeeded",
                "input lifecycle outcome")
        require(queued["marker_count"] == steered["marker_count"] == 1
                and queued["message_types"].count("user") == steered["message_types"].count("user") == 2, "queued/steered turn boundaries")
        require(aborted["outcome"] == aborted["marker_outcome"] == "interrupted", "abort became Done")
        require(all(run["provider_requests"][name] > 0 for name in ("success", "failure", "blocked")), "no provider executions")
    if require_pass:
        require(passed and run["cleanup"], "incomplete contract run or cleanup")
    return {"status": run["status"], "checks": len(run["checks"]), "production_admission": False,
            "connected_transport_qualified": False, "p0_gate_passed": False}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    print(json.dumps(validate(read_report(parser.parse_args().report)), indent=2))
