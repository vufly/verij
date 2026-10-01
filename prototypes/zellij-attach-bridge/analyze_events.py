#!/usr/bin/env python3
"""Summarize private debug IPC lifecycle traces without terminal contents."""
import argparse
import json
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    events = [(path.stem, json.loads(path.read_text())) for path in args.directory.glob("*.json")]
    events.sort(key=lambda item: int(item[1].get("time_ns", 0)))
    mismatch = [{"origin_generation": name.removeprefix("route-ended-"), **event}
                for name, event in events if name.startswith("route-ended-")
                and event.get("current_generation") not in (None, name.removeprefix("route-ended-"))]
    timeline = []
    for name, event in events:
        detail = {"name": name, "time_ns": event.get("time_ns")}
        for key in ("client_id", "requester", "current_generation", "bound", "ok", "error", "failure"):
            if key in event:
                detail[key] = event[key]
        if "result" in event:
            detail["status"] = event["result"]["status"]
        timeline.append(detail)
    report = {"directory": str(args.directory), "event_count": len(events),
              "route_cleanup_generation_mismatches": mismatch, "last_events": timeline[-90:]}
    text = json.dumps(report, indent=2) + "\n"
    if args.output:
        args.output.write_text(text)
    print(text)


if __name__ == "__main__":
    main()
