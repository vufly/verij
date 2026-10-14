#!/usr/bin/env python3
"""Matched private Pi workloads: master control, rebased control, rebased monitoring.

Controls retain the same real Pi, providers, exporter, two sidebars and fixture.
Only the owned Pi monitoring extension/reporter is absent. No global installation.
Reports contain hashes, bounded counters and role-separated resources, not content.
"""
import argparse
from collections import Counter
import json
from pathlib import Path
import shutil
import sys
import time

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]

PHASES = {"idle", "tool_burst", "recovery"}
CELLS = {"master_control", "rebased_control", "rebased_monitored"}


def qualify(report):
    assert report["complete"] and len(report["runs"]) == 3
    assert {r["cell"] for r in report["runs"]} == CELLS
    for run in report["runs"]:
        assert run["passed"] and run["version"] == "1.2.0"
        assert run["monitoring"] == (run["cell"] == "rebased_monitored")
        assert all(run["cleanup"][key] for key in (
            "private_processes_exited", "provider_stopped", "burst_provider_stopped", "private_directory_removed"))
        assert run["cleanup"]["forced_kills"] == 0
        assert run["nested_tools"] == 160 and run["event_counts"]["tool_execution_start"] == 161
        assert run["event_counts"]["tool_execution_end"] == 161 and run["event_counts"]["agent_settled"] == 1
        phases = {m["phase"]: m for m in run["measurements"]}
        assert set(phases) == PHASES
        for phase in phases.values():
            assert phase["seconds"] >= 6
            assert all(p["peak_rss_mib"] <= 768 for p in phase["processes"])
            roles = Counter(p["role"] for p in phase["processes"])
            assert roles["pi"] == 1 and roles["sidebar"] == 2
            assert roles["reporter"] == int(run["monitoring"])
            assert phase["counters"].get("proof_attempts", 0) == 0
        if run["monitoring"]:
            for name in ("idle", "recovery"):
                phase = phases[name]
                assert 0 < phase["counters"].get("native_record_writes", 0) <= phase["seconds"] / 5 + 2
            assert run["completion_revision"] == 1
            assert 0 < phases["tool_burst"]["counters"].get("native_record_writes", 0) <= 10 * phases["tool_burst"]["seconds"] + 4
        else:
            assert all(m["counters"].get("native_record_writes", 0) == 0 for m in phases.values())
    return True


def experiment(cli, plugin, command, seconds, cell, monitoring):
    from runtime import Runtime, sha

    runtime = Runtime(cli, plugin, command, seconds)
    runtime.env["VERIJ_BIN"] = str(cli)
    runtime.report.update(cell=cell, monitoring=monitoring, nested_tools=160,
                          assets={"cli": sha(cli), "exporter": sha(plugin)}, passed=False)
    try:
        runtime.report.update(version=runtime.run([*command, "--version"]),
                              zellij_version=runtime.run([runtime.zellij, "--version"]),
                              zellij_sha256=sha(runtime.zellij))
        assert runtime.report["version"] == "1.2.0"
        assert runtime.report["zellij_version"] == "zellij 0.45.1"
        runtime.start(monitoring=monitoring)
        if monitoring:
            runtime.wait(lambda: runtime.state()["reduced"]["status"] == "idle")
        else:
            assert not runtime.states()
        runtime.hosts()
        time.sleep(3)  # Settle startup/title work and one-second metric publication.
        runtime.measure("idle")
        offset = len(runtime.events())
        runtime.send("/p4 burst")
        runtime.measure("tool_burst", seconds=6)
        runtime.wait(lambda: (runtime.root / "burst-done-0").exists())
        runtime.wait(lambda: any(e["event"] == "agent_settled" for e in runtime.events()[offset:]))
        if monitoring:
            runtime.wait(lambda: runtime.state()["reduced"]["status"] == "done")
            runtime.report["completion_revision"] = runtime.state()["completion_revision"]
        else:
            assert not runtime.states()
        runtime.report["event_counts"] = dict(Counter(
            e["event"] for e in runtime.events()[offset:] if e["source"] == "observer"))
        runtime.measure("recovery")
        runtime.report["passed"] = True
    except Exception as error:
        runtime.report["failure"] = type(error).__name__
    finally:
        runtime.close()
    return runtime.report


def main():
    from runtime import sha

    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("master-cli", "master-plugin", "cli", "plugin"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--pi-bin", default="pi")
    parser.add_argument("--seconds", type=float, default=10)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not 10 <= args.seconds <= 60:
        parser.error("idle/recovery phases must be 10–60 seconds")
    command = [str(Path(shutil.which(args.pi_bin) or args.pi_bin).resolve(strict=True))]
    report = {"schema_version": 1, "complete": False,
              "scope": "One real compiled Pi, two hosts, no status bars; short matched experiment, not a soak",
              "controls": "Master and rebased controls omit only Pi monitoring; sidebars and exporters remain enabled",
              "resource_scope": "Pi role includes runtime and any in-process extension. Reporter role is native monitoring. "
                                "Sidebar/Zellij are shared infrastructure. Provider/harness CPU excluded. "
                                "Role totals and matched deltas are observations, not precise attribution of JS overhead.",
              "runtime_sha256": sha(Path(command[0])),
              "harness_assets": {str(p.relative_to(REPO)): sha(p) for p in (
                  Path(__file__).resolve(), HERE / "runtime.py", HERE / "fixture.ts", HERE / "launch.py",
                  HERE / "burst_provider.py", REPO / "prototypes/pi-runtime/provider.py",
                  REPO / "prototypes/pi-runtime/observer.ts", REPO / "prototypes/pi-runtime/trace.mjs")},
              "runs": []}
    for cell, cli, plugin, monitoring in (
        ("master_control", args.master_cli, args.master_plugin, False),
        ("rebased_control", args.cli, args.plugin, False),
        ("rebased_monitored", args.cli, args.plugin, True),
    ):
        run = experiment(cli.resolve(strict=True), plugin.resolve(strict=True), command, args.seconds, cell, monitoring)
        report["runs"].append(run)
        print(cell + ": " + ("PASS" if run["passed"] else "FAIL"), flush=True)
        if not run["passed"]:
            break
    report["complete"] = len(report["runs"]) == 3 and all(r["passed"] for r in report["runs"])
    try:
        qualify(report)
    except AssertionError:
        report["complete"] = False
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["complete"] else 1


if __name__ == "__main__":
    sys.exit(main())
