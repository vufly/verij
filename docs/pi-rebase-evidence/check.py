#!/usr/bin/env python3
"""Recheck retained rebase gates, artifact/source hashes and the scoped P3 smoke."""
import hashlib
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(REPO / "prototypes/pi-acceptance"))
from compare import qualify as qualify_comparison
from verify_evidence import qualify as qualify_acceptance


def read(name):
    return json.loads((HERE / name).read_text())


def verify_hashes(assets):
    for name, expected in assets.items():
        path = REPO / name
        assert path.is_file(), name
        assert hashlib.sha256(path.read_bytes()).hexdigest() == expected, name


def main():
    acceptance = read("pi-acceptance.json")
    qualify_acceptance(acceptance)
    verify_hashes(acceptance["assets"])
    comparison = read("pi-resource-comparison.json")
    qualify_comparison(comparison)
    verify_hashes(comparison["harness_assets"])
    provenance = read("provenance.json")
    expected = {"cli": provenance["artifact_sha256"]["rebased_cli"],
                "exporter": provenance["artifact_sha256"]["rebased_exporter"]}
    for run in comparison["runs"]:
        if run["cell"].startswith("rebased"):
            assert run["assets"] == expected
        else:
            assert run["assets"] == {"cli": provenance["artifact_sha256"]["master_cli"],
                                     "exporter": provenance["artifact_sha256"]["master_exporter"]}
    smoke = read("p3-smoke.json")
    assert smoke["status"] == "PASS" and len(smoke["experiments"]) == 1
    experiment = smoke["experiments"][0]
    assert experiment["status"] == "PASS" and experiment["monitoring"] == "enabled"
    assert experiment["stock_version"] == "zellij 0.45.1"
    assert experiment["hosts"] == 1 and experiment["tabs"] == 4
    topology = experiment["topology"]["resource-shared"]
    assert topology["tabs"] == topology["terminal_panes"] == 4
    assert topology["status_bar_instances"] == 0 and topology["clients"] == 1
    assert experiment["topology_refresh"] == experiment["detachment"] == "PASS"
    assert experiment["cleanup"]["known_processes_exited"] and not experiment["cleanup"]["remaining_pids"]
    assert experiment["assets"]["cli"]["sha256"] == expected["cli"]
    assert experiment["assets"]["exporter_tested"]["sha256"] == expected["exporter"]
    phases = {m["phase"]: m for m in experiment["measurements"]}
    assert len(phases) == 5 and {"idle_agent", "completion_visit", "idle_detached"} <= phases.keys()
    assert phases["idle_agent"]["counters"].get("proof_attempts", 0) == 0
    assert phases["idle_detached"]["counters"].get("proof_attempts", 0) == 0
    assert sum(m["counters"].get("proof_attempts", 0) for m in phases.values()) == 1
    visit = experiment["completion_visit"]
    assert visit["completion_revisions"] and len(visit["host_acknowledgements"]) == 1
    for acknowledgements in visit["host_acknowledgements"].values():
        for instance, revision in visit["completion_revisions"].items():
            assert revision > 0 and acknowledgements[instance]["acknowledged_revision"] >= revision
    print("PASS: two-distribution acceptance, three matched Pi cells, P3 smoke, current source/artifact hashes")


if __name__ == "__main__":
    main()
