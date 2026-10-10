#!/usr/bin/env python3
"""Corroborate saved H4 production records against original callback metadata."""
import argparse
import json
from pathlib import Path


def verify(report):
    root = Path(report["root"])
    assert report["status"] == "PASS"
    callbacks = [json.loads(line) for line in (root / "callbacks.ndjson").read_text().splitlines()]
    assert callbacks and all(e["neutral_stdout"] and e["callback_exit"] == 0 for e in callbacks)
    assert report["cleanup"]["private_auth_removed"] and report["cleanup"]["socket_context_removed"]
    assert not (root / "home").exists() and not (root / "profile-home").exists()
    checked = 0
    for check in report["checks"]:
        evidence = json.loads(Path(check["evidence_file"]).read_text())
        assert evidence == check["evidence"]
        record = evidence.get("record") if isinstance(evidence,dict) else None
        if isinstance(evidence,dict) and "identity" in evidence: record = evidence
        if not record: continue
        identity, state = record["identity"], record["state"]
        assert not identity["is_synthetic"]
        assert state["agent_instance_id"] == identity["agent_instance_id"]
        assert len(json.dumps(state)) <= 262144
        if identity.get("runner_id"):
            metadata = json.loads((root / f"watch-metadata-{identity['runner_id']}.json").read_text())
            watch = metadata["state"]
            assert identity["process"]["pid"] == watch["runner_pid"]
            assert identity["pane_key"]["terminal"] == int(watch["pane_id"].removeprefix("terminal_"))
            source = state["sources"]["magy-watch"]
            assert source["turn_id"] == identity["runner_id"]
            assert state["adapter_state"]["magy-watch"]["offset"] > 0
            assert source["conversation_id"] == state["adapter_state"]["magy-watch"]["conversation"]
            assert state["reduced"]["status"] == "working"
            cursor = state["adapter_state"]["magy-watch"]
            assert any(e["event"] == "init" and e["conversation_id"] == source["conversation_id"]
                       and e["end_offset"] <= cursor["offset"] for e in metadata["events"])
        else:
            owned = [e for e in callbacks if e.get("pid") == identity["process"]["pid"]
                     and e.get("birth") == identity["process"]["start_jiffies"]]
            assert owned
            assert all(int(e["pane"].removeprefix("terminal_")) == identity["pane_key"]["terminal"] for e in owned)
            status = state["reduced"]["status"]
            if status == "done":
                conversation = state["reduced"]["conversation_id"]
                assert any(e["event"] == "Stop" and e.get("conversationId") == conversation
                           and e.get("fullyIdle") is True and e.get("error_present") is False for e in owned)
                assert state["completion_revision"] > 0
            if status == "needs_input":
                assert any(e.get("tool_confirmation_pending") is True for e in owned)
        checked += 1
    names = {check["name"] for check in report["checks"]}
    assert {"background_tasks_delay_done","aggregate_background_stop_completes_once",
            "magy_pane_start_profile_home_shared_reporter","watch_bound_native_renderer_stream_working",
            "watch_final_state_and_auto_close_remove_row","cancel-watch_final_state_and_auto_close_remove_row",
            "pane_less_detached_run_never_registers_launcher","renamed_session_same_process_instance"} <= names
    return {"status":"PASS","checks":len(names),"corroborated_production_snapshots":checked,"root":str(root)}


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("report",type=Path)
    args = parser.parse_args()
    print(json.dumps(verify(json.loads(args.report.read_text())),indent=2))
