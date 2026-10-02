#!/usr/bin/env python3
"""Drive installed OpenCode against controlled HTTP provider responses in private storage.

Provider output is synthetic. Session execution, TUI-local callbacks, native
task children, cancellation/retry and reconnect snapshots are actual runtime.
No user credentials, global configuration or existing sessions are used.
"""
import argparse
from collections import Counter
import fcntl
import hashlib
import importlib.util
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import pty
import select
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time
from typing import Any
import urllib.error
import urllib.request
import uuid

spec = importlib.util.spec_from_file_location("bridge_verify", Path(__file__).resolve().parents[1] / "zellij-attach-bridge/verify.py")
assert spec and spec.loader
bridge = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bridge)
process_identity = bridge.process_identity


def key(value):
    return hashlib.sha256(value.encode()).hexdigest()[:20] if value else None


class Provider:
    def __init__(self):
        self.counts = Counter()
        self.entered = {name: threading.Event() for name in ("success", "retry", "error", "recovery", "cancel", "family", "child", "title")}
        self.gates = {name: threading.Event() for name in self.entered}
        for name in ("success", "error", "family", "title"):
            self.gates[name].set()
        fixture = self
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, format, *args):
                pass

            def handle(self):
                try:
                    super().handle()
                except (BrokenPipeError, ConnectionResetError):
                    pass  # Expected when OpenCode cancels a controlled request.

            def do_POST(self):
                data = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                model = data["model"]
                fixture.counts[model] += 1
                fixture.entered[model].set()
                if model == "error" or (model == "retry" and fixture.counts[model] <= 4):
                    self.send_response(400 if model == "error" else 429)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Retry-After", "1")
                    self.end_headers()
                    self.wfile.write(json.dumps({"error": {"message": "controlled provider fixture", "type": "rate_limit" if model == "retry" else "invalid_request"}}).encode())
                    return
                if not fixture.gates[model].wait(75):
                    return
                task = model == "family" and not any(item["role"] == "tool" for item in data["messages"])
                delta = {"role": "assistant", "content": "OK"}
                finish = "stop"
                if task:
                    delta = {"role": "assistant", "tool_calls": [{"index": 0, "id": "call_verij_child", "type": "function",
                        "function": {"name": "task", "arguments": json.dumps({"description": "Controlled child probe", "prompt": "Return OK", "subagent_type": "general"})}}]}
                    finish = "tool_calls"
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream" if data.get("stream") else "application/json")
                self.end_headers()
                try:
                    if data.get("stream"):
                        for change, reason in ((delta, None), ({}, finish)):
                            payload = {"id": "chatcmpl_probe", "object": "chat.completion.chunk", "created": int(time.time()),
                                       "model": model, "choices": [{"index": 0, "delta": change, "finish_reason": reason}]}
                            self.wfile.write(b"data: " + json.dumps(payload).encode() + b"\n\n")
                            self.wfile.flush()
                        self.wfile.write(b"data: [DONE]\n\n")
                    else:
                        self.wfile.write(json.dumps({"id": "chatcmpl_probe", "object": "chat.completion", "created": int(time.time()),
                            "model": model, "choices": [{"index": 0, "message": delta, "finish_reason": finish}],
                            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}).encode())
                except (BrokenPipeError, ConnectionResetError):
                    pass
        self.http = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.http.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        for gate in self.gates.values():
            gate.set()
        self.http.shutdown()
        self.http.server_close()
        self.thread.join(timeout=3)


