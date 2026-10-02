#!/usr/bin/env python3
"""Validate native stock receiver evidence and no-ack guarantees after a verify run."""
import json
from pathlib import Path


def main():
    result = json.loads(Path(__file__).with_name("results.json").read_text())
    assert result["status"] == "PASS" and result["stock_version"] == "zellij 0.45.1"
    root = Path(result["root"])
    assert "workspaces/verij-agent-monitoring/zellij/" not in result["stock_binary"]
    manifest = json.loads((root / "manifest.json").read_text())
    assert result["stock_binary"] == manifest["binary"]
    first = result["checks"][0]
    assert first["a"]["client_id"] != first["b"]["client_id"]
    assert first["a"]["server_pid"] == first["b"]["server_pid"]
    assert first["epochs_are_verij_plugin_loads_not_stock_socket_nonces"]
    traces = [json.loads(line) for line in (root / "control-results.ndjson").read_text().splitlines()]
    assert traces and all(not item["acknowledge_done"] and not item["whole_host_verified"] for item in traces)
    for item in traces:
        if item["status"] == "inner_focus_observed":
            assert item["observation"]["focused_pane"] == f"terminal_{item['request']['terminal_id']}"
    for label, token in (("shared-0", "verified-a-0"), ("shared-1", "verified-b-1"),
                         ("shared-1", "sidebar-enter-stock"), ("shared-0", "sidebar-click-stock"),
                         ("shared-2", "layer-a-2"), ("shared-1", "fullscreen-other-stock"),
                         ("shared-3", "background-tab-stock"), ("other-1", "cross-session-a"),
                         ("shared-1", "peer-remained-shared"), ("shared-3", "stack-a-3"), ("shared-4", "stack-a-4")):
        assert token in (root / f"input-{label}").read_text(), (label, token)
    assert json.loads((root / "cleanup.json").read_text())["known_processes_exited"]
    assert not Path(manifest["socket_dir"]).exists()
    print(json.dumps({"verified_stock_checks": len(result["checks"]), "native_keyboard_and_mouse_receiver_tokens": True,
                      "no_private_dependency_protocol": True, "acknowledgements_never_fabricated": True,
                      "cleanup_observed": True, "automated_evidence_grants_human_approval": False}, indent=2))


if __name__ == "__main__":
    main()
