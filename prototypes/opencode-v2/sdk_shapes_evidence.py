#!/usr/bin/env python3
"""Strict checker for private SDK response-shape evidence; no CLI admission."""
import argparse
import json
from pathlib import Path
import re

from marker_evidence import identity
from verify_evidence import keys, read_report, require

HERE = Path(__file__).resolve().parent


def identities(values, count):
    return (type(values) is list and len(values) == count and all(identity(item) for item in values)
            and len(set(values)) == count)


def validate_observation(observation):
    from sdk_shapes import CHECKS
    keys(observation, "checks root root_outcome marker_outcome active children messages permissions forms inbox metrics preaborted_rejected global_fetch_unchanged production_admission connected_transport_qualified")
    require(observation["checks"] == list(CHECKS), "SDK shape checks")
    require(observation["root_outcome"] == observation["marker_outcome"] == "succeeded", "successful root/marker with pending work")
    require(observation["production_admission"] is False and observation["connected_transport_qualified"] is False, "no CLI provenance/admission")
    require(observation["global_fetch_unchanged"] is True and observation["preaborted_rejected"] is True, "cancellation/isolation facts")
    require(identity(observation["root"]) and identities(observation["active"], 1)
            and observation["root"] not in observation["active"], "root/active identity")
    children = observation["children"]
    keys(children, "ids pages")
    require(identities(children["ids"], 3) and type(children["pages"]) is int and children["pages"] == 4
            and observation["root"] not in children["ids"] and not set(children["ids"]) & set(observation["active"]), "child pagination")
    messages = observation["messages"]
    keys(messages, "ids types ascending_ids descending_pages ascending_pages")
    require(identities(messages["ids"], 3) and messages["types"] == ["idle", "assistant", "user"]
            and messages["ascending_ids"] == messages["ids"][::-1], "message traversal identity")
    require(type(messages["descending_pages"]) is int and type(messages["ascending_pages"]) is int
            and messages["descending_pages"] == messages["ascending_pages"] == 4, "message page counts")
    for field in ("permissions", "forms"):
        scoped = observation[field]
        keys(scoped, "location_a location_b a b session")
        require(identity(scoped["location_a"]) and identity(scoped["location_b"])
                and scoped["location_a"] != scoped["location_b"], "location identities")
        require(identities(scoped["a"], 1) and identities(scoped["b"], 1) and scoped["a"] != scoped["b"]
                and scoped["session"] == scoped["a"], "pending input scope/unwrap")
    require(observation["permissions"]["location_a"] == observation["forms"]["location_a"]
            and observation["permissions"]["location_b"] == observation["forms"]["location_b"], "location scope agreement")
    require(identities(observation["inbox"], 1), "pending inbox")
    metrics = observation["metrics"]
    keys(metrics, "invocations calls errors active pending")
    require(all(type(value) is int for value in metrics.values()) and metrics == {
        "invocations": 22, "calls": 21, "errors": 0, "active": 0, "pending": 0}, "physical request accounting")
    return observation


def validate(report, require_pass=True):
    from sdk_shapes import ARTIFACTS, SDK_SOURCES
    from smoke import sha
    keys(report, "schema target artifact_hashes sdk_sources bun_sha256 evidence_kind production_admission connected_transport_qualified run")
    require(type(report["schema"]) is int and report["schema"] == 1, "schema")
    require(report["target"] == read_report(HERE / "pin.json") and report["sdk_sources"] == SDK_SOURCES, "runtime/source pins")
    keys(report["artifact_hashes"], " ".join(ARTIFACTS))
    require(all(report["artifact_hashes"][name] == sha(HERE / name) for name in ARTIFACTS), "stale artifact hashes")
    require(type(report["bun_sha256"]) is str and re.fullmatch(r"[0-9a-f]{64}", report["bun_sha256"]), "Bun provenance")
    require(report["evidence_kind"] == "private-tagged-sdk-runtime" and report["production_admission"] is False
            and report["connected_transport_qualified"] is False, "private SDK cannot qualify CLI transport/support")
    run = report["run"]
    keys(run, "status stage failure fixture observation inputs_unchanged provider_requests cleanup")
    require(run["status"] in ("SHAPES_PASS", "INCOMPLETE"), "status enum")
    require(run["stage"] in ("start", "seed", "observe", "verify_read_only", "complete"), "stage enum")
    require(run["failure"] in (None, "OSError", "RuntimeError", "ValueError", "KeyError", "TypeError", "TimeoutError", "SubprocessError"), "failure enum")
    require(type(run["cleanup"]) is bool and type(run["inputs_unchanged"]) is bool, "result booleans")
    keys(run["provider_requests"], "success failure blocked")
    require(all(type(value) is int and 0 <= value <= 100 for value in run["provider_requests"].values()), "provider count bound")
    fixture = run["fixture"]
    if fixture is not None:
        keys(fixture, "root active children permission_a permission_b form_a form_b pending location_a location_b")
        require(all(identity(value) for name, value in fixture.items() if name != "children")
                and identities(fixture["children"], 3), "fixture identity projection")
    if run["observation"] is not None:
        observation = validate_observation(run["observation"])
        require(fixture is not None and observation["root"] == fixture["root"] and observation["active"] == [fixture["active"]]
                and set(observation["children"]["ids"]) == set(fixture["children"])
                and observation["inbox"] == [fixture["pending"]], "fixture/SDK identity mismatch")
        for name, prefix in (("permissions", "permission"), ("forms", "form")):
            require(observation[name]["a"] == [fixture[prefix + "_a"]] and observation[name]["b"] == [fixture[prefix + "_b"]]
                    and observation[name]["location_a"] == fixture["location_a"]
                    and observation[name]["location_b"] == fixture["location_b"], "fixture/SDK pending scope mismatch")
    passed = run["status"] == "SHAPES_PASS"
    if passed:
        require(run["stage"] == "complete" and run["failure"] is None and run["observation"] is not None
                and run["inputs_unchanged"], "incomplete observations/read-only control")
        require(run["provider_requests"] == {"success": 1, "failure": 0, "blocked": 1}, "fixture execution control")
    if require_pass:
        require(passed and run["cleanup"], "incomplete run or cleanup")
    return {"status": run["status"], "checks": len(run["observation"]["checks"]) if run["observation"] else 0,
            "production_admission": False, "connected_transport_qualified": False, "p0_gate_passed": False}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    print(json.dumps(validate(read_report(parser.parse_args().report)), indent=2))
