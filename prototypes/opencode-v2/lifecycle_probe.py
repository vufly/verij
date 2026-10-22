#!/usr/bin/env python3
"""Private revert and active-shutdown experiment; never grants V2 admission.

Only fixture control mutates its disposable server. No CLI plugin, production
configuration, user service, or native completion ledger is touched.
"""
import argparse
import json
from pathlib import Path
import signal
import subprocess
import sys
import tempfile

from marker_probe import Experiment, key
from provider import Provider
from smoke import HERE, sha, signal_birth
from verify_evidence import read_report

ARTIFACTS = ("lifecycle_probe.py", "lifecycle_evidence.py", "marker_probe.py", "marker_evidence.py", "provider.py",
             "smoke.py", "verify_evidence.py", "pin.json")
CHECKS = ("two_completed_turns", "stage_preserves_history", "clear_wakes_new_marker",
          "commit_removes_reverted_markers", "replacement_turn_identity",
          "active_has_no_new_marker", "shutdown_preserves_open_turn", "restart_preserves_open_turn")
LABELS = ("completed", "staged", "cleared", "committed", "replacement", "active", "after_shutdown", "after_restart")


class LifecycleExperiment(Experiment):
    def __init__(self, binary, root, provider):
        super().__init__(binary, None, root, provider)
        self.report = {"status": "INCOMPLETE", "stage": "start", "failure": None,
                       "checks": [], "samples": [], "provider_requests": {}, "clear_provider_requests": 0, "cleanup": False}

    def observe(self, session, label):
        sample = self.sample(session, label)
        # Keep only complete projections if a later fixture read fails.
        self.report["samples"].pop()
        info = self.control.call(f"/api/session/{session}")["data"]
        sample["revert"] = key(info["revert"]["messageID"]) if info.get("revert") else None
        sample["active"] = session in self.control.call("/api/session/active")["data"]
        self.report["samples"].append(sample)
        return sample

    def run(self):
        self.private.version()
        self.start()
        self.report["stage"] = "revert"
        root = self.create()
        self.turn(root)
        first_marker = next(message["id"] for message in self.history(root) if message["type"] == "idle")
        self.turn(root)
        before = self.observe(root, "completed")
        self.check("two_completed_turns", before["marker_count"] == 2 and before["outcome"] == "succeeded")
        # Revert the second user message. Snapshot/file changes are disabled;
        # fixture control uses a disposable non-project cwd exclusively.
        boundary = next(message["id"] for message in self.history(root) if message["type"] == "user")
        self.control.call(f"/api/session/{root}/revert/stage", {"messageID": boundary, "files": False})
        staged = self.observe(root, "staged")
        self.check("stage_preserves_history", staged["revert"] == key(boundary)
                   and staged["message_ids"] == before["message_ids"] and staged["marker"] == before["marker"])
        calls_before_clear = self.provider.snapshot()["success"]
        self.control.call(f"/api/session/{root}/revert", method="DELETE")
        self.private.wait(lambda: root not in self.control.call("/api/session/active")["data"], timeout=8)
        cleared = self.observe(root, "cleared")
        self.report["clear_provider_requests"] = self.provider.snapshot()["success"] - calls_before_clear
        # Tagged clear wakes execution even without new input. It can mint a
        # successful idle marker without a model request: identity alone is
        # not proof that a new user execution should be acknowledged.
        self.check("clear_wakes_new_marker", cleared["revert"] is None and cleared["marker_count"] == 3
                   and cleared["message_ids"][1:] == before["message_ids"]
                   and cleared["marker"] not in before["message_ids"] and self.report["clear_provider_requests"] == 0)
        self.control.call(f"/api/session/{root}/revert/stage", {"messageID": boundary, "files": False})
        self.control.call(f"/api/session/{root}/revert/commit", {}, method="POST")
        committed = self.observe(root, "committed")
        self.check("commit_removes_reverted_markers", committed["revert"] is None and committed["marker_count"] == 1
                   and committed["marker"] == key(first_marker) and before["marker"] not in committed["message_ids"]
                   and key(boundary) not in committed["message_ids"])
        self.turn(root)
        replacement = self.observe(root, "replacement")
        self.check("replacement_turn_identity", replacement["marker_count"] == 2
                   and replacement["marker"] not in before["message_ids"]
                   and committed["marker"] in replacement["message_ids"])

        self.report["stage"] = "shutdown"
        # Complete one blocked-model turn, then interrupt the next by stopping
        # only our recorded server birth. Its historical marker is not current.
        self.provider.gate.set()
        suspended = self.create("blocked")
        self.turn(suspended)
        old_marker = next(message["id"] for message in self.history(suspended) if message["type"] == "idle")
        self.provider.entered.clear()
        self.provider.gate.clear()
        self.control.call(f"/api/session/{suspended}/prompt", {"text": "Return OK"})
        self.private.wait(self.provider.entered.is_set, timeout=8)
        active = self.observe(suspended, "active")
        self.check("active_has_no_new_marker", active["active"] and active["marker_count"] == 1
                   and active["marker"] == key(old_marker) and active["message_types"][0] != "idle")
        server = self.private.processes[-1]
        signal_birth(self.private.births[server.pid], signal.SIGTERM)
        self.private.wait(lambda: server.poll() is not None, timeout=10)
        self.provider.gate.set()
        self.start()
        shutdown = self.observe(suspended, "after_shutdown")
        self.check("shutdown_preserves_open_turn", shutdown["marker_count"] == 1
                   and shutdown["marker"] == active["marker"]
                   and shutdown["message_ids"][-len(active["message_ids"]):] == active["message_ids"]
                   and shutdown["message_types"][:len(shutdown["message_ids"]) - len(active["message_ids"])] in ([], ["assistant"]))
        # Explicit, unregistered `serve` does not establish managed-server
        # automatic resume. A second read proves replay only, not resumption.
        recovered = self.observe(suspended, "after_restart")
        self.check("restart_preserves_open_turn", recovered == {**shutdown, "label": "after_restart"})
        self.report.update(status="LIFECYCLE_PASS", stage="complete")


