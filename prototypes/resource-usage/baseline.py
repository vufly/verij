"""Matched private real-sidebar/native-reporter baseline, with bounded summaries."""
from collections import Counter, deque
from contextlib import contextmanager
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
from typing import Any

AGENT = Path(__file__).with_name("agent.mjs").resolve()
REPO = Path(__file__).resolve().parents[2]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def proc(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    if fields[0] == "Z":
        raise ProcessLookupError(pid)
    return {"birth": int(fields[19]), "ticks": int(fields[11]) + int(fields[12]),
            "rss": int(fields[21]) * os.sysconf("SC_PAGE_SIZE")}


class Experiment:
    def __init__(self, args, mode):
        self.args, self.mode = args, mode
        self.root = Path(tempfile.mkdtemp(prefix="verij-baseline-", dir=args.scratch_dir)).resolve()
        self.root.chmod(0o700)
        binary = shutil.which("zellij")
        if binary is None:
            raise RuntimeError("stock zellij is not installed")
        self.binary = Path(binary).resolve(strict=True)
        self.cli = args.cli.resolve(strict=True)
        self.plugin = args.plugin.resolve(strict=True)
        self.status = args.status_plugin.resolve(strict=True) if args.status_plugin else None
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(("ZELLIJ", "TMUX", "VERIJ", "OPENCODE", "MAGY"))}
        for kind in ("CONFIG", "DATA", "CACHE", "STATE", "RUNTIME"):
            directory = self.root / kind.lower()
            directory.mkdir(mode=0o700)
            self.env["XDG_RUNTIME_DIR" if kind == "RUNTIME" else f"XDG_{kind}_HOME"] = str(directory)
        for name in ("states", "control", "agents", "metrics", "sockets", "home"):
            (self.root / name).mkdir(mode=0o700)
        self.env.update(TERM="xterm-256color",
                        ZELLIJ_SOCKET_DIR=str(self.root / "sockets"),
                        VERIJ_STATES_DIR=str(self.root / "states"), VERIJ_CONTROL_DIR=str(self.root / "control"),
                        VERIJ_AGENT_STATE_DIR=str(self.root / "agents"), VERIJ_WASI_CONTROL_DIR="/host/control",
                        VERIJ_WASI_STATES_DIR="/host/states", VERIJ_RESOURCE_METRICS_DIR=str(self.root / "metrics"))
        self.tmux = ["tmux", "-f", "/dev/null", "-S", str(self.root / "terminal")]
        self.hosts = ["a", "b"][:args.hosts]
        self.sessions = ["resource-shared", *[f"resource-host-{h}" for h in self.hosts]]
        self.known = {}
        self.runtime = None
        self.harness_calls = Counter()
        self.report = {"status": "FAIL", "root": str(self.root), "monitoring": mode,
                       "measurement_kind": ("real stock Zellij, sidebars, visit protocol and native reporter; "
                                            + ("real OpenCode API/events with controlled SSE provider" if args.agent_runtime == "opencode" else "synthetic API/events")),
                       "control_definition": "exporters/status bars/attachments retained; sidebar replaced by sleep and bridge disabled",
                       "hosts": args.hosts, "tabs": args.tabs, "warmup_seconds": args.warmup,
                       "workload_seconds": args.seconds, "max_rss_mib": args.max_rss_mib,
                       "max_agent_rss_mib": args.max_agent_rss_mib,
                       "stock_binary": str(self.binary), "stock_version": self.run([self.binary, "--version"]).strip(),
                       "stock_sha256": digest(self.binary),
                       "process_scope": "long-lived private servers, clients, pane processes and reporters; transient CLI work counted separately",
                       "assets": {}, "measurements": []}
        assets = {"cli": self.cli, "exporter_tested": self.plugin,
                  "bridge": REPO / "integrations/opencode/bridge.mjs", "core": REPO / "integrations/opencode/core.mjs"}
        if self.status:
            assets["status_bar"] = self.status
        for name, path in assets.items():
            self.report["assets"][name] = {"path": str(path), "sha256": digest(path)}
        for name, path in {"installed_cli": Path(shutil.which("verij") or "/nonexistent"),
                           "installed_exporter": Path(os.environ.get("HOME", "")) / ".local/share/verij/verij_plugin.wasm",
                           "installed_bridge": Path(os.environ.get("OPENCODE_CONFIG_DIR", str(Path(os.environ.get("HOME", "")) / ".config/opencode"))) / "verij-monitoring/bridge.mjs",
                           "installed_core": Path(os.environ.get("OPENCODE_CONFIG_DIR", str(Path(os.environ.get("HOME", "")) / ".config/opencode"))) / "verij-monitoring/core.mjs"}.items():
            if path.is_file():
                self.report["assets"][name] = {"path": str(path), "sha256": digest(path)}

    def run(self, argv, timeout=10, data=None):
        result = subprocess.run(list(map(str, argv)), cwd=self.root, env=self.env, input=data,
                                capture_output=True, text=True, timeout=timeout)
        if result.returncode:
            raise RuntimeError(f"{argv[:5]}: {result.stderr[-1500:]} {result.stdout[-500:]}")
        return result.stdout.strip()

    def mux(self, *args):
        return self.run(self.tmux + list(args))

    def action(self, session, *args):
        self.harness_calls[f"{session}:{args[0]}"] += 1
        return self.run([self.binary, "-s", session, "action", *args])

    def native(self, *args):
        return json.loads(self.run([self.cli, *args], timeout=45))

    @contextmanager
    def startup_guard(self):
        directory = self.root / "runtime/verij"
        directory.mkdir(exist_ok=True)
        with (directory / "zellij-startup.lock").open("a+b") as lock:
            deadline = time.monotonic() + 5
            while True:
                try:
                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    break
                except BlockingIOError:
                    if time.monotonic() >= deadline:
                        raise TimeoutError("private startup lock acquisition")
                    time.sleep(.01)
            try:
                yield
            finally:
                fcntl.flock(lock, fcntl.LOCK_UN)

    def wait(self, predicate, seconds=35):
        deadline = time.monotonic() + seconds
        last = None
        while time.monotonic() < deadline:
            self.resources()  # RSS guard applies during setup/convergence too.
            try:
                value = predicate()
                if value:
                    return value
            except (OSError, ValueError, KeyError) as error:
                last = error
            time.sleep(.1)
        raise TimeoutError(f"convergence at {self.report.get('stage', 'startup')}: {last}")

    def inventory(self, session) -> Any:
        records = self.native("inventory", "--session", session)
        return records[0].get("inventory") if records else None

    def connected_clients(self):
        return json.loads((self.root / "states/resource-shared.json").read_text()).get("connected_clients")

    def remember(self, pid, role):
        self.known[pid] = {"role": role, "birth": proc(pid)["birth"]}

    def resources(self, enforce=True):
        values = {}
        for pid, identity in list(self.known.items()):
            try:
                value = proc(pid)
            except (OSError, ProcessLookupError):
                continue
            if value["birth"] != identity["birth"]:
                continue
            limit = (self.args.max_agent_rss_mib or self.args.max_rss_mib) if identity["role"] in ("opencode_server", "opencode_tui") else self.args.max_rss_mib
            if enforce and value["rss"] > limit * 2**20:
                self.report["rss_abort"] = {"pid": pid, "role": identity["role"], "rss_mib": value["rss"] / 2**20, "limit_mib": limit}
                raise RuntimeError(f"RSS abort: {identity['role']} {pid} exceeds {limit} MiB")
            values[pid] = value
        return values

    def metrics(self):
        totals = Counter()
        for file in (self.root / "metrics").glob("native-*.json"):
            value = json.loads(file.read_text())
            counters = value["counters"]
            totals.update({f"cli/{k}": v for k, v in counters.pop("cli").items()})
            totals.update(counters)
            pid = value["pid"]
            try:
                command = Path(f"/proc/{pid}/cmdline").read_bytes()
                role = "sidebar" if b"ui\0" in command else "native_reporter"
                if pid not in self.known:
                    self.remember(pid, role)
                else:
                    self.known[pid]["role"] = role
            except (OSError, ProcessLookupError):
                pass
        path = self.root / "metrics/api.json"
        if path.exists():
            value = json.loads(path.read_text())
            if "error" in value:
                raise RuntimeError(f"OpenCode resource plugin: {value['error']}")
            totals.update({f"api/{k}": v for k, v in value["endpoints"].items()})
            totals["owned_events"] = value["events"]
            if value["pid"] not in self.known:
                self.remember(value["pid"], "opencode_tui" if self.runtime else "synthetic_api_agent")
            elif self.runtime:
                self.known[value["pid"]]["role"] = "opencode_tui"
        return totals

    def set_workload(self, rate, complete=False, turn=1):
        path = self.root / "workload.json"
        temporary = path.with_suffix(".tmp")
        temporary.write_text(json.dumps({"events_per_second": rate, "complete": complete, "turn": turn}))
        temporary.replace(path)

    def measure(self, phase, seconds, retain=True, initial_counters=None):
        self.metrics()
        first = self.resources()
        counters = self.metrics() if initial_counters is None else initial_counters
        start, previous_at = time.monotonic(), time.monotonic()
        previous = counters
        intervals = deque(maxlen=60)
        peaks = {pid: value["rss"] for pid, value in first.items()}
        aborted = None
        while time.monotonic() - start < seconds:
            time.sleep(min(.25, max(0, seconds - (time.monotonic() - start))))
            try:
                current = self.resources()
            except RuntimeError as error:
                aborted = str(error)
                current = self.resources(enforce=False)
            for pid, value in current.items():
                peaks[pid] = max(peaks.get(pid, 0), value["rss"])
            now = time.monotonic()
            if now - previous_at >= 1:
                latest = self.metrics()
                intervals.append({"offset_seconds": round(now - start, 2),
                                  "seconds": round(now - previous_at, 2),
                                  "counters": dict(latest - previous),
                                  "total_rss_mib": round(sum(v["rss"] for v in current.values()) / 2**20, 2)})
                previous, previous_at = latest, now
            if aborted:
                break
        last = self.resources(enforce=False)
        elapsed = time.monotonic() - start
        delta = self.metrics() - counters
        processes = []
        for pid, value in last.items():
            initial = first.get(pid, value)
            processes.append({"pid": pid, "role": self.known[pid]["role"],
                              "cpu_percent": round((value["ticks"] - initial["ticks"]) / os.sysconf("SC_CLK_TCK") / elapsed * 100, 2),
                              "rss_mib": round(value["rss"] / 2**20, 2),
                              "rss_delta_mib": round((value["rss"] - initial["rss"]) / 2**20, 2),
                              "peak_rss_mib": round(peaks[pid] / 2**20, 2)})
        if retain or aborted:
            self.report["measurements"].append({"phase": phase, "elapsed_seconds": round(elapsed, 3),
                "aborted": aborted,
                "processes": processes, "total_cpu_percent": round(sum(p["cpu_percent"] for p in processes), 2),
                "total_rss_mib": round(sum(p["rss_mib"] for p in processes), 2),
                "counters": dict(delta), "rates_per_second": {k: round(v / elapsed, 3) for k, v in delta.items()},
                "intervals": list(intervals), "interval_capacity": 60, "counter_snapshot_resolution_seconds": 1})
        if aborted:
            raise RuntimeError(aborted)

    def start(self):
        root = self.root
        url = "file:" + str(self.plugin)
        cache = root / "cache/zellij"
        cache.mkdir(exist_ok=True)
        permissions = "".join(json.dumps(name) + " { ReadApplicationState; ChangeApplicationState; ReadCliPipes; }\n"
                              for name in (url, str(self.plugin)))
        staged_status = root / "status.wasm"
        if self.status:
            shutil.copyfile(self.status, staged_status)
            permissions += "".join(json.dumps(name) + " { ReadApplicationState; ChangeApplicationState; RunCommands; }\n"
                                   for name in ("file:" + str(staged_status), str(staged_status)))
        (cache / "permissions.kdl").write_text(permissions)
        base = ('default_shell "/bin/sh"\nshow_startup_tips false\nshow_release_notes false\n'
                'mirror_session false\nsession_serialization false\n'
                f'load_plugins {{\n {json.dumps(url)} {{\n state_dir "/host/states"\n control_dir "/host/control"\n }}\n}}\n')
        (root / "inner.kdl").write_text(base)
        (root / "outer.kdl").write_text(base + 'default_mode "locked"\nnested_session_handling "descend"\n')
        registry = root / "state/verij"
        registry.mkdir()
        (registry / "host-registry.toml").write_text("".join(
            f'[hosts.resource-host-{h}]\nmarker_key = "{h}"\n' for h in self.hosts))
        self.set_workload(0)
        agent = [shutil.which("node"), AGENT, root, self.cli, self.mode]
        if self.args.agent_runtime == "opencode":
            from provider import Runtime
            self.runtime = Runtime(self)
            agent = self.runtime.command()
        status_bar = (f'pane size=1 borderless=true {{\n plugin location={json.dumps("file:" + str(staged_status))} {{\n'
                      ' format_left "{mode} {session}"\n format_center "{tabs}"\n format_right "{datetime}"\n'
                      ' datetime_format "%H:%M:%S"\n }\n }\n' if self.status else "")
        tabs = []
        for index in range(self.args.tabs):
            pane = (f'pane command={json.dumps(str(agent[0]))} {{\n args '
                    + ' '.join(json.dumps(str(arg)) for arg in agent[1:]) + ';\n}\n') if index == 0 else 'pane;\n'
            tabs.append(f'tab name="probe-{index}" {{\n{pane}{status_bar}}}\n')
        (root / "inner-layout.kdl").write_text("layout {\n" + "".join(tabs) + "}\n")
        command = [self.binary, "--config", root / "inner.kdl", "--layout", root / "inner-layout.kdl", "attach", "-c", "resource-shared"]
        self.mux("new-session", "-d", "-s", "seed", "-x", "180", "-y", "55", "-c", str(root), *map(str, command))
        self.remember(int(self.mux("display-message", "-p", "-t", "seed", "#{pid}")), "tmux_server")
        self.remember(int(self.mux("display-message", "-p", "-t", "seed", "#{pane_pid}")), "seed_client")
        inv = self.wait(lambda: self.inventory("resource-shared"))
        assert inv is not None
        self.remember(inv["producer"]["server_pid"], "inner_server")
        self.wait(lambda: (root / "metrics/api.json").exists())
        if self.runtime:
            self.wait(lambda: json.loads((root / "metrics/api.json").read_text()).get("ready"))
        self.metrics()
        pane = self.action("resource-shared", "new-pane").strip()
        self.wait(lambda: len(self.inventory("resource-shared")["panes"]) == self.args.tabs + 1)
        self.action("resource-shared", "close-pane", "--pane-id", pane)
        self.wait(lambda: len(self.inventory("resource-shared")["panes"]) == self.args.tabs)
        self.report["topology_refresh"] = "PASS"
        for host in self.hosts:
            self.report["stage"] = f"host-{host} startup"
            workspace = ["workspace", "resource-shared", "--host", host, "--plugin", self.plugin, "--client-config", root / "inner.kdl"]
            sidebar = (f'pane size="28%" name="Verij" command={json.dumps(str(self.cli))} {{ args "ui"; }}\n'
                       if self.mode == "enabled" else 'pane size="28%" name="Verij" command="sleep" { args "86400"; }\n')
            layout = ('layout {\n pane split_direction="vertical" {\n' + sidebar
                      + f'pane name="Workspace" command={json.dumps(str(self.cli))} {{ args '
                      + ' '.join(json.dumps(str(arg)) for arg in workspace) + ';\n}\n}\n}\n')
            path = root / f"host-{host}.kdl"
            path.write_text(layout)
            command = [self.binary, "--config", root / "outer.kdl", "--layout", path, "attach", "-c", f"resource-host-{host}"]
            with self.startup_guard():
                self.mux("new-session", "-d", "-s", f"host-{host}", "-x", "180", "-y", "55", "-c", str(root), *map(str, command))
                self.remember(int(self.mux("display-message", "-p", "-t", f"host-{host}", "#{pane_pid}")), "outer_client")
                inv = self.wait(lambda: self.inventory(f"resource-host-{host}"))
                assert inv is not None
                self.remember(inv["producer"]["server_pid"], "host_server")
            marker = self.wait(lambda: json.loads((root / f"control/attachment-{host}.json").read_text()))
            self.remember(marker["attachment_process"]["pid"], "inner_attachment")
            self.native("navigate", "register", "--host", host, "--host-session", f"resource-host-{host}",
                        "--workspace-pane", "1", "--client-pid", marker["attachment_process"]["pid"],
                        "--session", "resource-shared", "--plugin", self.plugin)
            self.native("navigate", "focus", "--host", host, "--session", "resource-shared", "--pane", "0")
        self.mux("kill-session", "-t", "seed")
        self.wait(lambda: self.connected_clients() == self.args.hosts)
        topology = {}
        for session in self.sessions:
            inv = self.inventory(session)
            for pane in inv["panes"]:
                if pane.get("pane_pid") and pane["pane_pid"] not in self.known:
                    self.remember(pane["pane_pid"], "pane_process")
            panes = json.loads(self.action(session, "list-panes", "--all", "--json"))
            clients = self.action(session, "list-clients")
            topology[session] = {"server_pid": inv["producer"]["server_pid"],
                                 "terminal_panes": sum(not p["is_plugin"] for p in panes),
                                 "plugin_panes": sum(p["is_plugin"] for p in panes),
                                 "status_bar_instances": sum(p["is_plugin"] and "status.wasm" in json.dumps(p) for p in panes),
                                 "tabs": len({p["tab_id"] for p in panes}),
                                 "clients": sum(bool(line.split()) and line.split()[0].isdigit() for line in clients.splitlines())}
        self.report["topology"] = topology
        if self.mode == "enabled":
            self.wait(lambda: any((root / "agents/agents/v1").glob("*/state.json")))
            self.metrics()
            if not any(v["role"] == "sidebar" for v in self.known.values()):
                raise RuntimeError("real sidebar counters not present")
        self.measure("warmup", self.args.warmup, retain=False)
        self.report.pop("stage", None)

    def execute(self):
        self.start()
        self.measure("idle_agent", self.args.seconds)
        for rate in (20, 100):
            self.set_workload(rate)
            if self.runtime:
                self.runtime.prompt()
            workload = "provider_chunks" if self.runtime else "owned_events"
            self.measure(f"{rate}_{workload}_per_second", self.args.seconds)
        if self.args.verify_p1_visits and self.mode == "enabled":
            self.verify_passive_visits()
        else:
            self.set_workload(0, complete=True)
            self.measure("completion_visit", max(3, self.args.seconds))
        if self.mode == "enabled":
            states = list((self.root / "agents/agents/v1").glob("*/state.json"))
            revisions = {}
            for path in states:
                state = json.loads(path.read_text())
                revisions[state["agent_instance_id"]] = state["completion_revision"]
            acknowledgements = {}
            for host in self.hosts:
                path = self.root / f"state/verij/agent-monitoring/hosts/{host}.json"
                acknowledgements[host] = json.loads(path.read_text())["acknowledgements"] if path.exists() else {}
            self.report["completion_visit"] = {"completion_revisions": revisions, "host_acknowledgements": acknowledgements}
            if not revisions or not all(r > 0 for r in revisions.values()):
                raise RuntimeError("controlled completion was not observed")
            if not all(all(acks.get(instance, {}).get("acknowledged_revision", 0) >= revision
                           for instance, revision in revisions.items()) for acks in acknowledgements.values()):
                raise RuntimeError("passive host-local completion acknowledgement was not observed")
        for host in self.hosts:
            self.mux("kill-session", "-t", f"host-{host}")
            self.run([self.binary, "kill-session", f"resource-host-{host}"])
        self.wait(lambda: self.connected_clients() == 0)
        self.report["detachment"] = "PASS"
        self.measure("idle_detached", self.args.seconds)
        self.report["status"] = "PASS"

    def verify_passive_visits(self):
        assert self.args.agent_runtime == "synthetic"
        self.report["p1_visits"] = True
        host = self.hosts[0]
        self.set_workload(0)
        binding = json.loads((self.root / f"control/binding-host-{host}.json").read_text())
        context = binding["context"]
        path = self.root / ("control/focus-" + "-".join(str(context[k])
                           for k in ("server_pid", "plugin_id", "client_id", "epoch")) + ".json")
        def inner_terminal():
            return json.loads(path.read_text()).get("terminal")
        def acked(observer):
            path = self.root / f"state/verij/agent-monitoring/hosts/{observer}.json"
            return json.loads(path.read_text())["acknowledgements"] if path.exists() else {}
        def proof_calls(measurement):
            return sum(v for k, v in measurement["counters"].items()
                       if k.startswith("cli/") and k.endswith((":pipe", ":list-clients")))
        # Actual keyboard navigation in the requesting host's owned inner client.
        self.mux("send-keys", "-t", f"host-{host}", "C-t", "2")
        self.wait(lambda: inner_terminal() == 1)
        self.action(f"resource-host-{host}", "focus-pane-id", "terminal_0")
        self.set_workload(0, complete=True)
        self.measure("completion_sidebar", max(3, self.args.seconds))
        assert not acked(host), "sidebar focus consumed Done"
        assert all(acked(peer) for peer in self.hosts[1:]), "independent focused peer did not acknowledge"
        sidebar = self.report["measurements"][-1]
        assert sidebar["counters"].get("proof_attempts", 0) == len(self.hosts) - 1
        assert proof_calls(sidebar) == 5 * (len(self.hosts) - 1)

        self.action(f"resource-host-{host}", "focus-pane-id", "terminal_1")
        self.measure("completion_other_pane", max(3, self.args.seconds))
        other = self.report["measurements"][-1]
        assert not acked(host), "another inner pane consumed Done"
        assert other["counters"].get("proof_attempts", 0) == proof_calls(other) == 0

        manual_start = self.metrics()
        self.mux("send-keys", "-t", f"host-{host}", "C-t", "1")
        self.wait(lambda: inner_terminal() == 0)
        self.measure("manual_completion_visit", max(3, self.args.seconds), initial_counters=manual_start)
        manual = self.report["measurements"][-1]
        assert acked(host), "manual inner navigation did not acknowledge"
        assert manual["counters"].get("proof_attempts", 0) == 1
        assert proof_calls(manual) == 5, "successful passive acknowledgement repeated native proof"
        # A second actual outer display must invalidate native proof authority.
        # Passive failures stay coalesced/backed off; cached hints cannot replace
        # the fresh single-display list-clients checks.
        command = [self.binary, "--config", self.root / "outer.kdl", "attach", f"resource-host-{host}"]
        self.mux("new-session", "-d", "-s", "second-display", "-x", "180", "-y", "55", "-c", str(self.root), *map(str, command))
        self.remember(int(self.mux("display-message", "-p", "-t", "second-display", "#{pane_pid}")), "second_outer_client")
        self.set_workload(0, complete=True, turn=2)
        self.measure("completion_multiple_displays", max(8, self.args.seconds))
        multiple = self.report["measurements"][-1]
        failed = multiple["counters"].get(f"cli/resource-host-{host}:list-clients", 0)
        assert 1 <= failed <= 3, "failed proof attempts escaped exponential backoff"
        assert multiple["counters"].get(f"cli/resource-host-{host}:pipe", 0) == 0
        assert multiple["counters"].get("proof_attempts", 0) == failed + len(self.hosts) - 1
        assert all(ack["acknowledged_revision"] == 1 for ack in acked(host).values())
        assert all(all(ack["acknowledged_revision"] == 2 for ack in acked(peer).values()) for peer in self.hosts[1:])
        self.report["failed_proof_attempts"] = failed
        self.mux("kill-session", "-t", "second-display")
        outer_context = json.loads((self.root / f"control/outer-journal-{host}.json").read_text())["context"]
        outer_path = self.root / ("control/focus-" + "-".join(str(outer_context[k])
                              for k in ("server_pid", "plugin_id", "client_id", "epoch")) + ".json")
        self.action(f"resource-host-{host}", "focus-pane-id", "terminal_0")
        self.wait(lambda: json.loads(outer_path.read_text()).get("terminal") == 0)
        self.measure("focus_backoff_reset", 1, retain=False)
        recovery_start = self.metrics()
        self.action(f"resource-host-{host}", "focus-pane-id", "terminal_1")
        self.measure("single_display_recovery", max(3, self.args.seconds), initial_counters=recovery_start)
        recovery = self.report["measurements"][-1]
        assert all(ack["acknowledged_revision"] == 2 for ack in acked(host).values())
        assert recovery["counters"].get("proof_attempts", 0) == 1 and proof_calls(recovery) == 5
        for measurement in self.report["measurements"][:3]:
            assert measurement["counters"].get("proof_attempts", 0) == 0
            assert proof_calls(measurement) == 0
        self.report["passive_visit_checks"] = {"idle_and_working_no_proofs": True,
            "sidebar_does_not_acknowledge": True, "other_pane_no_proofs": True,
            "peer_acknowledgements_independent": True, "manual_inner_navigation_acknowledges": True,
            "one_five_call_proof_per_host": True, "multiple_displays_reject_acknowledgement": True,
            "failed_proofs_back_off": True, "meaningful_focus_resets_backoff": True}

    def close(self):
        # Also discover children when an early setup abort precedes exporter/metric
        # publication. Match the exact private socket root, then retain process birth.
        marker = f"ZELLIJ_SOCKET_DIR={self.env['ZELLIJ_SOCKET_DIR']}".encode()
        for directory in Path("/proc").iterdir():
            if not directory.name.isdigit() or int(directory.name) in self.known:
                continue
            try:
                if marker in (directory / "environ").read_bytes().split(b"\0"):
                    self.remember(int(directory.name), "cleanup_discovered")
            except (OSError, ProcessLookupError):
                pass
        for session in self.sessions:
            try:
                subprocess.run([str(self.binary), "kill-session", session], cwd=self.root, env=self.env,
                               capture_output=True, timeout=8)
            except subprocess.TimeoutExpired:
                pass
        try:
            subprocess.run(self.tmux + ["kill-server"], env=self.env, capture_output=True, timeout=8)
        except subprocess.TimeoutExpired:
            pass
        if self.runtime:
            self.runtime.close()
        def alive(pid, identity):
            try:
                return proc(pid)["birth"] == identity["birth"]
            except (OSError, ProcessLookupError):
                return False
        deadline = time.monotonic() + 5
        while any(alive(pid, v) for pid, v in self.known.items()) and time.monotonic() < deadline:
            time.sleep(.1)
        remaining = [pid for pid, v in self.known.items() if alive(pid, v)]
        # Only terminate retained, birth-qualified private processes after graceful cleanup.
        for pid in remaining:
            if alive(pid, self.known[pid]):
                os.kill(pid, 9)
        time.sleep(.1)
        remaining = [pid for pid, v in self.known.items() if alive(pid, v)]
        self.report["cleanup"] = {"known_processes_exited": not remaining, "remaining_pids": remaining}
        if remaining:
            self.report.update(status="FAIL", error="private processes remain")
        self.report["harness_cli_calls"] = dict(self.harness_calls)
        (self.root / "results.json").write_text(json.dumps(self.report, indent=2) + "\n")


