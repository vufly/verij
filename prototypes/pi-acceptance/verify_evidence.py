#!/usr/bin/env python3
"""Independently check bounded P4 evidence, scope, semantic edges and budgets."""
import argparse
from collections import Counter
import json
from pathlib import Path
import re

REQUIRED = {"startup", "late_sidebars", "two_hosts", "reconnect", "reload", "success", "parallel", "nested",
            "tool_error", "provider_error", "retry_success", "retry_exhausted", "cancel", "queued", "late_queued",
            "continuation", "auto_compact", "ui", "ui_nested", "postrun_error", "continuation_error", "manual_compact",
            "name", "input_visit", "tree", "new", "resume", "fork", "reporter_loss", "two_processes",
            "inherited_background", "pane_targeting", "non_tui", "burst_budget", "lifetimes"}
STATUSES = {"idle", "working", "needs_input", "done", "error", "unknown"}
SAMPLE_KEYS = {"pane", "instance", "status", "record_revision", "completion_revision", "source_epoch", "source_revision",
               "pending_count", "request_kinds", "observed_settlements"}
ERRORS = {"provider_error": "error", "retry_exhausted": "error", "cancel": "idle", "postrun_error": "unknown",
          "continuation_error": "unknown"}
RUNS = {"success", "parallel", "nested", "tool_error", "provider_error", "retry_success", "retry_exhausted", "cancel",
        "queued", "late_queued", "continuation", "auto_compact", "ui", "ui_nested", "postrun_error", "continuation_error"}


