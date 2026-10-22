#!/usr/bin/env python3
"""Private interactive route/event/reload experiment, not production monitoring."""
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
import time

from marker_probe import fixture_config
from provider import Provider
from smoke import HERE, PrivateRun, identity, same_birth, sha
from verify_evidence import bounded_json, read_report

ARTIFACTS = ("cli_contract.py", "cli_evidence.py", "cli-observer.mjs", "cli-observer-core.mjs",
             "marker_probe.py", "marker_evidence.py", "provider.py", "smoke.py", "verify_evidence.py", "pin.json")
CHECKS = ("home", "interactive_success", "options_reload", "source_reload", "disabled_cleanup",
          "disabled_execution", "reenabled_selection", "reenabled_success", "exit_cleanup")


def frames(path):
    if not path.exists():
        return []
    with path.open("rb") as stream:
        raw = stream.read(64 * 1024 + 1)
    if len(raw) > 64 * 1024:
        raise ValueError("trace_byte_bound")
    lines = raw.splitlines(keepends=True)
    if len(lines) > 96 or any(len(line) > 2048 for line in lines):
        raise ValueError("trace_frame_bound")
    return [bounded_json(line, maximum=2048) for line in lines if line.endswith(b"\n")]


class CliExperiment(PrivateRun):
    def __init__(self, binary, root, mode, provider):
        super().__init__(binary, root, mode)
        self.provider = provider
        self.output = root / "cli-observer.ndjson"
        self.config_path = root / "config/opencode/cli.json"
        (root / "config/opencode/opencode.json").write_text(json.dumps(fixture_config(provider)))
        (root / "fixture/tui.js").write_bytes((HERE / "cli-observer.mjs").read_bytes())
        (root / "fixture/cli-observer-core.mjs").write_bytes((HERE / "cli-observer-core.mjs").read_bytes())
        self.config(1)
        self.report.update(stage="start", checks=[], initial_provider_requests=0, disabled_provider_requests=0, provider_requests=None)

    def config(self, generation, disabled=False):
        entries = [{"package": str(self.root / "fixture"), "options": {"fixture": "verij-v2-cli", "generation": generation}}]
        if disabled:
            entries.append("-verij.cli.contract")
        value = {"plugins": entries, "highlights": False, "animations": False,
                 "attention": {"notifications": False, "sound": False}}
        # Atomic save tests the tagged config directory watcher, not partial JSON.
        temporary = self.config_path.with_suffix(".tmp")
        temporary.write_text(json.dumps(value))
        temporary.replace(self.config_path)

    def check(self, name, condition):
        if not condition:
            raise ValueError("contract_assertion")
        self.report["checks"].append(name)

    def current(self):
        return frames(self.output)

    def count(self, kind):
        return sum(frame["kind"] == kind for frame in self.current())

    def quiet(self, seconds=.5):
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            self.descendants()
            self.drain()
            time.sleep(.025)

    def selected(self):
        routes = [frame for frame in self.current() if frame["kind"] == "route"]
        return routes[-1] if routes else None

    def prompt(self):
        os.write(self.master, b"Return OK")
        self.quiet(.15)
        os.write(self.master, b"\r")

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
        self.wait(lambda: sum(f.get("event") == "session.execution.succeeded" for f in self.current()) == 2, timeout=20)
        self.quiet(1)
        selected = self.selected()["selected"]
        self.report["initial_provider_requests"] = self.provider.snapshot()["success"]
        self.check("interactive_success", selected is not None and self.selected()["route"] == "session"
                   and 1 <= self.report["initial_provider_requests"] <= 3)

        self.report["stage"] = "reload"
        self.config(2)
        self.wait(lambda: self.count("setup") == 2 and self.count("cleanup") == 1 and self.selected()["generation"] == 2)
        self.check("options_reload", self.selected()["selected"] == selected)
        source = self.root / "fixture/tui.js"
        source.write_bytes(source.read_bytes() + b"\n// Private source-save reload fixture.\n")
        self.wait(lambda: self.count("setup") == 3 and self.count("cleanup") == 2)
        self.check("source_reload", self.selected()["selected"] == selected)
        self.config(2, disabled=True)
        self.wait(lambda: self.count("cleanup") == 3)
        self.check("disabled_cleanup", self.count("setup") == 3)
        before_frames = self.current()
        before_calls = self.provider.snapshot()["success"]
        self.prompt()
        self.wait(lambda: self.provider.snapshot()["success"] == before_calls + 1, timeout=15)
        self.quiet(1)
        self.report["disabled_provider_requests"] = self.provider.snapshot()["success"] - before_calls
        self.check("disabled_execution", self.current() == before_frames and self.report["disabled_provider_requests"] == 1)
        self.config(3)
        self.wait(lambda: self.count("setup") == 4 and self.selected()["generation"] == 3)
        self.check("reenabled_selection", self.selected()["selected"] == selected)
        self.prompt()
        self.wait(lambda: sum(f.get("event") == "session.execution.succeeded" and f["generation"] == 3
                             for f in self.current()) == 2, timeout=20)
        self.check("reenabled_success", self.provider.snapshot()["success"] == self.report["initial_provider_requests"] + 2)
        self.report["stage"] = "exit"
        os.write(master, b"\x03")
        self.wait(lambda: self.count("cleanup") == 4, timeout=8)
        self.check("exit_cleanup", self.count("setup") == 4)
        self.report.update(status="CLI_PASS", stage="complete")


def run(binary):
    pin = read_report(HERE / "pin.json")
    if not binary.is_absolute() or binary.resolve() != binary or sha(binary) != pin["executable_sha256"]:
        raise ValueError("pinned_absolute_binary_required")
    if Path("/etc/opencode").exists() or Path("/etc/opencode").is_symlink():
        raise ValueError("ambient_system_config")
    report = {"schema": 1, "target": pin, "artifact_hashes": {name: sha(HERE / name) for name in ARTIFACTS},
              "status": "INCOMPLETE", "production_admission": False, "connected_transport_qualified": False, "runs": []}
    for mode in ("standalone", "server"):
        provider = Provider()
        try:
            with tempfile.TemporaryDirectory(prefix="verij-v2-cli-") as temporary:
                experiment = CliExperiment(binary, Path(temporary), mode, provider)
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
    if all(item["status"] == "CLI_PASS" and item["cleanup"] for item in report["runs"]):
        report["status"] = "CLI_PASS"
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
    from cli_evidence import validate
    validate(report, require_pass=False)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "production_admission": False}))
    return 0 if report["status"] == "CLI_PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
