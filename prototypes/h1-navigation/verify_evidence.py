#!/usr/bin/env python3
"""Validate original production H1 receiver, identity, acknowledgement and cleanup evidence."""
import json
from pathlib import Path


def main():
    result = json.loads(Path(__file__).with_name("results.json").read_text())
    assert result["status"] == "PASS" and not result["human_gate_approved"]
    root = Path(result["root"])
    initial = json.loads((root / "inventory-initial.json").read_text())
    assert initial["session_instance_id"] and {pane["terminal_id"] for pane in initial["panes"]} == set(range(5))
    assert all(pane["pane_process"] and pane["stack_id"] is None for pane in initial["panes"])
    a = json.loads((root / "registration-a.json").read_text())
    b = json.loads((root / "registration-b.json").read_text())
    assert a["context"]["client_id"] != b["context"]["client_id"]
    assert a["server_process"] == b["server_process"]
    for label, token in ((0, "h1-a-pane-0"), (1, "h1-b-pane-1"), (2, "h1-a-pane-2"), (3, "h1-a-pane-3"),
                         (4, "h1-a-pane-4"), (1, "h1-fullscreen-other-target"), (0, "h1-owned-attachment-still-single"),
                         (0,"h1-after-return-sequence-resynced"),(1,"h1-after-rename-still-addressed")):
        assert token in (root / f"input-h1-shared-{label}").read_text()
    manifest = json.loads((root / "review-manifest.json").read_text())
    identity = json.loads((root / "agents/agents/v1" / manifest["synthetic_instance"] / "identity.json").read_text())
    state = json.loads((root / "agents/agents/v1" / manifest["synthetic_instance"] / "state.json").read_text())
    assert identity["is_synthetic"] and identity["pane_key"]["session"] == initial["session_instance_id"]
    assert state["completion_revision"] == 2 and state["reduced"]["status"] == "done"
    acknowledgements = json.loads((root / "state/verij/agent-monitoring/hosts/a.json").read_text())
    assert acknowledgements["acknowledgements"][manifest["synthetic_instance"]]["acknowledged_revision"] == 1
    assert not (root / "state/verij/agent-monitoring/hosts/b.json").exists()
    assert result["cleanup"]["known_processes_exited"] and not Path(manifest["socket_dir"]).exists()
    print(json.dumps({"live_checks": len(result["checks"]), "actual_receiver_tokens": True,
                      "synthetic_semantics_explicit": True, "host_ack_did_not_consume_newer_completion": True,
                      "cleaned_automated_run": True, "human_h1_approved": False}, indent=2))


if __name__ == "__main__":
    main()
