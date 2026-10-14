"""Private stock-Zellij runtime for P4. No global configuration or credentials."""
from collections import Counter
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(REPO / "prototypes/pi-runtime"))
from provider import MODELS, Provider
from burst_provider import BurstProvider


def birth(pid):
    try:
        return pid, process(pid)["birth"]
    except (OSError, ValueError, IndexError, ProcessLookupError):
        return None


def records(path):
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines(keepends=True) if line.endswith("\n")]


def sha(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def process(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    if fields[0] in ("Z", "X", "x"):
        raise ProcessLookupError(pid)
    return {"birth": int(fields[19]), "ticks": int(fields[11]) + int(fields[12]),
            "rss": int(fields[21]) * os.sysconf("SC_PAGE_SIZE"), "ppid": int(fields[1])}


def opaque(value):
    return hashlib.sha256(str(value).encode()).hexdigest()[:20] if value is not None else None


class Runtime:
    session = "pi-acceptance"

    def __init__(self, cli, plugin, command, seconds=6):
        self.root = Path(tempfile.mkdtemp(prefix="verij-pi-p4-")).resolve()
        self.root.chmod(0o700)
        self.cli, self.plugin = cli.resolve(strict=True), plugin.resolve(strict=True)
        self.zellij = Path(shutil.which("zellij")).resolve(strict=True)
        self.command, self.seconds = command, seconds
        self.pi_probe = Path(command[0])
        if len(command) > 1:
            import shlex
            self.pi_probe = self.root / "pi-entry"
            self.pi_probe.write_text("#!/bin/sh\nexec " + shlex.join(command) + ' "$@"\n')
            self.pi_probe.chmod(0o700)
        self.provider, self.burst_provider = None, None
        self.known, self.calls, self.samples, self.last_samples = {}, Counter(), [], {}
        self.report = {"cases": [], "measurements": [], "cleanup": {}}
        self.env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "TERM": "xterm-256color",
                    "LANG": "C.UTF-8", "SHELL": "/bin/sh", "PI_OFFLINE": "1", "PI_SKIP_VERSION_CHECK": "1",
                    "PI_TELEMETRY": "0"}
        for name in ("home", "tmp", "agent", "states", "control", "agents", "metrics", "sockets"):
            (self.root / name).mkdir(mode=0o700)
        self.env.update(HOME=str(self.root / "home"), TMPDIR=str(self.root / "tmp"),
                        PI_CODING_AGENT_DIR=str(self.root / "agent"),
                        ZELLIJ_SOCKET_DIR=str(self.root / "sockets"),
                        VERIJ_STATES_DIR=str(self.root / "states"), VERIJ_CONTROL_DIR=str(self.root / "control"),
                        VERIJ_AGENT_STATE_DIR=str(self.root / "agents"), VERIJ_WASI_CONTROL_DIR="/host/control",
                        VERIJ_WASI_STATES_DIR="/host/states", VERIJ_RESOURCE_METRICS_DIR=str(self.root / "metrics"))
        for kind in ("CONFIG", "DATA", "CACHE", "STATE", "RUNTIME"):
            path = self.root / kind.lower()
            path.mkdir(mode=0o700)
            self.env["XDG_RUNTIME_DIR" if kind == "RUNTIME" else f"XDG_{kind}_HOME"] = str(path)
        self.tmux = [shutil.which("tmux"), "-f", "/dev/null", "-S", str(self.root / "terminal")]
        self.sessions = [self.session]
        self.terminals = ["seed"]

    def run(self, argv, data=None, timeout=15, check=True):
        result = subprocess.run(list(map(str, argv)), cwd=self.root, env=self.env, input=data,
                                capture_output=True, text=True, timeout=timeout)
        if check and result.returncode:
            # Raw output remains private. Retained evidence contains exception class only.
            raise RuntimeError(f"private command {argv[:3]} failed: {result.stderr[-1500:]} {result.stdout[-500:]}")
        return result.stdout.strip() if check else result

    def mux(self, *args):
        return self.run([*self.tmux, *args])

    def action(self, session, *args):
        self.calls[args[0]] += 1
        return self.run([self.zellij, "-s", session, "action", *args])

    def native(self, *args):
        return json.loads(self.run([self.cli, *args], timeout=45))

    def inventory(self, session=None):
        return json.loads((self.root / f"states/{session or self.session}.json").read_text())["inventory"]

    def states(self):
        values = []
        for path in (self.root / "agents/agents/v1").glob("*/state.json"):
            state = json.loads(path.read_text())
            identity = json.loads(path.with_name("identity.json").read_text())
            if birth(identity["process"]["pid"]) != (identity["process"]["pid"], identity["process"]["start_jiffies"]):
                continue  # Same definitive process-live gate as AgentWatcher.
            state["pane_key"] = identity["pane_key"]
            state["identity"] = identity
            values.append(state)
        return values

    def state(self, pane=0):
        # Attribution uses immutable pane, never working directory or trace PID.
        return next((s for s in self.states() if s["pane_key"]["terminal"] == pane), None)

    def events(self, pane=0):
        return records(self.root / f"trace-{pane}.ndjson")

    def remember(self, pid, role):
        try:
            self.known[pid] = {"birth": process(pid)["birth"], "role": role}
        except (OSError, ProcessLookupError):
            pass

    def discover(self):
        marker = f"ZELLIJ_SOCKET_DIR={self.env['ZELLIJ_SOCKET_DIR']}".encode()
        for directory in Path("/proc").iterdir():
            if not directory.name.isdigit():
                continue
            pid = int(directory.name)
            try:
                if marker not in (directory / "environ").read_bytes().split(b"\0"):
                    continue
                args = (directory / "cmdline").read_bytes().split(b"\0")
                role = "other"
                if b"agent" in args and b"pi" in args:
                    role = "reporter"
                elif b"ui" in args:
                    role = "sidebar"
                elif any(a.endswith(b"/cli.js") for a in args) or str(self.command[0]).encode() == args[0]:
                    role = "pi"
                elif args[0] == str(self.zellij).encode():
                    role = "zellij"
                self.remember(pid, role)
            except (OSError, IndexError):
                pass

    def snapshot(self):
        for state in self.states():
            source = state.get("sources", {}).get("pi", {})
            reduced = state["reduced"]
            sample = {"pane": state["pane_key"]["terminal"], "instance": opaque(state["agent_instance_id"]),
                      "status": reduced["status"], "record_revision": state["record_revision"],
                      "completion_revision": state["completion_revision"],
                      "source_epoch": source.get("source_epoch", 0), "source_revision": source.get("source_revision", 0),
                      "pending_count": len(reduced.get("pending_requests", [])),
                      "observed_settlements": sum(e["event"] == "agent_settled" and e["source"] == "observer"
                                                  for e in self.events(state["pane_key"]["terminal"])),
                      "request_kinds": sorted(r["kind"] for r in reduced.get("pending_requests", []))}
            if self.last_samples.get(sample["instance"]) != sample:
                if len(self.samples) >= 8192:
                    raise RuntimeError("bounded acceptance sample capacity exceeded")
                self.samples.append(sample)
                self.last_samples[sample["instance"]] = sample

    def wait(self, predicate, seconds=20):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                self.snapshot()
                value = predicate()
                if value:
                    return value
            except (OSError, ValueError, KeyError, StopIteration):
                pass
            time.sleep(.025)
        raise TimeoutError("private convergence deadline")

    def send(self, text, pane=0):
        self.action(self.session, "write-chars", "--pane-id", f"terminal_{pane}", text)
        self.action(self.session, "write", "--pane-id", f"terminal_{pane}", "13")

    def escape(self, pane=0):
        self.action(self.session, "write", "--pane-id", f"terminal_{pane}", "27")

    def sidebar(self, host):
        # Inspect only sidebar terminal, not Pi text in the adjacent Workspace.
        return self.action(f"pi-host-{host}", "dump-screen", "--pane-id", "terminal_1")

    def ack(self, host):
        path = self.root / f"state/verij/agent-monitoring/hosts/{host}.json"
        return json.loads(path.read_text())["acknowledgements"] if path.exists() else {}

    def start(self, monitoring=True):
        self.provider = Provider()
        self.burst_provider = BurstProvider()
        root = self.root
        # Settings/model definitions match P0. Monitoring loads through owned autoload,
        # not --extension. Only the active test fixture and trace probe are explicit.
        settings = {"defaultProvider": "p0-fixture", "defaultModel": "success", "defaultThinkingLevel": "off",
                    "enableInstallTelemetry": False, "enableAnalytics": False, "quietStartup": True,
                    "cacheWarming": "off", "tuiMode": "regular", "terminal": {"showImages": False},
                    "defaultProjectTrust": "never", "retry": {"enabled": True, "maxRetries": 1,
                    "baseDelayMs": 10, "maxAgentDelayMs": 1000, "provider": {"maxRetries": 0}},
                    "compaction": {"enabled": True, "reserveTokens": 4096, "keepRecentTokens": 0,
                    "modelOverrides": {"p0-fixture/bad_compaction": {"reserveTokens": -1}}}}
        (root / "agent/settings.json").write_text(json.dumps(settings))
        models = {"providers": {"p0-fixture": {"baseUrl": self.provider.url, "api": "openai-completions",
                  "apiKey": "p4-not-a-credential", "models": [{"id": m, "name": m, "reasoning": False,
                  "input": ["text"], "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
                  "contextWindow": 32000, "maxTokens": 1024, "compat": {"supportsStore": False,
                  "supportsDeveloperRole": False, "supportsReasoningEffort": False}} for m in MODELS]}}}
        models["providers"]["p4-workload"] = {**models["providers"]["p0-fixture"], "baseUrl": self.burst_provider.url,
            "models": [{**models["providers"]["p0-fixture"]["models"][0], "id": "burst", "name": "burst"}]}
        (root / "agent/models.json").write_text(json.dumps(models))
        if monitoring:
            self.native("agent", "setup", "pi", "--config-dir", root / "agent", "--pi-bin", self.pi_probe)
        launch = {"command": self.command, "root": str(root), "repo": str(REPO)}
        (root / "launch.json").write_text(json.dumps(launch))
        url = "file:" + str(self.plugin)
        cache = root / "cache/zellij"
        cache.mkdir(exist_ok=True)
        (cache / "permissions.kdl").write_text("".join(json.dumps(name) +
            " { ReadApplicationState; ChangeApplicationState; ReadCliPipes; }\n" for name in (url, str(self.plugin))))
        base = ('default_shell "/bin/sh"\nshow_startup_tips false\nshow_release_notes false\n'
                'mirror_session false\nsession_serialization false\n' +
                f'load_plugins {{\n {json.dumps(url)} {{\n state_dir "/host/states"\n control_dir "/host/control"\n }}\n}}\n')
        (root / "inner.kdl").write_text(base)
        (root / "outer.kdl").write_text(base + 'default_mode "locked"\nnested_session_handling "descend"\n')
        (root / "inner-layout.kdl").write_text('layout {\n tab name="pi-main" {\n pane command=' +
             json.dumps(sys.executable) + ' { args ' + json.dumps(str(HERE / "launch.py")) + ' ' +
             json.dumps(str(root / "launch.json")) + '; }\n }\n tab name="parking" { pane; }\n}\n')
        self.mux("new-session", "-d", "-s", "seed", "-x", "180", "-y", "55", "-c", str(root),
                 self.zellij, "--config", root / "inner.kdl", "--new-session-with-layout", root / "inner-layout.kdl", "--session", self.session)
        self.wait(lambda: self.inventory())
        # Explicit pane commands avoid attach/create layout differences in stock
        # Zellij. Launch normally from a shell; verify foreground ancestry.
        import shlex
        self.send(shlex.join([sys.executable, str(HERE / "launch.py"), str(root / "launch.json")]))
        self.action(self.session, "new-tab", "--name", "parking")
        if monitoring:
            self.wait(lambda: self.state())
        self.wait(lambda: any(e["event"] == "fixture_ready" for e in self.events()))
        self.discover()

    def hosts(self):
        registry = self.root / "state/verij"
        registry.mkdir(exist_ok=True)
        (registry / "host-registry.toml").write_text('[hosts.pi-host-a]\nmarker_key = "a"\n[hosts.pi-host-b]\nmarker_key = "b"\n')
        for host in ("a", "b"):
            session = f"pi-host-{host}"
            self.sessions.append(session)
            self.terminals.append(f"host-{host}")
            args = ["workspace", self.session, "--host", host, "--plugin", str(self.plugin),
                    "--client-config", str(self.root / "inner.kdl")]
            layout = ('layout {\n pane split_direction="vertical" {\n pane size="28%" name="Verij" command=' +
                      json.dumps(str(self.cli)) + ' { args "ui"; }\n pane name="Workspace" command=' +
                      json.dumps(str(self.cli)) + ' { args ' + ' '.join(json.dumps(a) for a in args) + '; }\n }\n}\n')
            path = self.root / f"host-{host}.kdl"
            path.write_text(layout)
            # Prevent private sidebars racing startup/attach while host is seeded.
            lockdir = self.root / "runtime/verij"
            lockdir.mkdir(exist_ok=True)
            with (lockdir / "zellij-startup.lock").open("a+b") as lock:
                fcntl.flock(lock, fcntl.LOCK_EX)
                self.mux("new-session", "-d", "-s", f"host-{host}", "-x", "180", "-y", "55", "-c", str(self.root),
                         self.zellij, "--config", self.root / "outer.kdl", "--new-session-with-layout", path, "--session", session)
                self.wait(lambda: self.inventory(session))
                self.action(session, "new-tab", "--layout", str(path))
                self.wait(lambda: len(self.inventory(session)["panes"]) >= 3)
                self.action(session, "close-pane", "--pane-id", "terminal_0")
            marker = self.wait(lambda: json.loads((self.root / f"control/attachment-{host}.json").read_text()))
            self.native("navigate", "register", "--host", host, "--host-session", session, "--workspace-pane", "2",
                        "--client-pid", marker["attachment_process"]["pid"], "--session", self.session, "--plugin", self.plugin)
            # Leave both hosts on parking tab so Done remains unread.
            self.native("navigate", "focus", "--host", host, "--session", self.session, "--pane", "1")
        self.mux("kill-session", "-t", "seed")
        self.discover()

    def metrics(self):
        totals = Counter()
        for path in (self.root / "metrics").glob("native-*.json"):
            value = json.loads(path.read_text())["counters"]
            totals.update({f"cli/{k}": v for k, v in value.pop("cli").items()})
            totals.update(value)
        return totals

    def measure(self, phase, seconds=None, drive=None):
        self.discover()
        start, counters = time.monotonic(), self.metrics()
        first = {pid: process(pid) for pid in self.known if birth(pid)}
        peak = {pid: v["rss"] for pid, v in first.items()}
        while time.monotonic() - start < (seconds or self.seconds):
            self.snapshot()
            if drive:
                drive()
            for pid, initial in first.items():
                if birth(pid) != (pid, initial["birth"]):
                    continue
                value = process(pid)
                peak[pid] = max(peak[pid], value["rss"])
                if peak[pid] > 768 * 2**20:
                    raise RuntimeError("private RSS guard exceeded")
            time.sleep(.025)
        elapsed = time.monotonic() - start
        values = []
        for pid, initial in first.items():
            if birth(pid) != (pid, initial["birth"]):
                continue
            last = process(pid)
            values.append({"role": self.known[pid]["role"], "cpu_percent": round(
                (last["ticks"] - initial["ticks"]) / os.sysconf("SC_CLK_TCK") / elapsed * 100, 3),
                "rss_mib": round(last["rss"] / 2**20, 3), "rss_delta_mib": round((last["rss"] - initial["rss"]) / 2**20, 3),
                "peak_rss_mib": round(peak[pid] / 2**20, 3)})
        entry = {"phase": phase, "seconds": round(elapsed, 3), "processes": values,
                 "counters": dict(self.metrics() - counters)}
        self.report["measurements"].append(entry)
        return entry

    def close(self):
        self.discover()
        for session in self.sessions:
            self.run([self.zellij, "kill-session", session], check=False)
        self.run([*self.tmux, "kill-server"], check=False)
        deadline = time.monotonic() + 5
        while any(birth(pid) == (pid, i["birth"]) for pid, i in self.known.items()) and time.monotonic() < deadline:
            time.sleep(.1)
        forced = 0
        for pid, identity in self.known.items():
            if birth(pid) == (pid, identity["birth"]):
                os.kill(pid, signal.SIGKILL)
                forced += 1
        time.sleep(.1)
        self.report["cleanup"] = {"private_processes_exited": not any(
            birth(pid) == (pid, i["birth"]) for pid, i in self.known.items()), "forced_kills": forced,
            "provider_stopped": self.provider.close() if self.provider else True,
            "burst_provider_stopped": self.burst_provider.close() if self.burst_provider else True}
        self.report["harness_cli_calls"] = dict(self.calls)
        shutil.rmtree(self.root)
        self.report["cleanup"]["private_directory_removed"] = not self.root.exists()
