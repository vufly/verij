#!/usr/bin/env python3
"""Private real-TTY terminal-event experiment; no production support admission."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import signal
import struct
import subprocess
import sys
import tempfile
import termios

from cli_contract import ARTIFACTS as CLI_ARTIFACTS, CliExperiment
from marker_probe import fixture_config
from provider import Provider
from smoke import HERE, identity, same_birth, sha
from verify_evidence import read_report

ARTIFACTS = (*CLI_ARTIFACTS, "terminal_contract.py", "terminal_evidence.py", "terminal-observer.mjs")
CHECKS = ("home", "selected_root", "terminal_event", "typed_envelope", "terminal_metadata", "exit_cleanup")
CASES = (("standalone", "failure"), ("standalone", "user_abort"), ("server", "failure"), ("server", "user_abort"))


class TerminalExperiment(CliExperiment):
    def __init__(self, binary, root, mode, provider, case):
        super().__init__(binary, root, mode, provider)
        self.case = case
        config = fixture_config(provider)
        config["model"] = "fixture/" + ("failure" if case == "failure" else "blocked")
        (root / "config/opencode/opencode.json").write_text(json.dumps(config))
        (root / "fixture/tui.js").write_bytes((HERE / "terminal-observer.mjs").read_bytes())
        del self.report["initial_provider_requests"], self.report["disabled_provider_requests"]
        self.report["case"] = case

    def events(self, channel="typed"):
        return [f for f in self.current() if f["kind"] == "event" and f["channel"] == channel]

    def launch(self):
        self.version()
        args = [str(self.binary)]
        args += ["--standalone"] if self.mode == "standalone" else ["--server", self.serve()]
        master, slave = pty.openpty()
        self.master = master
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 140, 0, 0))
        helper = "import fcntl,os,sys,termios;fcntl.ioctl(0,termios.TIOCSCTTY,0);os.execvpe(sys.argv[1],sys.argv[1:],os.environ)"
        try:
            client = self.remember(subprocess.Popen([sys.executable, "-c", helper, *args],
                cwd=self.root / "work", env=self.env, stdin=slave, stdout=slave, stderr=slave, start_new_session=True))
        finally:
            os.close(slave)
        def loaded():
            if client.poll() is not None:
                raise RuntimeError("client_exit")
            return self.selected()
        self.wait(loaded)
        current = identity(client.pid)
        if not same_birth(self.births.get(client.pid), current):
            raise ValueError("client_birth")
        if not (current["tty"] > 0 and current["pid"] == current["pgrp"] == current["session"] == current["tpgid"]):
            raise ValueError("foreground_tty")
        if Path(f"/proc/{client.pid}/exe").resolve() != self.binary:
            raise ValueError("client_executable")
        self.report["client"] = current
        self.descendants()
        for birth in self.births.values():
            live = identity(birth["pid"])
            if not same_birth(birth, live):
                continue
            executable = Path(f"/proc/{birth['pid']}/exe").resolve()
            if not same_birth(birth, identity(birth["pid"])):
                raise ValueError("layout_birth_changed")
            role = "client" if birth["pid"] == client.pid else "server" if executable == self.binary else "helper"
            self.report["layout"].append({"role": role, "birth": live})
        self.check("home", self.selected()["route"] == "home" and self.selected()["selected"] is None
                   and any(item["role"] == "server" for item in self.report["layout"]))
        self.report["stage"] = "execution"
        self.quiet(1)
        self.prompt()
        self.wait(lambda: self.selected()["selected"] is not None and bool(self.events()), timeout=20)
        selected = self.selected()["selected"]
        self.check("selected_root", self.selected()["route"] == "session" and self.events()[0]["session"] == selected)
        if self.case == "user_abort":
            self.wait(self.provider.entered.is_set, timeout=10)
            self.report["stage"] = "interruption"
            # User input, not a monitor API call. Two Esc presses arm/confirm the
            # tagged prompt's interrupt action. The provider remains blocked.
            self.quiet(.5)
            os.write(master, b"\x1b")
            self.quiet(.2)
            os.write(master, b"\x1b")
        terminal = "session.execution.failed" if self.case == "failure" else "session.execution.interrupted"
        self.wait(lambda: any(f["event"] == terminal for f in self.events())
                  and any(f["event"] == terminal for f in self.events("envelope")), timeout=15)
        self.quiet(.5)
        typed, envelope = self.events(), self.events("envelope")
        self.check("terminal_event", [f["event"] for f in typed] == ["session.execution.started", terminal]
                   and all(f["session"] == f["selected"] == selected for f in typed))
        normalize = lambda values: [{k: v for k, v in f.items() if k not in ("channel", "index")} for f in values]
        self.check("typed_envelope", normalize(typed) == normalize(envelope)
                   and typed[0]["seq"] < typed[1]["seq"] and typed[0]["event_id"] != typed[1]["event_id"])
        self.check("terminal_metadata", typed[-1]["error_type"] == ("provider.invalid-request" if self.case == "failure" else None)
                   and typed[-1]["interruption_reason"] == ("user" if self.case == "user_abort" else None))
        self.report["stage"] = "exit"
        os.write(master, b"\x03")
        self.wait(lambda: self.count("cleanup") == 1, timeout=8)
        self.check("exit_cleanup", self.count("setup") == 1)
        self.report.update(status="TERMINAL_PASS", stage="complete")


def run(binary):
    pin = read_report(HERE / "pin.json")
    if not binary.is_absolute() or binary.resolve() != binary or sha(binary) != pin["executable_sha256"]:
        raise ValueError("pinned_absolute_binary_required")
    if Path("/etc/opencode").exists() or Path("/etc/opencode").is_symlink():
        raise ValueError("ambient_system_config")
    report = {"schema": 1, "target": pin, "artifact_hashes": {name: sha(HERE / name) for name in ARTIFACTS},
              "status": "INCOMPLETE", "production_admission": False, "connected_transport_qualified": False, "runs": []}
    for mode, case in CASES:
        provider = Provider()
        try:
            with tempfile.TemporaryDirectory(prefix="verij-v2-terminal-") as temporary:
                experiment = TerminalExperiment(binary, Path(temporary), mode, provider, case)
                try:
                    experiment.launch()
                except (OSError, RuntimeError, ValueError, KeyError, TypeError, TimeoutError, subprocess.SubprocessError) as error:
                    experiment.report["failure"] = ("TimeoutError" if isinstance(error, TimeoutError) else
                        "OSError" if isinstance(error, OSError) else "SubprocessError" if isinstance(error, subprocess.SubprocessError)
                        else type(error).__name__)
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
    if all(item["status"] == "TERMINAL_PASS" and item["cleanup"] for item in report["runs"]):
        report["status"] = "TERMINAL_PASS"
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
    from terminal_evidence import validate
    validate(report, require_pass=False)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "production_admission": False}))
    return 0 if report["status"] == "TERMINAL_PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
