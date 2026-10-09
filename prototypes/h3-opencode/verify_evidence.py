#!/usr/bin/env python3
"""Corroborate H3 production snapshots against original TUI metadata and births."""
import argparse
import hashlib
import json
from pathlib import Path


def key(value):
    return hashlib.sha256(value.encode()).hexdigest()[:20]


def verify(file):
    report = json.loads(file.read_text())
    assert report["status"] == "PASS", report.get("stage")
    assert report["probe_processes_exited"] and report["server_and_host_exited"]
    root = Path(report["artifacts_dir"])
    events = [json.loads(line) for line in (root / "tui-events.ndjson").read_text().splitlines()]
    seen = set()
    verified = 0
    for check in report["checks"]:
        if "evidence_file" not in check:
            continue
        original = json.loads(Path(check["evidence_file"]).read_text())
        assert original == check["records"], "report snapshot differs from original capture"
        for record in original:
            identity, state = record["identity"], record["state"]
            assert not identity["is_synthetic"]
            process = identity["process"]
            init = next(e for e in events if e["kind"] == "init" and e["pid"] == process["pid"])
            assert int(init["pane"]) == identity["pane_key"]["terminal"]
            birth = init["birth"]
            assert birth["pgrp"] == birth["tpgid"] and birth["tty_nr"] != 0
            assert all(birth[name] == process[name] for name in ("pid", "start_jiffies", "boot_id"))
            seen.add(identity["agent_instance_id"])
            reduced = state["reduced"]
            source = state["sources"]["opencode"]
            assert source["source_epoch"] > 0 and source["source_revision"] > 0
            assert "lease" in source["capabilities"]
            if reduced["status"] == "done":
                conversation, user = reduced["current_turn_id"].split("/")
                assert conversation == reduced["conversation_id"]
                assert not reduced["pending_requests"] and reduced["background_task_count"] == 0
                assert any(e.get("pid") == process["pid"] and e.get("session") == key(conversation)
                           and e.get("message", {}).get("role") == "assistant"
                           and e["message"].get("parent_message") == key(user)
                           and e["message"].get("finish") in ("stop", "end_turn")
                           and e["message"].get("completed") and not e["message"].get("error_name")
                           for e in events), "Done lacks actual terminal assistant metadata"
                assert state["opencode_completions"][reduced["current_turn_id"]] == reduced["completion_revision"]
            for request in reduced["pending_requests"]:
                assert any(e["kind"] == "h3-request" and e["pid"] == process["pid"]
                           and e["request"] == key(request["id"]) and e["type"] == request["kind"] + ".asked"
                           for e in events), "pending request lacks actual upstream event"
            if reduced["status"] == "error":
                assert any(e.get("pid") == process["pid"] and e.get("message", {}).get("error_name")
                           == reduced["latest_error"]["message"] for e in events)
            verified += 1
    assert len(seen) >= 4, "must include distinct replacement and standalone process bindings"
    print(json.dumps({"status":"PASS","checks":len(report["checks"]),"corroborated_snapshots":verified,
                      "instances":len(seen)}, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("results", type=Path)
    verify(parser.parse_args().results)
