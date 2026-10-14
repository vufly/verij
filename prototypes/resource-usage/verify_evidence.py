#!/usr/bin/env python3
"""Validate retained P0 topology, isolation, counters, acknowledgements and cleanup."""
import argparse
import json
from pathlib import Path


def verify(report):
    experiments = report["experiments"]
    assert {e["monitoring"] for e in experiments} == {"enabled", "disabled"}
    assert len({e["root"] for e in experiments}) == 2
    controls = []
    for experiment in experiments:
        assert experiment["cleanup"]["known_processes_exited"]
        assert experiment["stock_version"] == "zellij 0.45.1"
        assert "stock_sha256" in experiment
        for asset in experiment["assets"].values():
            assert len(asset["sha256"]) == 64
        if experiment["status"] != "PASS":
            assert "RSS abort" in experiment["error"], experiment["error"]
            continue  # Retained guarded abort is valid evidence, not a passing workload.
        assert experiment["topology_refresh"] == experiment["detachment"] == "PASS"
        shared = experiment["topology"]["resource-shared"]
        assert shared["tabs"] == shared["terminal_panes"] == shared["status_bar_instances"] == experiment["tabs"]
        assert shared["clients"] == experiment["hosts"]
        controls.append({name: {k: v for k, v in topology.items() if k != "server_pid"}
                         for name, topology in experiment["topology"].items()})
        phases = {m["phase"] for m in experiment["measurements"]}
        assert {"idle_agent", "completion_visit", "idle_detached"} <= phases
        assert len(phases) == 5
        for measurement in experiment["measurements"]:
            assert measurement["elapsed_seconds"] > 0
            assert len(measurement["intervals"]) <= measurement["interval_capacity"] == 60
            counters = measurement["counters"]
            assert all(v >= 0 for v in counters.values())
            assert not counters.get("cli_overflow", 0)
            if experiment["monitoring"] == "disabled":
                assert not sum(v for k, v in counters.items() if k.startswith(("cli/", "api/")))
                assert not counters.get("native_record_writes", 0)
                assert not counters.get("proof_attempts", 0)
        if experiment["monitoring"] == "enabled":
            visit = experiment["completion_visit"]
            assert all(revision > 0 for revision in visit["completion_revisions"].values())
            assert len(visit["host_acknowledgements"]) == experiment["hosts"]
            for host in visit["host_acknowledgements"].values():
                for instance, revision in visit["completion_revisions"].items():
                    assert host[instance]["acknowledged_revision"] >= revision
            assert sum(m["counters"].get("proof_attempts", 0) for m in experiment["measurements"]) > 0
    if len(controls) == 2:
        assert controls[0] == controls[1], "matched topology changed"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reports", nargs="*", type=Path)
    parser.add_argument("--summary", action="store_true")
    parser.add_argument("--phase", help="Filter summary to one workload phase")
    parser.add_argument("--expect-rss-abort", action="store_true")
    args = parser.parse_args()
    files = args.reports or sorted((Path(__file__).resolve().parents[2] / "docs/resource-baselines").glob("*host*tab*.json"))
    rows = []
    for path in files:
        report = json.loads(path.read_text())
        if args.expect_rss_abort:
            assert report["status"] == "FAIL"
            for experiment in report["experiments"]:
                assert "RSS abort" in experiment["error"]
                assert experiment["rss_abort"]["rss_mib"] > experiment["rss_abort"]["limit_mib"]
                assert experiment["cleanup"]["known_processes_exited"]
            continue
        verify(report)
        for experiment in report["experiments"]:
            for m in experiment["measurements"]:
                elapsed = m["elapsed_seconds"]
                rows.append({"report": path.name, "monitoring": experiment["monitoring"], "phase": m["phase"],
                    "elapsed": elapsed, "status": experiment["status"], "total_cpu": m["total_cpu_percent"],
                    "server_cpu": round(sum(p["cpu_percent"] for p in m["processes"] if p["role"] in ("inner_server", "host_server")), 2),
                    "cli_per_sec": round(sum(v for k, v in m["counters"].items() if k.startswith("cli/")) / elapsed, 2),
                    "api_per_sec": round(sum(v for k, v in m["counters"].items() if k.startswith("api/")) / elapsed, 2),
                    "writes_per_sec": round(m["counters"].get("native_record_writes", 0) / elapsed, 2),
                    "proofs_per_sec": round(m["counters"].get("proof_attempts", 0) / elapsed, 2),
                    "events_per_sec": round(m["counters"].get("owned_events", 0) / elapsed, 2),
                    "rss_mib": m["total_rss_mib"]})
    if args.summary:
        print(json.dumps([r for r in rows if args.phase is None or r["phase"] == args.phase], indent=2))
    else:
        print(json.dumps({"status": "PASS", "retained_reports": len(files), "workload_intervals": len(rows)}))


if __name__ == "__main__":
    main()
