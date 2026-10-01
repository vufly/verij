#!/usr/bin/env python3
"""Check retained, redacted G2 callback evidence; never infer Done from idle."""
import argparse
from collections import Counter
import json
from pathlib import Path


def qualify(path):
    events = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    ready = [event for event in events if event.get("kind") == "ready"]
    pids = {event["pid"] for event in ready}
    assert len(pids) == 2 and len({event["pane"] for event in ready}) == 2
    callbacks = [event for event in events if event.get("kind") == "event"]
    for status in ("busy", "idle"):
        assert any(event["type"] == "session.status" and event.get("status_type") == status for event in callbacks)
    assert all(event["ts"] >= next(r["ts"] for r in ready if r["pid"] == event["pid"]) for event in callbacks)
    pairs = {}
    for event in callbacks:
        assert event["is_own"] == bool(event["active_route_session_id"] and
                                      event["event_session_id"] == event["active_route_session_id"])
        key = (event["type"], event["evt_id"])
        pairs.setdefault(key, []).append(event)
    broadcast = []
    for kind in ("permission.asked", "permission.replied", "question.asked", "question.replied", "session.error"):
        matches = [items for (event_type, _), items in pairs.items() if event_type == kind and
                   {event["pid"] for event in items} == pids]
        assert matches, f"missing two-client callback for {kind}"
        sample = matches[0]
        assert {event["is_own"] for event in sample} == {True, False}
        broadcast.append({"type": kind, "event_id": sample[0]["evt_id"],
                          "receivers": sorted(event["pid"] for event in sample),
                          "own_and_foreign": True})
    for prefix, pending in (("permission", "has_pending_permission"), ("question", "has_pending_question")):
        asked = [event for event in callbacks if event["type"] == prefix + ".asked" and event["is_own"]]
        replied = [event for event in callbacks if event["type"] == prefix + ".replied" and event["is_own"]]
        assert asked and replied
        assert all(event["state_snapshot"][pending] for event in asked)
        assert any(reply["req_id"] == request["req_id"] and reply["ts"] >= request["ts"] and
                   not reply["state_snapshot"][pending] for request in asked for reply in replied)
    nav = [event for event in events if event.get("kind") == "navigation_executed"]
    assert len(nav) == 4 and {event["new_route"] for event in nav} == {"session", "home"}
    disposed = {event["pid"] for event in events if event.get("kind") == "dispose"}
    assert disposed == pids
    return {"source": str(path), "records": len(events), "types": dict(Counter(event["type"] for event in callbacks)),
            "ready_receivers": sorted(pids), "matched_broadcast_samples": broadcast,
            "pending_request_snapshots_clear_after_matched_reply": True,
            "programmatic_route_switches": nav, "both_plugins_disposed": True,
            "busy_and_idle_observed": True,
            "terminal_success_finish_metadata_verified": False,
            "descendant_family_verified": False, "mid_turn_recovery_verified": False,
            "foreground_terminal_tpgid_verified": False,
            "qualification": "Direct-session route equality demonstrates a candidate filter, not production reducer isolation. Idle is not Done; null is unknown. Shortened IDs are fixture redaction only."}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("observations", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    text = json.dumps(qualify(args.observations), indent=2) + "\n"
    if args.output:
        args.output.write_text(text)
    print(text)


if __name__ == "__main__":
    main()
