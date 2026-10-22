#!/usr/bin/env python3
"""Validate bounded lifecycle projections without granting monitoring support."""
import argparse
import json
from pathlib import Path

from marker_evidence import identity
from verify_evidence import keys, read_report, require

HERE = Path(__file__).resolve().parent
MESSAGE_TYPES = ("agent-switched", "model-switched", "location-switched", "user", "synthetic", "system",
                 "skill", "shell", "assistant", "compaction", "idle")


def validate(report, require_pass=True):
    from lifecycle_probe import ARTIFACTS, CHECKS, LABELS
    from smoke import sha
    keys(report, "schema target artifact_hashes evidence_kind production_admission connected_transport_qualified automatic_resume_qualified run")
    require(type(report["schema"]) is int and report["schema"] == 1, "lifecycle schema")
    require(report["target"] == read_report(HERE / "pin.json"), "target pin")
    keys(report["artifact_hashes"], " ".join(ARTIFACTS))
    require(all(report["artifact_hashes"][name] == sha(HERE / name) for name in ARTIFACTS), "stale artifact hashes")
    require(report["evidence_kind"] == "private-headless-lifecycle", "evidence kind")
    require(all(report[name] is False for name in ("production_admission", "connected_transport_qualified", "automatic_resume_qualified")),
            "headless evidence cannot qualify production, CLI transport or automatic resume")
    run = report["run"]
    keys(run, "status stage failure checks samples provider_requests clear_provider_requests cleanup")
    require(run["status"] in ("LIFECYCLE_PASS", "INCOMPLETE"), "status enum")
    require(run["stage"] in ("start", "revert", "shutdown", "complete"), "stage enum")
    require(run["failure"] in (None, "OSError", "RuntimeError", "ValueError", "KeyError", "TypeError", "TimeoutError", "SubprocessError"), "failure enum")
    require(type(run["cleanup"]) is bool, "cleanup boolean")
    require(type(run["clear_provider_requests"]) is int and 0 <= run["clear_provider_requests"] <= 100, "clear request bound")
    require(type(run["checks"]) is list and run["checks"] == list(CHECKS[:len(run["checks"])]), "check ordering")
    require(type(run["samples"]) is list and len(run["samples"]) <= len(LABELS), "sample bound")
    for index, sample in enumerate(run["samples"]):
        keys(sample, "label session outcome marker marker_outcome marker_count message_ids message_types revert active")
        require(sample["label"] == LABELS[index], "sample ordering")
        require(identity(sample["session"]), "unhashed session")
        require(sample["revert"] is None or identity(sample["revert"]), "unhashed revert")
        require(type(sample["active"]) is bool, "activity boolean")
        require(sample["outcome"] in (None, "succeeded", "failed", "interrupted") and
                sample["marker_outcome"] in (None, "succeeded", "failed", "interrupted"), "outcome enum")
        require(type(sample["marker_count"]) is int and 0 <= sample["marker_count"] <= 32, "marker bound")
        require(type(sample["message_ids"]) is list and len(sample["message_ids"]) <= 32 and
                all(identity(item) for item in sample["message_ids"]), "message identity bound")
        require(len(set(sample["message_ids"])) == len(sample["message_ids"]), "duplicate message")
        require(type(sample["message_types"]) is list and len(sample["message_types"]) == len(sample["message_ids"])
                and all(kind in MESSAGE_TYPES for kind in sample["message_types"]), "message types")
        require(sample["message_types"].count("idle") == sample["marker_count"], "marker count mismatch")
        if sample["marker_count"]:
            position = sample["message_types"].index("idle")
            require(identity(sample["marker"]) and sample["marker"] == sample["message_ids"][position]
                    and sample["marker_outcome"] is not None, "latest historical marker")
        else:
            require(sample["marker"] is None and sample["marker_outcome"] is None, "missing marker")
    keys(run["provider_requests"], "success failure blocked")
    require(all(type(value) is int and 0 <= value <= 100 for value in run["provider_requests"].values()), "provider count bound")
    passed = run["status"] == "LIFECYCLE_PASS"
    if passed:
        require(run["stage"] == "complete" and run["failure"] is None and run["checks"] == list(CHECKS), "incomplete checks")
        require(len(run["samples"]) == len(LABELS), "incomplete samples")
        before, staged, cleared, committed, replacement, active, shutdown, recovered = run["samples"]
        require(len({s["session"] for s in (before, staged, cleared, committed, replacement)}) == 1, "revert root changed")
        require(len({s["session"] for s in (active, shutdown, recovered)}) == 1 and active["session"] != before["session"], "shutdown namespace")
        require(before["marker_count"] == 2 and before["outcome"] == before["marker_outcome"] == "succeeded", "two completed turns")
        require(before["message_types"].count("user") == 2
                and staged["revert"] == before["message_ids"][before["message_types"].index("user")], "staged boundary")
        require(staged["message_ids"] == before["message_ids"] and staged["message_types"] == before["message_types"]
                and staged["marker"] == before["marker"], "stage changed history identity")
        require(cleared["marker_count"] == 3 and cleared["marker_outcome"] == cleared["outcome"] == "succeeded"
                and cleared["message_ids"][1:] == before["message_ids"] and cleared["message_types"][1:] == before["message_types"]
                and cleared["message_types"][0] == "idle" and cleared["marker"] not in before["message_ids"]
                and run["clear_provider_requests"] == 0, "clear wake marker without model execution")
        require(all(s["revert"] is None for s in (before, cleared, committed, replacement, active, shutdown, recovered)), "uncleared revert")
        require(committed["marker_count"] == 1 and committed["marker"] in before["message_ids"]
                and before["marker"] not in committed["message_ids"] and staged["revert"] not in committed["message_ids"], "commit retained reverted history")
        boundary = before["message_ids"].index(staged["revert"]) + 1
        require(committed["message_ids"] == before["message_ids"][boundary:]
                and committed["message_types"] == before["message_types"][boundary:], "commit rewrote older history")
        require(replacement["marker_count"] == 2 and replacement["outcome"] == replacement["marker_outcome"] == "succeeded"
                and replacement["marker"] not in before["message_ids"]
                and replacement["message_ids"][-len(committed["message_ids"]):] == committed["message_ids"]
                and replacement["message_types"][-len(committed["message_types"]):] == committed["message_types"],
                "replacement reused terminal identity or rewrote older history")
        require(active["active"] and active["message_types"][0] != "idle" and active["marker_count"] == 1
                and active["message_types"].count("user") == 2, "no open execution with historical success")
        require(all(s["outcome"] == s["marker_outcome"] == "succeeded" for s in (active, shutdown, recovered)),
                "historical success projection changed")
        require(not shutdown["active"] and not recovered["active"], "unregistered serve unexpectedly resumed")
        added = len(shutdown["message_ids"]) - len(active["message_ids"])
        require(added in (0, 1) and shutdown["message_ids"][added:] == active["message_ids"]
                and shutdown["message_types"][added:] == active["message_types"]
                and shutdown["message_types"][:added] in ([], ["assistant"])
                and shutdown["marker"] == active["marker"] and shutdown["marker_count"] == 1,
                "shutdown fabricated terminal marker or rewrote history")
        require(recovered == {**shutdown, "label": "after_restart"}, "restart replay changed identity")
        require(all(not s["active"] for s in (before, staged, cleared, committed, replacement)), "revert performed during execution")
        require(run["provider_requests"]["success"] >= 3 and run["provider_requests"]["blocked"] >= 2, "no fixture executions")
    if require_pass:
        require(passed and run["cleanup"], "incomplete run or cleanup")
    return {"status": run["status"], "checks": len(run["checks"]), "production_admission": False,
            "connected_transport_qualified": False, "automatic_resume_qualified": False, "p0_gate_passed": False}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    print(json.dumps(validate(read_report(parser.parse_args().report)), indent=2))
