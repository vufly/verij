#!/usr/bin/env python3
"""Corroborate H2 UI/status claims against retained private fixture projections."""
import argparse
import json
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
    assert len(report["checks"])==len(CHECKS) and set(report["checks"])==CHECKS
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
    receipts={receipt["request"]["request_id"]:receipt for receipt in ordered_receipts}
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
    if report.get("review_live"):
        assert Path(manifest["tmux_socket"]).exists()
    else:
        assert report["cleanup"]=={"private_socket_removed":True,"known_processes_exited":True}
        assert not Path(manifest["socket_dir"]).exists()
    print(json.dumps({"status":"PASS","root":str(root),"checks":len(CHECKS),
                      "evidence":"native identity/control receipts, receiver token, staged host acknowledgements, restart and UI frames",
                      "semantic_records_synthetic":True,"human_h2_approved":False},indent=2))


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report",type=Path)
    verify(parser.parse_args().report)
