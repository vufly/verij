#!/usr/bin/env python3
"""Corroborate H2 UI/status claims against retained private fixture projections."""
import argparse
import json
import statistics
from pathlib import Path


CHECKS = {
    "production_three_level_tree_all_statuses_and_synthetic_labels",
    "cursor_highlight_and_sidebar_focus_leave_done_for_both_hosts",
    "agent_enter_native_keyboard_transfer_clears_done_only_in_this_host",
    "agent_leaf_space_folds_parent_tab_without_other_tab",
    "mouse_agent_activation_acknowledges_other_host_independently",
    "collapsed_summary_and_host_ack_selection_folds_survive_sidebar_restart",
    "native_navigation_visit_auto_acknowledges_current_host_only",
    "visiting_needs_input_does_not_resolve_permission_request",
    "narrow_resize_preserves_status_and_fold_structure",
    "pane_closure_removes_agent_and_retains_parent_with_valid_selection",
}


def verify(report_path):
    report=json.loads(report_path.read_text())
    assert report["status"]=="PASS"
    assert report["semantic_records_synthetic"] is True
    assert report["human_h2_approved"] is False
    expected_checks = CHECKS | ({"workspace_title_startup_contention_retries_without_persistent_error"}
                               if report.get("startup_contention") else set())
    if report.get("legacy_source_inventory_absent"):
        expected_checks.add("missing_source_exporter_bootstraps_verified_inventory_before_agent_switch")
        expected_checks.add("repeated_same_session_agent_activation_transfers_keyboard_without_session_switch")
    if report.get("latency"):
        expected_checks.add("cross_session_focus_latency_and_host_local_active_tab_rendering")
    assert len(report["checks"])==len(expected_checks) and set(report["checks"])==expected_checks
    root=Path(report["root"])
    assert root.name.startswith("vj-h2-")
    load=lambda name:json.loads((root / name).read_text())
    manifest=load("review-manifest.json")
    assert manifest["root"]==str(root) and manifest["binary"]==report["stock_binary"]
    assert "workspaces/verij-agent-monitoring/zellij/" not in report["stock_binary"]
    assert len(report["stock_sha256"])==64
    inventory=load("inventory-initial.json")
    assert inventory["session_instance_id"] and inventory["server_process"]
    panes={str(pane["terminal_id"]):pane for pane in inventory["panes"]}
    assert set(panes)==set(map(str,range(5)))
    states={}
    agents=load("agent-evidence.json")
    for pane,instance in manifest["instances"].items():
        identity=agents[pane]["identity"]
        state=agents[pane]["state"]
        assert identity["is_synthetic"] is True
        assert identity["agent_instance_id"]==instance==state["agent_instance_id"]
        assert identity["pane_key"]=={"session":inventory["session_instance_id"],"terminal":int(pane)}
        assert identity["process"]==panes[pane]["pane_process"]
        assert identity["process"]["pid"]>0 and identity["process"]["start_jiffies"]>0
        assert state["reduced"]["conversation_title"]==f"Fixture-{pane}"
        states[pane]=state
    assert states["0"]["completion_revision"]==states["2"]["completion_revision"]==1
    assert states["0"]["reduced"]["status"]==states["2"]["reduced"]["status"]=="done"
    assert states["1"]["reduced"]["pending_requests"]==[{"id":"fixture-permission","kind":"permission"}]
    assert states["1"]["reduced"]["status"]=="needs_input"
    assert states["3"]["reduced"]["status"]=="error"
    assert states["4"]["reduced"]["status"]=="unknown"
    ordered_receipts=[json.loads(line) for line in (root / "control/control-results.ndjson").read_text().splitlines()]
    if report.get("initial_host_bindings_absent"):
        surfaces = [r for r in ordered_receipts if r["request"]["operation"]["type"] == "registration_surface"]
        assert surfaces and all(r["request"]["sequence"] == 0 and not r["whole_host_verified"]
            and not r["acknowledge_done"] for r in surfaces)
        bindings = [load(f"registration-{host}.json") for host in ("a", "b")]
        assert bindings[0]["context"]["client_id"] != bindings[1]["context"]["client_id"]
    receipts={receipt["request"]["request_id"]:receipt for receipt in ordered_receipts}
    if report.get("legacy_source_inventory_absent"):
        recovered = load("legacy-source-recovered.json")
        assert recovered["server_process"] and recovered["session_instance_id"]
        switches = [receipt for receipt in ordered_receipts if receipt["request"]["operation"]["type"] == "switch"
                    and receipt["observation"]["session_name"] == "h2-legacy"]
        assert len(switches) >= 2
        assert all(receipt["status"] == "switch_dispatched_unverified" and
                   receipt["request"]["operation"]["session_name"] == "h2-shared" for receipt in switches)
        focus = load("same-session-focus.json")
        assert len(focus["tokens"]) == 6
        assert not any(receipt["request"]["operation"]["type"] == "switch" for receipt in focus["receipts"])
        for item in focus["tokens"]:
            assert item["token"] in (root / f"input-h2-shared-{item['pane']}").read_text()
            assert any(receipt["status"] == "inner_focus_observed"
                       and receipt["request"]["operation"] == {"type": "focus", "terminal": item["pane"]}
                       and receipt["observation"]["session_name"] == "h2-shared" for receipt in focus["receipts"])
    for stage,expected in (
        ("before-visits",{"a":{},"b":{}}),
        ("after-enter-a",{"a":{"0":1},"b":{}}),
        ("after-both-visits",{"a":{"0":1},"b":{"0":1}}),
        ("after-native-a",{"a":{"0":1,"2":1},"b":{"0":1}}),
    ):
        sample=load(f"ack-{stage}.json")
        for host,wanted in expected.items():
            store=sample[host]
            acks=store["acknowledgements"] if store else {}
            assert set(acks)=={manifest["instances"][pane] for pane in wanted}
            registration=load(f"registration-{host}.json")
            for pane,revision in wanted.items():
                ack=acks[manifest["instances"][pane]]
                assert ack["acknowledged_revision"]==revision
                receipt=receipts[ack["request_id"]]
                assert receipt["status"]=="observed"
                assert receipt["request"]["operation"]=={"type":"query"}
                assert receipt["request"]["context"]==registration["context"]
                assert receipt["observation"]["terminal"]==int(pane)
                index=ordered_receipts.index(receipt)
                outer=[(position,value) for position,value in enumerate(ordered_receipts)
                       if value["request"]["operation"]=={"type":"query"}
                       and value["observation"]["session_name"]==registration["host_session"]
                       and value["observation"]["terminal"]==registration["workspace_pane"]]
                before=next(value for position,value in reversed(outer) if position<index)
                after=next(value for position,value in outer if position>index)
                assert before["observation"]["observed_at_ms"]<=receipt["observation"]["observed_at_ms"]<=after["observation"]["observed_at_ms"]
                assert after["observation"]["observed_at_ms"]<=ack["acknowledged_at_ms"]
    assert "h2-enter-agent-target" in (root / "input-h2-shared-0").read_text()
    assert "h2-needs-input-agent-target" in (root / "input-h2-shared-1").read_text()
    permission_focus=load("needs-input-focus.json")
    assert permission_focus["status"]=="inner_focus_observed"
    assert permission_focus["observation"]["session_name"]=="h2-host-a"
    assert permission_focus["observation"]["terminal"]==1
    restart=load("sidebar-restart.json")
    assert restart["before"]["generation"]<restart["after"]["generation"]
    assert restart["before"]["process"]!=restart["after"]["process"]
    for field in ("selected","collapsed","collapsed_tabs"):
        assert restart["persisted_before"][field]==restart["persisted_after"][field]
    assert restart["persisted_after"]["collapsed_tabs"]
    ready=manifest["ui_ready_receipts"]["a"]
    assert all(ready[key]==value for key,value in restart["after"]["process"].items())
    initial=(root / "initial-statuses-host-a.txt").read_text()
    assert all(f"Fixture-{pane}" in initial for pane in range(5))
    assert all(icon in initial for icon in "○!×?")
    assert any(icon in initial for icon in "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
    assert "✓" in (root / "done-before-visit-host-a.txt").read_text()
    final=(root / "final-tree-host-a.txt").read_text()
    assert "Fixture-4" not in final and "Fixture-3" in final and "stack" in final
    if report.get("startup_contention"):
        contention = load("startup-contention.json")
        assert contention["held_seconds"] >= 2.5
        assert contention["target_title"] in contention["recovered_workspace_title"]
        frames = load("startup-contention-frames.json")
        assert len(frames) == contention["frame_count"] and len(frames) >= 10
        frames.append((root / "startup-contention-after.txt").read_text())
        assert all("Workspace title:" not in frame and "lock deadline" not in frame for frame in frames)
    latency_summary = None
    if report.get("latency"):
        samples = load("cross-session-latency.json")
        assert len(samples) == report["latency"]["hops"] == 8
        assert {sample["session"] for sample in samples} == {"h2-legacy", "h2-shared"}
        for index, sample in enumerate(samples):
            assert 0 < sample["focus_ms"] <= sample["highlight_ms"]
            receipt = sample["outer_receipt"]
            assert receipt["status"] == "inner_focus_observed"
            assert receipt["observation"]["session_name"] == "h2-host-a"
            assert receipt["observation"]["observed_at_ms"] >= sample["started_at_ms"]
            assert receipt in ordered_receipts
            assert any(value["status"] == "inner_focus_observed"
                       and value["observation"]["session_name"] == sample["session"]
                       and value["observation"]["terminal"] == sample["pane"]
                       and value["observation"]["observed_at_ms"] >= sample["started_at_ms"] for value in ordered_receipts)
            assert sample["token"] in (root / f"input-{sample['session']}-{sample['pane']}").read_text()
            assert sample["marker"] in (root / f"cross-hop-{index}.txt").read_text()
            if "verij_plugin_panes" in sample:
                assert set(sample["verij_plugin_panes"]) == {"h2-legacy", "h2-shared"}
                assert all(len(ids) <= 3 for ids in sample["verij_plugin_panes"].values())
        latency_summary = {
            "hops": len(samples),
            "min_focus_ms": min(sample["focus_ms"] for sample in samples),
            "median_focus_ms": statistics.median(sample["focus_ms"] for sample in samples),
            "max_focus_ms": max(sample["focus_ms"] for sample in samples),
            "median_highlight_followup_ms": statistics.median(sample["highlight_ms"] - sample["focus_ms"] for sample in samples),
            "max_highlight_followup_ms": max(sample["highlight_ms"] - sample["focus_ms"] for sample in samples),
        }
    if report.get("review_live"):
        assert Path(manifest["tmux_socket"]).exists()
    else:
        assert report["cleanup"]=={"private_socket_removed":True,"known_processes_exited":True}
        assert not Path(manifest["socket_dir"]).exists()
    print(json.dumps({"status":"PASS","root":str(root),"checks":len(expected_checks),
                      "evidence":"native identity/control receipts, receiver token, staged host acknowledgements, restart and UI frames",
                      "semantic_records_synthetic":True,"human_h2_approved":False,
                      **({"latency": latency_summary} if latency_summary else {})},indent=2))


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report",type=Path)
    verify(parser.parse_args().report)