def run_baseline(args):
    if args.verify_p1_visits and (args.agent_runtime != "synthetic" or args.tabs < 2):
        raise ValueError("P1 visit verification needs synthetic producer and at least two tabs")
    if not (args.seconds > 0 and args.warmup >= 0 and args.tabs > 0 and args.max_rss_mib > 0
            and all(math.isfinite(v) for v in (args.seconds, args.warmup, args.max_rss_mib))
            and (args.max_agent_rss_mib is None or (args.max_agent_rss_mib > 0 and math.isfinite(args.max_agent_rss_mib)))):
        raise ValueError("positive seconds/tabs/RSS required; warmup must be nonnegative")
    modes = ("disabled", "enabled") if args.monitoring == "both" else (args.monitoring,)
    reports = []
    for mode in modes:
        experiment = Experiment(args, mode)
        try:
            experiment.execute()
        except Exception as error:
            experiment.report["error"] = str(error)
            screens = {}
            for terminal in ("seed", *[f"host-{h}" for h in experiment.hosts]):
                try:
                    screens[terminal] = experiment.mux("capture-pane", "-p", "-t", terminal)[-4096:]
                except (RuntimeError, OSError, subprocess.SubprocessError):
                    pass
            experiment.report["failure_screens"] = screens
        finally:
            experiment.close()
        reports.append(experiment.report)
    report = {"status": "PASS" if all(r["status"] == "PASS" for r in reports) else "FAIL", "experiments": reports}
    text = json.dumps(report, indent=2) + "\n"
    args.output.write_text(text)
    print(json.dumps({"status": report["status"], "output": str(args.output),
                      "errors": [r.get("error") for r in reports if r["status"] != "PASS"]}) if args.quiet else text)
    return 0 if report["status"] == "PASS" else 1
