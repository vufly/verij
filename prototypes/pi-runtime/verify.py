#!/usr/bin/env python3
"""P0: real Pi TUI lifecycle traces in a private PTY with a loopback provider.

Only whitelisted metadata is retained. The provider and active test extension
are synthetic; Pi scheduling, dialogs, cancellation and session lifecycle are
real. This is not pane ownership, a production adapter, or P4 qualification.
"""
import argparse
from collections import Counter
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

from provider import MODELS, Provider

HERE = Path(__file__).resolve().parent
CASES = (
    "success", "parallel", "nested", "tool_error", "provider_error", "retry_success",
    "retry_exhausted", "cancel", "queued", "late_queued", "continuation", "auto_compact", "ui", "ui_nested", "postrun_error", "continuation_error",
)


def sha(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def birth(pid):
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        return None if fields[0] in ("Z", "X", "x") else (pid, int(fields[19]))
    except (OSError, ValueError, IndexError):
        return None


def records(path):
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines(keepends=True) if line.endswith("\n")]


class Probe:
    def __init__(self, command, root, order):
        self.root, self.order = root, order
        self.trace = root / "trace.ndjson"
        self.master = None
        self.child = None
        self.child_birth = None
        self.terminal_tail = bytearray()  # Bounded, private, never in retained evidence.
        self.report = {"order": order, "cases": [], "cleanup": {}}
        self.provider = Provider()
        try:
            self.launch(command)
        except BaseException:
            self.close()
            raise

    def launch(self, command):
        root, order = self.root, self.order
        self.env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "TERM": "xterm-256color", "LANG": "C.UTF-8",
            "SHELL": "/bin/sh", "HOME": str(root / "home"), "TMPDIR": str(root / "tmp"),
            "PI_CODING_AGENT_DIR": str(root / "agent"), "PI_OFFLINE": "1", "PI_SKIP_VERSION_CHECK": "1",
            "PI_TELEMETRY": "0", "VJ_PI_TRACE": str(self.trace), "VJ_PI_SESSION_CONTROL": str(root / "session-control")}
        for kind in ("CONFIG", "CACHE", "DATA", "STATE"):
            self.env[f"XDG_{kind}_HOME"] = str(root / kind.lower())
        for path in ("home", "tmp", "agent", "config", "cache", "data", "state"):
            (root / path).mkdir(mode=0o700)
        settings = {"defaultProvider": "p0-fixture", "defaultModel": "success", "defaultThinkingLevel": "off",
            "enableInstallTelemetry": False, "enableAnalytics": False, "quietStartup": True, "cacheWarming": "off",
            "tuiMode": "regular", "terminal": {"showImages": False}, "defaultProjectTrust": "never",
            "retry": {"enabled": True, "maxRetries": 1, "baseDelayMs": 10, "maxAgentDelayMs": 1000,
                "provider": {"maxRetries": 0}},
            "compaction": {"enabled": True, "reserveTokens": 4096, "keepRecentTokens": 0,
                "modelOverrides": {"p0-fixture/bad_compaction": {"reserveTokens": -1}}}}
        (root / "agent/settings.json").write_text(json.dumps(settings))
        models = {"providers": {"p0-fixture": {"baseUrl": self.provider.url, "api": "openai-completions",
            "apiKey": "p0-not-a-credential", "models": [{"id": model, "name": model, "reasoning": False,
                "input": ["text"], "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
                "contextWindow": 32000, "maxTokens": 1024,
                "compat": {"supportsStore": False, "supportsDeveloperRole": False, "supportsReasoningEffort": False}}
                for model in MODELS]}}}
        (root / "agent/models.json").write_text(json.dumps(models))
        extensions = [HERE / "observer.ts", HERE / "fixture.ts"]
        if order == "observer-last":
            extensions.reverse()
        args = [*command, "--offline", "--no-extensions", "--no-builtin-tools", "--no-skills",
            "--no-prompt-templates", "--no-context-files", "--no-mcp", "--no-approve",
            "--model", "p0-fixture/success", "--thinking", "off"]
        for extension in extensions:
            args.extend(("--extension", str(extension)))
        master, slave = pty.openpty()
        self.master = master
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 140, 0, 0))
        # No preexec_fn in a threaded parent: controlling-PTY setup happens in
        # an isolated helper which execs Pi. Its PID/process group stays stable.
        helper = "import fcntl,os,sys,termios;fcntl.ioctl(0,termios.TIOCSCTTY,0);os.execvpe(sys.argv[1],sys.argv[1:],os.environ)"
        try:
            self.child = subprocess.Popen([sys.executable, "-c", helper, *args], cwd=root, env=self.env,
                stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
            self.child_birth = birth(self.child.pid)
            self.report["private_launch"] = {"environment_allowlist": sorted(self.env), "no_inherited_credentials": True,
                "isolated_agent_directory": True, "project_resources_disabled": True,
                "controlling_pty": True, "provider_loopback_only": True}
        finally:
            os.close(slave)

    def drain(self):
        while self.master is not None and select.select([self.master], [], [], 0)[0]:
            try:
                data = os.read(self.master, 65536)
            except OSError:
                break
            if not data:
                break
            self.terminal_tail.extend(data)
            del self.terminal_tail[:-16384]
            if b"\x1b[6n" in data:
                os.write(self.master, b"\x1b[1;1R")

    def wait(self, predicate, timeout=15):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.drain()
            value = predicate()
            if value:
                return value
            if self.child is not None and self.child.poll() is not None:
                raise RuntimeError(f"Pi exited ({self.child.returncode}) before expected event")
            time.sleep(.02)
        raise TimeoutError("private Pi event deadline exceeded")

    def events(self):
        return records(self.trace)

    def send(self, text):
        self.drain()
        os.write(self.master, text.encode() + b"\r")

    def action(self, operation, name=None, terminal_event="command_done"):
        start, provider_start = len(self.events()), len(self.provider.observations())
        self.send("/probe " + operation + (" " + name if name else ""))
        dismissed = False
        def done():
            nonlocal dismissed
            trace = self.events()[start:]
            if operation == "idle_ui" and not dismissed and any(e["event"] == "ui_prompt_start" for e in trace):
                time.sleep(.12)
                self.drain()
                os.write(self.master, b"\x1b")
                dismissed = True
            return any(e["source"] == "fixture" and e["event"] == terminal_event for e in trace)
        self.wait(done)
        return self.capture(name or operation, start, provider_start)

    def capture(self, case, start, provider_start=None):
        trace = self.events()[start:]
        # Reindex physical append order. Runtime IDs preserve reload boundaries.
        for index, event in enumerate(trace):
            event["index"] = index
        entry = {"case": case, "trace": trace, "event_counts": dict(Counter(e["event"] for e in trace if e["source"] == "observer"))}
        if provider_start is not None:
            entry["provider_requests"] = self.provider.observations()[provider_start:]
        self.report["cases"].append(entry)
        return entry

    def run_case(self, name):
        start, provider_start = len(self.events()), len(self.provider.observations())
        self.send("/probe run " + name)
        dismissed = set()
        cancelled = False

        def settled():
            nonlocal cancelled
            trace = self.events()[start:]
            observer = [e for e in trace if e["source"] == "observer"]
            if name == "cancel" and any(r["model"] == name for r in self.provider.observations()[provider_start:]) and not cancelled:
                self.drain()
                os.write(self.master, b"\x1b")
                cancelled = True
            if name == "ui":
                for i, event in enumerate(observer):
                    if event["event"] == "ui_prompt_start" and i not in dismissed:
                        # Let the actual dialog render before delivering Escape.
                        time.sleep(.12)
                        self.drain()
                        os.write(self.master, b"\x1b")
                        dismissed.add(i)
            return any(e["event"] == "agent_settled" for e in observer)

        self.wait(settled, timeout=25)
        self.capture(name, start, provider_start)

    def run(self, selected):
        self.wait(lambda: any(e["event"] == "fixture_ready" for e in self.events()))
        # Factory and startup order evidence, before any synthetic agent work.
        self.capture("startup", 0)
        for name in selected:
            self.run_case(name)
        if selected == list(CASES):
            self.action("compact", "manual_compact", "manual_compact_done")
            # Seed fresh content so a second manual attempt actually calls the provider.
            self.run_case("success")
            self.action("compact", "compact_error", "manual_compact_error")
            self.action("idle_ui")
            self.action("name")
            start = len(self.events())
            self.send("/probe tree")
            self.wait(lambda: any(e["event"] == "session_tree" for e in self.events()[start:]))
            self.capture("tree", start)
            for operation, reason in (("new", "new"), ("resume", "resume"), ("fork", "fork"), ("reload", "reload")):
                start = len(self.events())
                self.send("/probe " + operation)
                self.wait(lambda: any(e["source"] == "observer" and e["event"] == "session_start" and e.get("reason") == reason
                    for e in self.events()[start:]))
                self.capture(operation, start)
        start = len(self.events())
        self.send("/probe quit")
        self.wait(lambda: any(e["event"] == "session_shutdown" and e.get("reason") == "quit" for e in self.events()[start:]))
        self.capture("quit", start)
        self.drain()
        self.child.wait(timeout=5)
        return self.report

    def close(self):
        if self.child is not None:
            if self.child.poll() is None and self.child_birth == birth(self.child.pid):
                os.killpg(self.child.pid, signal.SIGTERM)
                try:
                    self.child.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    if self.child_birth == birth(self.child.pid):
                        os.killpg(self.child.pid, signal.SIGKILL)
                    self.child.wait(timeout=3)
            # No fixture subprocess/tool launches exist; the process group is
            # ours. Check that no group member is live after the leader exits.
            live_members = []
            for path in Path("/proc").iterdir():
                if not path.name.isdigit():
                    continue
                try:
                    fields = (path / "stat").read_text().rsplit(")", 1)[1].split()
                    if int(fields[2]) == self.child.pid and fields[0] not in ("Z", "X", "x"):
                        live_members.append(int(path.name))
                except (OSError, ValueError, IndexError):
                    pass
            self.report["cleanup"]["pi_process_group_empty"] = not live_members
        if self.master is not None:
            os.close(self.master)
            self.master = None
        self.report["cleanup"]["provider_stopped"] = self.provider.close()
        self.terminal_tail.clear()


def write_report(report, path):
    # Keep header/case metadata readable, with one compact record per trace line.
    # Token markers are generated locally and can never come from Pi content.
    payload = json.loads(json.dumps(report))
    encoded_events = {}
    for run in payload.get("runs", []):
        traces = [case["trace"] for case in run.get("cases", [])]
        if "partial_trace" in run:
            traces.append(run["partial_trace"])
        for trace in traces:
            for index, event in enumerate(trace):
                marker = f"__P0_TRACE_RECORD_{len(encoded_events)}__"
                encoded_events[json.dumps(marker)] = json.dumps(event, separators=(",", ":"))
                trace[index] = marker
    text = json.dumps(payload, indent=2)
    text = re.sub(r'"__P0_TRACE_RECORD_\d+__"', lambda match: encoded_events[match[0]], text)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pi-bin", default="pi", help="Compiled Pi executable")
    parser.add_argument("--node-cli", type=Path, help="Private npm Pi 1.2.0 dist/cli.js (never installed by this script)")
    parser.add_argument("--node-bin", default="node")
    parser.add_argument("--runtime", choices=("compiled", "node", "both"), default="both")
    parser.add_argument("--order", choices=("observer-first", "observer-last", "both"), default="both")
    parser.add_argument("--cases", nargs="+", choices=CASES, default=list(CASES))
    parser.add_argument("--output", type=Path, default=HERE / "results.json")
    args = parser.parse_args()
    targets = []
    if args.runtime in ("compiled", "both"):
        binary = Path(shutil.which(args.pi_bin) or args.pi_bin).resolve(strict=True)
        targets.append(("compiled", [str(binary)], binary))
    if args.runtime in ("node", "both"):
        if not args.node_cli or not args.node_cli.is_file():
            parser.error("--node-cli must point to the isolated npm Pi dist/cli.js for Node qualification")
        node = Path(shutil.which(args.node_bin) or args.node_bin).resolve(strict=True)
        cli = args.node_cli.resolve(strict=True)
        targets.append(("node", [str(node), str(cli)], cli))
    orders = ("observer-first", "observer-last") if args.order == "both" else (args.order,)
    report = {"schema_version": 1, "scope": "P0 real Pi TUI lifecycle; synthetic provider/test extension; no Zellij ownership or Verij adapter",
        "fixture_hashes": {path.name: sha(path) for path in (HERE / "verify.py", HERE / "provider.py", HERE / "observer.ts", HERE / "fixture.ts", HERE / "trace.mjs")},
        "runs": [], "limitations": ["Separate background extensions are not tracked", "Early exceptions without current-run outcome must remain Unknown",
            "Direct terminal input/external approval is outside extension UI events", "No P1-P4 adapter/navigation/resource acceptance"]}
    try:
        for runtime, command, executable in targets:
            minimal = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "PI_OFFLINE": "1", "PI_TELEMETRY": "0"}
            version = subprocess.check_output([*command, "--version"], env=minimal, text=True, timeout=15).strip()
            if version != "1.2.0":
                raise RuntimeError(f"unqualified {runtime} Pi version {version}")
            for order in orders:
                run = {"runtime": runtime, "version": version, "executable_sha256": sha(executable)}
                report["runs"].append(run)
                with tempfile.TemporaryDirectory(prefix="vj-pi-p0-") as temporary:
                    root = Path(temporary)
                    probe = None
                    failure = None
                    try:
                        probe = Probe(command, root, order)
                        run.update(probe.report)
                        probe.run(args.cases)
                    except Exception as error:
                        # Only exception class goes into retained evidence; no terminal,
                        # provider body, path, prompt, or tool error text.
                        run["failure"] = type(error).__name__
                        if probe is not None:
                            run["partial_trace"] = probe.events()
                        failure = error
                    finally:
                        if probe is not None:
                            probe.close()
                run.setdefault("cleanup", {})["private_directory_removed"] = not root.exists()
                if failure is not None:
                    raise failure
                if not all(run["cleanup"].values()):
                    raise RuntimeError("private probe cleanup incomplete")
                print(f"{runtime} / {order}: {len(run['cases'])} cases; cleanup {all(run['cleanup'].values())}", flush=True)
    except Exception as error:
        print(f"P0 incomplete: {type(error).__name__}", file=sys.stderr)
        return_code = 1
    else:
        return_code = 0
    report["complete"] = return_code == 0 and len(report["runs"]) == len(targets) * len(orders)
    write_report(report, args.output)
    return return_code


if __name__ == "__main__":
    sys.exit(main())
