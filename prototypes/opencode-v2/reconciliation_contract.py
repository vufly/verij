#!/usr/bin/env python3
"""Private explicit-server CLI/event-to-marker experiment; no support admission."""
import argparse
import json
from pathlib import Path
import signal
import subprocess
import sys
import tempfile

from marker_probe import Control, key
from provider import Provider
from smoke import HERE, sha, signal_birth
from terminal_contract import ARTIFACTS as TERMINAL_ARTIFACTS, CHECKS as TERMINAL_CHECKS, TerminalExperiment
from verify_evidence import read_report

ARTIFACTS = (*TERMINAL_ARTIFACTS, "reconciliation_contract.py", "reconciliation_evidence.py",
             "reconciliation-observer.mjs", "reconciliation-plugin.mjs")
CHECKS = (*TERMINAL_CHECKS, "event_marker_identity", "repeated_identity", "restart_identity",
          "missed_terminal_recovery", "missed_repeated_identity")
LABELS = ("terminal", "repeated", "restart", "missed", "missed_repeated")
CASES = ("failure", "user_abort")


class ReconciliationExperiment(TerminalExperiment):
    def __init__(self, binary, root, provider, case):
        super().__init__(binary, root, "server", provider, case)
        for name in ("reconciliation-observer.mjs", "reconciliation-plugin.mjs"):
            (root / "fixture" / name).write_bytes((HERE / name).read_bytes())
        (root / "fixture/tui.js").write_bytes((HERE / "reconciliation-plugin.mjs").read_bytes())
        self.report.update(samples=[], control_calls=0, observer_unchanged=False)
        self.control = None
        self.server = None

    def serve(self):
        url = super().serve()
        self.server = self.processes[-1]
        # Provisioned by the fixture that launched the server, never scraped
        # from the CLI, shared service, process args, or connection defaults.
        self.control = Control(url, self.env["OPENCODE_PASSWORD"])
        return url

    def read(self, path):
        if self.report["control_calls"] >= 128:
            raise ValueError("read_call_bound")
        self.report["control_calls"] += 1
        return self.control.call(path)

    def sample(self, session, label):
        info = self.read(f"/api/session/{session}")["data"]
        active = self.read("/api/session/active")["data"]
        head = self.read(f"/api/session/{session}/message?limit=1&order=desc")["data"]
        if type(head) is not list or len(head) != 1 or head[0].get("type") != "idle":
            raise ValueError("terminal_head_required")
        marker = head[0]
        error = marker.get("error")
        error_type = None
        if error is not None:
            if type(error) is not dict or error.get("type") != "provider.invalid-request":
                raise ValueError("fixture_error_type")
            error_type = error["type"]
        sample = {"label": label, "session": key(info["id"]), "outcome": info.get("outcome"),
                  "marker": key(marker["id"]), "marker_outcome": marker.get("outcome"),
                  "error_type": error_type, "has_interruption_reason": "reason" in marker, "active": session in active}
        # Validate projection before retaining any upstream fields.
        from reconciliation_evidence import validate_sample
        validate_sample(sample)
        self.report["samples"].append(sample)
        return sample

    def launch(self):
        super().launch()
        client = next(process for process in self.processes if process.pid == self.report["client"]["pid"])
        self.wait(lambda: client.poll() is not None, timeout=8)
        # The terminal fixture exited its CLI and invoked observer cleanup.
        # Subsequent reads are fixture-owned GETs, not a plugin transport hook.
        self.report.update(status="INCOMPLETE", stage="recovery")
        frozen = self.current()
        terminal = self.events()[-1]
        roots = self.read("/api/session?limit=2&order=desc")["data"]
        if len(roots) != 1 or key(roots[0]["id"]) != terminal["session"] or roots[0].get("parentID"):
            raise ValueError("isolated_root_required")
        session = roots[0]["id"]
        first = self.sample(session, "terminal")
        expected = "failed" if self.case == "failure" else "interrupted"
        self.check("event_marker_identity", first["marker"] == terminal["marker_from_event"]
                   and first["outcome"] == first["marker_outcome"] == expected and not first["active"])
        repeated = self.sample(session, "repeated")
        self.check("repeated_identity", repeated == {**first, "label": "repeated"})

        signal_birth(self.births[self.server.pid], signal.SIGTERM)
        self.wait(lambda: self.server.poll() is not None, timeout=10)
        self.serve()
        def ready():
            try:
                return self.read("/api/info").get("version") == "2.0.32"
            except RuntimeError:
                return False
        self.wait(ready, timeout=15)
        restarted = self.sample(session, "restart")
        self.check("restart_identity", restarted == {**first, "label": "restart"})

        self.report["stage"] = "missed"
        self.provider.entered.clear()
        self.provider.gate.clear()
        # Fixture execution control only. The observer has already disposed.
        self.control.call(f"/api/session/{session}/prompt", {"text": "Return OK"})
        if self.case == "user_abort":
            self.wait(self.provider.entered.is_set, timeout=10)
            self.control.call(f"/api/session/{session}/interrupt?resume=false", {}, method="POST")
        def settled():
            active = self.read("/api/session/active")["data"]
            head = self.read(f"/api/session/{session}/message?limit=1&order=desc")["data"]
            return session not in active and len(head) == 1 and head[0].get("type") == "idle" and key(head[0]["id"]) != first["marker"]
        self.wait(settled, timeout=20)
        missed = self.sample(session, "missed")
        self.report["observer_unchanged"] = self.current() == frozen
        self.check("missed_terminal_recovery", missed["marker"] != first["marker"]
                   and missed["outcome"] == missed["marker_outcome"] == expected
                   and not missed["active"] and self.report["observer_unchanged"])
        again = self.sample(session, "missed_repeated")
        self.check("missed_repeated_identity", again == {**missed, "label": "missed_repeated"})
        self.report.update(status="RECONCILIATION_PASS", stage="complete")