def run(binary):
    pin = read_report(HERE / "pin.json")
    if not binary.is_absolute() or binary.resolve() != binary or sha(binary) != pin["executable_sha256"]:
        raise ValueError("pinned_absolute_binary_required")
    if Path("/etc/opencode").exists() or Path("/etc/opencode").is_symlink():
        raise ValueError("ambient_system_config")
    report = {"schema": 1, "target": pin, "artifact_hashes": {name: sha(HERE / name) for name in ARTIFACTS},
              "evidence_kind": "private-headless-lifecycle", "production_admission": False,
              "connected_transport_qualified": False, "automatic_resume_qualified": False, "run": None}
    provider = Provider()
    try:
        with tempfile.TemporaryDirectory(prefix="verij-v2-lifecycle-") as temporary:
            experiment = LifecycleExperiment(binary, Path(temporary), provider)
            try:
                experiment.run()
            except (OSError, RuntimeError, ValueError, KeyError, TypeError, TimeoutError, subprocess.SubprocessError) as error:
                # Fixed classifications only; upstream error text is private.
                experiment.report["failure"] = ("TimeoutError" if isinstance(error, TimeoutError) else
                    "ValueError" if isinstance(error, ValueError) else "OSError" if isinstance(error, OSError) else
                    "SubprocessError" if isinstance(error, subprocess.SubprocessError) else type(error).__name__)
            finally:
                experiment.private.close()
                experiment.report["cleanup"] = experiment.private.report["cleanup"]
                experiment.report["provider_requests"] = provider.snapshot()
                report["run"] = experiment.report
    finally:
        closed = provider.close()
    report["run"]["cleanup"] = report["run"]["cleanup"] and closed
    if sha(binary) != pin["executable_sha256"]:
        raise ValueError("binary_changed")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    def interrupted(_number, _frame):
        raise KeyboardInterrupt
    previous = signal.signal(signal.SIGTERM, interrupted)
    try:
        report = run(args.binary)
    finally:
        signal.signal(signal.SIGTERM, previous)
    from lifecycle_evidence import validate
    validate(report, require_pass=False)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["run"]["status"], "production_admission": False}))
    return 0 if report["run"]["status"] == "LIFECYCLE_PASS" and report["run"]["cleanup"] else 1


if __name__ == "__main__":
    sys.exit(main())