def qualify(report, require_both=True):
    assert set(report) == {"schema_version", "scope", "assets", "runs", "limitations", "complete"}
    assert report["schema_version"] == 1 and report["complete"] is True
    encoded = json.dumps(report)
    assert len(encoded) < 2 * 1024 * 1024
    assert not any(s in encoded for s in ("PRIVATE_", "/tmp/", "/home/", "terminal_on_failure", "failure_screens"))
    assert report["limitations"] and "stock Zellij" in report["scope"]
    assert all(re.fullmatch(r"[0-9a-f]{64}", digest) for digest in report["assets"].values())
    assert len(report["assets"]) >= 10
    assert {run["runtime"] for run in report["runs"]} == ({"compiled", "node"} if require_both else {"compiled"})
    summaries = []
    for run in report["runs"]:
        assert run["passed"] is True and run["no_global_installation"] is True
        assert run["pi_version"] == "1.2.0" and run["zellij_version"] == "zellij 0.45.1"
        assert re.fullmatch(r"[a-f0-9]{64}", run["executable_sha256"])
        assert re.fullmatch(r"[a-f0-9]{64}", run["zellij_sha256"])
        assert len(run["cases"]) <= 64 and len(run["measurements"]) <= 16
        assert all(run["cleanup"][key] is True for key in
                   ("private_processes_exited", "provider_stopped", "burst_provider_stopped", "private_directory_removed"))
        assert run["cleanup"]["forced_kills"] == 0
        assert not any("TOKEN" in key or "KEY" in key or "SECRET" in key for key in run["environment_allowlist"])
        names = Counter(case["case"] for case in run["cases"])
        assert set(names) == REQUIRED
        assert names["success"] >= 2 and names["parallel"] == 2
        by_name = {case["case"]: case["facts"] for case in run["cases"]}
        assert by_name["startup"]["row_count"] == 1 and by_name["startup"]["completion_revision"] == 0
        assert by_name["startup"]["real_exporter"] and by_name["startup"]["foreground_verified"]
        assert by_name["late_sidebars"]["doctor_loaded"]
        assert by_name["two_hosts"]["unvisited_peer_preserved"] and by_name["two_hosts"]["observed_revision_only"]
        assert by_name["reconnect"]["no_replayed_done"] and by_name["reconnect"]["same_completed_at"]
        assert 0 < by_name["reconnect"]["reconnect_seconds"] <= 10
        assert by_name["input_visit"]["visit_preserves_request"]
        assert by_name["reporter_loss"]["pi_kept_working"] and by_name["reporter_loss"]["live_row_unknown"]
        assert 14 <= by_name["reporter_loss"]["lease_degradation_seconds"] <= 25
        assert by_name["two_processes"]["synthetic_inventory"] is False
        assert len(set(by_name["two_processes"]["panes"])) == 2
        assert by_name["inherited_background"]["actual_pi_tui"] and by_name["inherited_background"]["foreground_binding_rejected"]
        assert by_name["inherited_background"]["no_extra_row"]
        assert all(by_name["pane_targeting"][key] is True for key in ("rename", "move", "floating", "stacked", "background_tab_activation"))
        assert set(by_name["non_tui"]["modes"]) == {"print", "json", "rpc", "help", "version", "export"}
        assert by_name["non_tui"]["no_rows"] and by_name["non_tui"]["no_protocol_contamination"]
        assert by_name["non_tui"]["no_reporter_invocations"] and by_name["non_tui"]["disabled_discovery_no_reporter"]
        assert all(by_name["lifetimes"][key] is True for key in ("kill_removed_row", "quit_removed_row",
                   "pane_close_removed_row", "same_pane_new_process_new_identity", "no_ack_transfer"))
        for case in run["cases"]:
            assert set(case) == {"case", "facts", "samples"}
            facts = case["facts"]
            for sample in case["samples"]:
                assert set(sample) == SAMPLE_KEYS
                assert sample["status"] in STATUSES and sample["pending_count"] in (0, 1)
                assert re.fullmatch(r"[a-f0-9]{20}", sample["instance"])
                assert set(sample["request_kinds"]) <= {"confirmation", "question", "ui"}
                assert (sample["pending_count"] > 0) == (sample["status"] == "needs_input")
            if case["case"] in RUNS:
                status = ERRORS.get(case["case"], "done")
                assert facts["expected_status"] == status and facts["no_premature_completion"] is True
                assert facts["completion_after"] == facts["completion_before"] + int(status == "done")
                assert facts["event_counts"]["agent_settled"] == facts["event_counts"]["before_agent_start"] == 1
                assert any(s["status"] == "working" for s in case["samples"])
                assert any(s["status"] == status for s in case["samples"])
                for sample in case["samples"]:
                    if sample["pane"] == facts["pane"] and sample["completion_revision"] > facts["completion_before"]:
                        assert sample["observed_settlements"] > facts["settlements_before"], "native Done preceded settlement"
                for request in facts["provider_requests"]:
                    assert set(request) == {"model", "attempt", "status", "summary"}
                    assert request["model"] in RUNS | {"bad_compaction"} and request["status"] in (200, 400, 503)
            if case["case"] in ("name", "tree", "new", "resume", "fork", "reload"):
                assert facts["stable_process_row"] and facts["no_historical_done"]
        assert by_name["ui"]["native_dialogs_observed_before_reply"] == 5
        assert by_name["ui_nested"]["event_counts"]["ui_prompt_start"] == by_name["ui_nested"]["event_counts"]["ui_prompt_end"] == 1
        phases = {m["phase"]: m for m in run["measurements"]}
        assert set(phases) == {"acknowledged_heartbeat", "idle", "two_panes_two_hosts_idle", "two_panes_tool_burst"}
        for phase in phases.values():
            assert 5 <= phase["seconds"] <= 61
            assert all(p["peak_rss_mib"] <= 768 for p in phase["processes"])
        for name in ("acknowledged_heartbeat", "idle", "two_panes_two_hosts_idle"):
            assert phases[name]["counters"].get("proof_attempts", 0) == 0
            assert all(key.endswith(":list-sessions") for key in phases[name]["counters"] if key.startswith("cli/"))
        idle, burst = phases["two_panes_two_hosts_idle"], phases["two_panes_tool_burst"]
        assert Counter(p["role"] for p in idle["processes"])["reporter"] == 2
        assert 0 < idle["counters"].get("native_record_writes", 0) <= 2 * (idle["seconds"] / 5 + 2)
        assert 0 < burst["counters"].get("native_record_writes", 0) <= 2 * (10 * burst["seconds"] + 4)
        assert by_name["burst_budget"]["nested_tools_per_pane"] == 160
        assert by_name["burst_budget"]["exactly_one_aggregate_completion"] is True
        assert len(by_name["burst_budget"]["event_counts_by_pane"]) == 2
        assert all(c["tool_execution_start"] == c["tool_execution_end"] == 161 and c["agent_settled"] == 1
                   for c in by_name["burst_budget"]["event_counts_by_pane"].values())
        summaries.append({"runtime": run["runtime"], "checks": len(run["cases"]),
                          "successful_aggregate_runs": sum(c["facts"].get("expected_status") == "done" for c in run["cases"]),
                          "idle_writes": idle["counters"].get("native_record_writes", 0),
                          "burst_writes": burst["counters"].get("native_record_writes", 0),
                          "max_rss_mib": max(p["peak_rss_mib"] for m in run["measurements"] for p in m["processes"]),
                          "private_cleanup": True})
    return {"qualified": True, "scope": "P4 local two-host acceptance within explicit limitations; no global deployment approval",
            "runs": summaries, "limitations": report["limitations"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("evidence", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    value = qualify(json.loads(args.evidence.read_text()))
    text = json.dumps(value, indent=2) + "\n"
    if args.output:
        args.output.write_text(text)
    print(text)


if __name__ == "__main__":
    main()