def run(binary):
    pin = read_report(HERE / "pin.json")
    if not binary.is_absolute() or binary.resolve() != binary or sha(binary) != pin["executable_sha256"]:
        raise ValueError("pinned_absolute_binary_required")
    if Path("/etc/opencode").exists() or Path("/etc/opencode").is_symlink():
        raise ValueError("ambient_system_config")
    report = {"schema": 1, "target": pin, "artifact_hashes": {name: sha(HERE / name) for name in ARTIFACTS},
              "status": "INCOMPLETE", "production_admission": False, "connected_transport_qualified": False, "runs": []}
    for case in CASES:
        provider = Provider()
        try:
            with tempfile.TemporaryDirectory(prefix="verij-v2-reconciliation-") as temporary:
                experiment = ReconciliationExperiment(binary, Path(temporary), provider, case)
                try:
                    experiment.launch()
                except (OSError, RuntimeError, ValueError, KeyError, TypeError, TimeoutError, subprocess.SubprocessError) as error:
                    experiment.report.update(status="INCOMPLETE", failure=("TimeoutError" if isinstance(error, TimeoutError) else
                        "OSError" if isinstance(error, OSError) else "SubprocessError" if isinstance(error, subprocess.SubprocessError)
                        else type(error).__name__))
                finally:
                    try:
                        experiment.report["frames"] = experiment.current()
                    except (OSError, ValueError, TypeError):
                        experiment.report.update(frames=[], status="INCOMPLETE", failure="ValueError")
                    finally:
                        experiment.close()
                        experiment.report["provider_requests"] = provider.snapshot()
                        report["runs"].append(experiment.report)
        finally:
            closed = provider.close()
        report["runs"][-1]["cleanup"] = report["runs"][-1]["cleanup"] and closed
    if sha(binary) != pin["executable_sha256"]:
        raise ValueError("binary_changed")
    if all(item["status"] == "RECONCILIATION_PASS" and item["cleanup"] for item in report["runs"]):
        report["status"] = "RECONCILIATION_PASS"
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
    from reconciliation_evidence import validate
    validate(report, require_pass=False)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "production_admission": False}))
    return 0 if report["status"] == "RECONCILIATION_PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