class Probe:
    def __init__(self, binary, root, sockets):
        self.binary, self.root = binary, root
        self.children, self.host, self.master, self.server = [], None, None, None
        self.events = root / "tui-events.ndjson"
        self.commands = root / "commands"
        self.commands.mkdir(mode=0o700)
        self.fixture = Provider()
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(("ZELLIJ", "VERIJ_ZELLIJ", "OPENCODE"))}
        for kind in ("CONFIG", "CACHE", "DATA", "STATE"):
            self.env[f"XDG_{kind}_HOME"] = str(root / kind.lower())
        self.env.update(HOME=str(root / "home"), OPENCODE_TEST_HOME=str(root / "home"),
                        OPENCODE_CONFIG_DIR=str(root / "config" / "opencode"),
                        OPENCODE_DISABLE_PROJECT_CONFIG="1", OPENCODE_DISABLE_AUTOUPDATE="1",
                        OPENCODE_DISABLE_DEFAULT_PLUGINS="1", OPENCODE_DISABLE_EXTERNAL_SKILLS="1",
                        OPENCODE_DISABLE_CLAUDE_CODE_SKILLS="1", VJ_RUNTIME_EVENTS=str(self.events),
                        VJ_RUNTIME_COMMANDS=str(self.commands), TERM="xterm-256color", ZELLIJ_SOCKET_DIR=sockets)
        (root / "home").mkdir(mode=0o700)
        (root / "tmp").mkdir(mode=0o700)
        self.env["TMPDIR"] = str(root / "tmp")
        config = {"$schema": "https://opencode.ai/config.json", "model": "fixture/success", "small_model": "fixture/title",
                  "enabled_providers": ["fixture"], "autoupdate": False, "share": "disabled", "snapshot": False,
                  "lsp": False, "formatter": False, "permission": {"*": "deny", "task": "allow"},
                  "agent": {"title": {"disable": True}, "summary": {"disable": True}, "general": {"model": "fixture/child"}},
                  "provider": {"fixture": {"npm": "@ai-sdk/openai-compatible", "name": "Controlled local fixture", "options": {
                      "apiKey": "probe-not-a-secret", "baseURL": f"http://127.0.0.1:{self.fixture.http.server_port}/v1", "timeout": 70000},
                      "models": {name: {"name": name, "limit": {"context": 32000, "output": 2000}, "tool_call": True}
                                 for name in self.fixture.entered}}}}
        config_path = root / "opencode.json"
        config_path.write_text(json.dumps(config))
        tui = root / "tui.json"
        tui.write_text(json.dumps({"plugin": [str(Path(__file__).with_name("runtime-tui.mjs").resolve())]}))
        self.env.update(OPENCODE_CONFIG=str(config_path), OPENCODE_TUI_CONFIG=str(tui))
        self.zconfig = root / "zellij.kdl"
        self.zconfig.write_text('default_shell "/bin/sh"\nshow_startup_tips false\nshow_release_notes false\nmirror_session false\nsession_serialization false\n')
        self.layout = root / "layout.kdl"
        self.layout.write_text("layout {\n pane\n}\n")
        self.report = {"artifacts_dir": str(root), "provider": "controlled local HTTP fixture (synthetic model output)",
                       "opencode_version": subprocess.check_output(["opencode", "--version"], text=True).strip(), "checks": [], "limitations": []}
        self.name = "runtime-probe"

    def stage(self, name):
        self.report["stage"] = name

    def drain(self):
        if self.master is not None:
            while select.select([self.master], [], [], 0)[0]:
                try:
                    data = os.read(self.master, 65536)
                    with (self.root / "host.terminal").open("ab") as file:
                        file.write(data)
                except OSError:
                    break

    def wait(self, predicate, timeout=45):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.drain()
            try:
                value = predicate()
                if value:
                    return value
            except (OSError, ValueError, urllib.error.URLError):
                pass
            time.sleep(.1)
        raise TimeoutError(f"deadline at {self.report.get('stage')}")

    def observations(self):
        return [json.loads(line) for line in self.events.read_text().splitlines() if line.strip()] if self.events.exists() else []

    def http(self, path, data=None, method=None) -> Any:
        request = urllib.request.Request(self.url + path, data=json.dumps(data).encode() if data is not None else None,
                                         headers={"Content-Type": "application/json"}, method=method or ("POST" if data is not None else "GET"))
        with urllib.request.urlopen(request, timeout=5) as response:
            content = response.read()
            return json.loads(content) if content else None

    def action(self, *parts):
        result = subprocess.run([str(self.binary), "--config", str(self.zconfig), "-s", self.name, "action", *parts],
                                env=self.env, cwd=self.root, capture_output=True, text=True, timeout=8)
        if result.returncode:
            raise RuntimeError((parts, result.stderr[:200]))
        return result.stdout.strip()

    def command(self, pid, sessions, route=None, selected=None):
        correlation = uuid.uuid4().hex
        data = {"id": correlation, "sessions": sessions}
        if route:
            data.update(action="navigate", route=route, params={"sessionID": selected} if selected else {})
        target = self.commands / f"{pid}.json"
        temporary = target.with_suffix(".tmp")
        temporary.write_text(json.dumps(data))
        os.replace(temporary, target)
        return self.wait(lambda: next((e for e in self.observations() if e.get("correlation") == correlation), None), timeout=10)

    def ready(self, pane, excluding=None):
        return self.wait(lambda: next((e for e in self.observations() if e["kind"] == "ready" and
                                     e["pane"] == str(pane) and e["pid"] != excluding), None))

    def prompt(self, session, model):
        self.http(f"/session/{session}/prompt_async", {"model": {"providerID": "fixture", "modelID": model},
                                                    "parts": [{"type": "text", "text": "Controlled runtime probe"}]})

    def messages(self, session):
        return self.http(f"/session/{session}/message")

    def settled(self, session, error=False):
        messages = self.messages(session)
        assistants = [message["info"] for message in messages if message["info"]["role"] == "assistant"]
        if not assistants:
            return None
        last = assistants[-1]
        if self.http("/session/status").get(session, {}).get("type", "idle") != "idle":
            return None
        if error:
            return last if last.get("error") else None
        return last if last.get("finish") in ("stop", "end_turn") and last.get("time", {}).get("completed") and not last.get("error") else None

    def start(self):
        self.stage("isolated OpenCode listener and config")
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        self.url = f"http://127.0.0.1:{port}"
        self.log = (self.root / "server.log").open("wb")
        self.server = subprocess.Popen(["opencode", "serve", "--port", str(port), "--hostname", "127.0.0.1"],
                                       cwd=self.root, env=self.env, stdout=self.log, stderr=self.log, start_new_session=True)
        self.wait(lambda: self.http("/global/health"))
        assert self.server.poll() is None
        paths = self.http("/path")
        assert all(str(self.root) in paths.get(name, "") for name in ("home", "config", "state", "directory")), paths
        # Verify the selected LISTEN socket is actually held by this server PID.
        listeners = {line.split()[9] for line in Path(f"/proc/{self.server.pid}/net/tcp").read_text().splitlines()[1:]
                     if int(line.split()[1].split(":")[1], 16) == port and line.split()[3] == "0A"}
        targets = set()
        for fd in Path(f"/proc/{self.server.pid}/fd").iterdir():
            try:
                targets.add(os.readlink(fd))
            except FileNotFoundError:
                pass
        assert targets & {f"socket:[{inode}]" for inode in listeners}, "listener not held by server PID"
        self.server_birth = process_identity(self.server.pid)
        self.report["checks"].append({"private_home_xdg_database_and_verified_listener": True,
                                      "port": port, "server_birth": self.server_birth, "paths": paths})
        a = self.http("/session", {})["id"]
        b = self.http("/session", {})["id"]
        database = self.root / "data" / "opencode" / "opencode.db"
        assert database.is_file(), "runtime database not in private XDG storage"
        self.report["checks"][0]["database_path"] = str(database)
        self.stage("two real pane-local TUIs")
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 160, 0, 0))
        self.host = subprocess.Popen([str(self.binary), "--config", str(self.zconfig), "--layout", str(self.layout),
                                      "attach", "-c", self.name, "--", "opencode", "attach", self.url, "--session", a],
                                     env=self.env, cwd=self.root, stdin=slave, stdout=slave, stderr=slave,
                                     start_new_session=True, preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0))
        os.close(slave)
        aa = self.ready(0)
        pane = self.action("new-pane", "--", "opencode", "attach", self.url, "--session", b)
        assert pane == "terminal_1", pane
        bb = self.ready(1)
        for tui in (aa, bb):
            birth = tui["birth"]
            assert birth["pgrp"] == birth["tpgid"] == tui["pid"] and birth["tty_nr"] != 0, birth
            assert process_identity(tui["pid"]) == {k: birth[k] for k in ("pid", "boot_id", "start_jiffies")}
            self.children.append(tui["pid"])
        assert aa["birth"]["tty_nr"] != bb["birth"]["tty_nr"]
        self.command(aa["pid"], [a], "session", a)
        self.command(bb["pid"], [b], "session", b)
        inventory = json.loads(self.action("list-panes", "--all", "--json"))
        (self.root / "native-inventory.json").write_text(json.dumps(inventory))
        self.report["checks"].append({"two_native_panes_foreground_tpgid_and_birth_verified": True,
                                      "tuis": [aa, bb], "inventory": [
                                          {"pane_id": p["id"], "command_is_opencode_attach": p.get("pane_command", "").startswith("opencode attach "),
                                           "cwd_is_private": p.get("pane_cwd") == str(self.root)} for p in inventory if not p["is_plugin"]]})
        return a, b, aa, bb

    def run(self):
        a, b, aa, bb = self.start()
        assert self.server is not None
        self.stage("terminal finish metadata")
        self.prompt(a, "success")
        completed = self.wait(lambda: self.settled(a))
        snapshot = self.command(aa["pid"], [a])
        assert any(m["finish"] == "stop" and m["completed"] and not m["error_name"] for m in snapshot["sessions"][0]["messages"]), snapshot
        self.report["checks"].append({"terminal_success_metadata_not_idle_alone": True, "snapshot": snapshot,
                                      "api_finish": completed["finish"], "api_completed": True})

        self.stage("nonretryable terminal error")
        self.prompt(b, "error")
        failure = self.wait(lambda: self.settled(b, error=True))
        self.report["checks"].append({"terminal_error_not_done": True, "error_name": failure["error"]["name"],
                                      "finish": failure.get("finish"), "snapshot": self.command(bb["pid"], [b])})

        self.stage("retry remains nonterminal")
        retry_session = self.http("/session", {})["id"]
        self.command(aa["pid"], [retry_session], "session", retry_session)
        self.prompt(retry_session, "retry")
        retry = self.wait(lambda: next((e for e in self.observations() if e.get("session") == key(retry_session) and e.get("status") == "retry"), None), timeout=60)
        assert retry["retry_attempt"] > 0
        retry_snapshot = self.command(aa["pid"], [retry_session])
        assert not any(m["finish"] == "stop" and m["completed"] for m in retry_snapshot["sessions"][0]["messages"])
        self.fixture.gates["retry"].set()
        final = self.wait(lambda: self.settled(retry_session), timeout=60)
        self.report["checks"].append({"retry_callback_before_terminal_finish": True, "retry": retry,
                                      "retry_snapshot": retry_snapshot, "final_finish": final["finish"]})

        self.stage("cancellation is not terminal success")
        cancelled = self.http("/session", {})["id"]
        self.command(aa["pid"], [cancelled], "session", cancelled)
        self.prompt(cancelled, "cancel")
        self.wait(self.fixture.entered["cancel"].is_set)
        self.http(f"/session/{cancelled}/abort", {})
        aborted = self.wait(lambda: self.settled(cancelled, error=True))
        assert aborted["error"]["name"] == "MessageAbortedError", aborted
        self.fixture.gates["cancel"].set()
        self.report["checks"].append({"abort_has_explicit_message_error": True, "error_name": aborted["error"]["name"],
                                      "snapshot": self.command(aa["pid"], [cancelled])})

        self.stage("TUI replacement recovers in-flight snapshot without replay")
        recovering = self.http("/session", {})["id"]
        self.command(aa["pid"], [recovering], "session", recovering)
        self.prompt(recovering, "recovery")
        self.wait(self.fixture.entered["recovery"].is_set)
        before = self.command(aa["pid"], [recovering])
        assert before["sessions"][0]["status"] == "busy", before
        count = self.fixture.counts["recovery"]
        assert process_identity(aa["pid"]) == {k: aa["birth"][k] for k in ("pid", "boot_id", "start_jiffies")}
        os.kill(aa["pid"], signal.SIGKILL)
        self.wait(lambda: process_identity(aa["pid"]) is None)
        pane = self.action("new-pane", "--in-place", "--pane-id", "terminal_0", "--close-replaced-pane", "--",
                           "opencode", "attach", self.url, "--session", recovering)
        replacement_pane = int(pane.split("_")[-1]) if pane else 2
        anew = self.ready(replacement_pane, aa["pid"])
        self.children.append(anew["pid"])
        birth = anew["birth"]
        assert birth["pgrp"] == birth["tpgid"] == anew["pid"]
        assert process_identity(anew["pid"]) == {k: birth[k] for k in ("pid", "boot_id", "start_jiffies")}
        recovered = self.command(anew["pid"], [recovering], "session", recovering)
        first_snapshot = recovered
        def hydrated():
            snapshot = self.command(anew["pid"], [recovering])
            value = snapshot["sessions"][0]
            return snapshot if value["status"] == "busy" and value["messages"] else None
        recovered = self.wait(hydrated, timeout=25)
        assert self.fixture.counts["recovery"] == count and process_identity(self.server.pid) == self.server_birth
        self.fixture.gates["recovery"].set()
        self.wait(lambda: self.settled(recovering))
        terminal = self.command(anew["pid"], [recovering])
        assert any(m["finish"] == "stop" and m["completed"] for m in terminal["sessions"][0]["messages"])
        self.report["checks"].append({"replacement_tui_midturn_snapshot_without_provider_replay": True,
                                      "before": before, "first_snapshot": first_snapshot, "recovered": recovered,
                                      "terminal": terminal, "provider_requests": count,
                                      "ready_flag_can_precede_snapshot_hydration": first_snapshot["sessions"][0]["status"] is None})
        aa = anew

        self.stage("native child family and parent completion")
        family = self.http("/session", {})["id"]
        self.command(aa["pid"], [family], "session", family)
        self.prompt(family, "family")
        self.wait(self.fixture.entered["child"].is_set)
        child = self.wait(lambda: next((s for s in self.http(f"/session/{family}/children") if s.get("parentID") == family), None))
        owned = self.command(aa["pid"], [family, child["id"]])
        foreign = self.command(bb["pid"], [family, child["id"]])
        assert all(s["in_family"] for s in owned["sessions"]), owned
        assert all(not s["in_family"] for s in foreign["sessions"]), foreign
        assert self.http("/session/status")[child["id"]]["type"] == "busy"
        assert not self.settled(family)
        self.fixture.gates["child"].set()
        self.wait(lambda: self.settled(child["id"]))
        self.wait(lambda: self.settled(family))
        final = self.command(aa["pid"], [family, child["id"]])
        assert all(any(m["finish"] == "stop" and m["completed"] for m in s["messages"]) for s in final["sessions"]), final
        self.report["checks"].append({"native_task_child_family_owner_and_foreign": True, "owned": owned, "foreign": foreign,
                                      "busy_child_prevented_terminal_parent": True, "final": final})
        self.stage("home route suppresses later semantic ownership")
        self.command(aa["pid"], [a], "home")
        since = len(self.observations())
        prior_message = self.messages(a)[-1]["info"]["id"]
        self.prompt(a, "success")
        self.wait(lambda: (item := self.settled(a)) and item["id"] != prior_message)
        events = [e for e in self.observations()[since:] if e.get("session") == key(a) and e["pid"] == aa["pid"]]
        assert events and all(not e["in_family"] for e in events), events
        self.report["checks"].append({"post_home_semantic_events_have_no_local_owner": True})
        self.report["provider_requests"] = dict(self.fixture.counts)
        self.report["limitations"].append("Controlled provider responses are synthetic; this verifies installed runtime handling, not remote-model/provider coverage.")
        self.report["limitations"].append("TUI recovery uses a new native pane/process attached to the same live server; server crash recovery and production reducer are not implemented.")

    def close(self):
        self.fixture.close()
        if self.host:
            subprocess.run([str(self.binary), "kill-session", self.name], cwd=self.root, env=self.env, capture_output=True, timeout=8)
            try:
                self.host.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.host.kill()
                self.host.wait(timeout=5)
            assert self.master is not None
            os.close(self.master)
            self.master = None
        if self.server:
            self.server.terminate()
            try:
                self.server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.server.kill()
                self.server.wait(timeout=5)
        if hasattr(self, "log"):
            self.log.close()
        self.report["probe_processes_exited"] = all(process_identity(pid) is None for pid in self.children)
        self.report["server_and_host_exited"] = (self.server is None or self.server.poll() is not None) and (self.host is None or self.host.poll() is not None)
        if not self.report["probe_processes_exited"] or not self.report["server_and_host_exited"]:
            self.report.update(status="FAIL", cleanup_incomplete=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--scratch-dir", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    args.scratch_dir.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="vj-runtime-", dir=args.scratch_dir)).resolve()
    with tempfile.TemporaryDirectory(prefix="vj-runtime-sock-") as sockets:
        probe = Probe(args.binary.resolve(strict=True), root, sockets)
        try:
            probe.run()
            probe.report["status"] = "PASS"
            probe.report.pop("stage", None)
        except Exception as error:
            probe.report.update(status="FAIL", error=str(error))
        finally:
            probe.close()
    text = json.dumps(probe.report, indent=2) + "\n"
    (root / "results.json").write_text(text)
    if args.output:
        args.output.write_text(text)
    print(text)
    return 0 if probe.report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
