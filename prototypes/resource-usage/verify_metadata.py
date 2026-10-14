#!/usr/bin/env python3
"""Check retained real metadata transport, completion, isolation and cleanup evidence."""
import argparse
import hashlib
import json
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    parser.add_argument("--current-assets", action="store_true")
    args = parser.parse_args()
    report = json.loads(args.report.read_text())
    assert report["status"] == "PASS"
    assert report["opencode_version"].startswith("1.")
    assert report["probe_processes_exited"] and report["server_and_host_exited"]
    assert report["rss_guard_mib"] == {"opencode": 1536, "zellij": 512}
    assert len(report["peak_rss_mib"]) >= 4
    assert all(value <= 1536 for value in report["peak_rss_mib"].values())
    for asset in report["assets"].values():
        assert len(asset["sha256"]) == 64
        if args.current_assets:
            assert hashlib.sha256(Path(asset["path"]).read_bytes()).hexdigest() == asset["sha256"]
    checks = report["checks"]
    assert any(check.get("private_home_xdg_database_and_verified_listener") for check in checks)
    assert any(check.get("two_native_panes_foreground_tpgid_and_birth_verified") for check in checks)
    attached = next(check for check in checks if check.get("large_output_repeated_recovery"))
    assert attached["history_messages"] > 100
    assert attached["synthetic_tool_output_bytes"] >= 2**20
    assert attached["message_queries"] >= 6
    assert attached["max_message_limit"] == 1
    before, after = attached["metrics_before"], attached["metrics_after"]
    assert after["endpoints"]["session.messages"] - before["endpoints"]["session.messages"] == attached["message_queries"]
    bytes_read = after["transport"]["session.messages"]["bytes"] - before["transport"]["session.messages"]["bytes"]
    assert bytes_read == attached["transport_bytes"] < attached["synthetic_tool_output_bytes"] // 2
    assert after["transport"]["session.messages"]["cancellations"] > before["transport"]["session.messages"]["cancellations"]
    reload = next(check for check in checks if check.get("long_history_reload_same_completion"))
    assert reload["source_epoch_after"] > reload["source_epoch_before"]
    assert reload["completion_revision"] == attached["completion_revision"] == 1
    assert reload["provider_requests"]["success"] == 1
    worker = next(check for check in checks if check.get("standalone_worker_no_message_body_rpc"))
    before, after = worker["metrics_before"], worker["metrics_after"]
    assert after["endpoints"]["session.messages"] - before["endpoints"]["session.messages"] >= 6
    assert after["transport"]["session.messages"] == {"bytes": 0, "reads": 0, "cancellations": 0}
    assert after["max_message_limit"] == worker["completion_revision"] == 1
    assert after["producer_paths"]["directory"] == after["cwd"]
    # Startup/rebind can cancel unrelated SDK reads; retained diagnostics remain
    # visible. Metadata proofs must succeed and the native Done check passed live.
    assert not after["failures"].get("session.messages")
    print(json.dumps({"status": "PASS", "checks": len(checks), "attach_transport_bytes": bytes_read,
                      "worker_message_body_bytes": 0, "same_completion_after_reload": True}))


if __name__ == "__main__":
    main()
