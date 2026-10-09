#!/usr/bin/env python3
"""Production H3 adapter in real stock Zellij panes; controlled model output."""
import argparse
from collections import Counter
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import threading
import time
import uuid
from typing import Any

SOURCE = Path(__file__).resolve().parents[1] / "opencode-runtime/verify_runtime.py"
spec = importlib.util.spec_from_file_location("h0_runtime", SOURCE)
assert spec and spec.loader
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


class Provider:
    def __init__(self):
        self.counts = Counter()
        names = ("success", "error", "retry", "cancel", "recovery", "family", "child", "permission", "question", "title", "parent")
        self.entered = {name: threading.Event() for name in names}
        self.gates = {name: threading.Event() for name in names}
        for gate in self.gates.values():
            gate.set()
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, format, *args):
                pass

            def handle(self):
                try:
                    super().handle()
                except (BrokenPipeError, ConnectionResetError):
                    pass  # Expected when abort closes a controlled request.

            def do_POST(self):
                request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                model = request["model"]
                owner.counts[model] += 1
                owner.entered[model].set()
                if model == "error" or (model == "retry" and owner.counts[model] <= 2):
                    self.send_response(400 if model == "error" else 429)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Retry-After", "1")
                    self.end_headers()
                    self.wfile.write(b'{"error":{"message":"controlled fixture","type":"invalid_request"}}')
                    return
                has_tool = any(m["role"] == "tool" for m in request["messages"])
                if model == "family" and has_tool:
                    owner.entered["parent"].set()
                    if not owner.gates["parent"].wait(60):
                        return
                if not owner.gates[model].wait(60):
                    return
                delta = {"role": "assistant", "content": "OK"}
                finish = "stop"
                if not has_tool and model in ("family", "permission", "question"):
                    tool, arguments = {
                        "family": ("task", {"description": "Fixture child", "prompt": "Return OK", "subagent_type": "general"}),
                        "permission": ("bash", {"command": "printf VJ_H3_PERMISSION", "description": "Harmless fixture marker"}),
                        "question": ("question", {"questions": [{"header": "Fixture", "question": "Choose fixture", "options": [
                            {"label": "Alpha", "description": "Fixture alpha"}, {"label": "Beta", "description": "Fixture beta"}]}]}),
                    }[model]
                    delta = {"role": "assistant", "tool_calls": [{"index": 0, "id": "call_fixture_" + model,
                        "type": "function", "function": {"name": tool, "arguments": json.dumps(arguments)}}]}
                    finish = "tool_calls"
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                try:
                    for change, reason in ((delta, None), ({}, finish)):
                        payload = {"id": "chatcmpl_fixture", "object": "chat.completion.chunk", "created": int(time.time()),
                            "model": model, "choices": [{"index": 0, "delta": change, "finish_reason": reason}]}
                        self.wfile.write(b"data: " + json.dumps(payload).encode() + b"\n\n")
                        self.wfile.flush()
                    self.wfile.write(b"data: [DONE]\n\n")
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


class Probe(runtime.Probe):
    def __init__(self, zellij, verij, plugin, root, sockets):
        super().__init__(zellij, root, sockets)
        self.verij = verij
        self.fixture.close()
        self.fixture = Provider()
        self.env.update(VERIJ_STATES_DIR=str(root / "states"), VERIJ_AGENT_STATE_DIR=str(root / "agents"),
                        VJ_H3_COMMANDS=str(root / "h3-commands"))
        for path in (root / "states", root / "agents", root / "control", root / "h3-commands", root / "cache/zellij"):
            path.mkdir(parents=True, exist_ok=True)
        config = json.loads((root / "opencode.json").read_text())
        config["provider"]["fixture"]["options"]["baseURL"] = f"http://127.0.0.1:{self.fixture.http.server_port}/v1"
        config["provider"]["fixture"]["models"] = {name: {"name":name,"limit":{"context":32000,"output":2000},"tool_call":True}
                                                     for name in self.fixture.entered}
        config["permission"] = {"*":"deny","task":"allow","bash":"ask","question":"allow"}
        (root / "opencode.json").write_text(json.dumps(config))
        tui = root / "config/opencode/tui.json"
        tui.parent.mkdir(parents=True, exist_ok=True)
        driver = str(Path(__file__).with_name("driver.mjs"))
        tui.write_text(json.dumps({"$schema":"https://opencode.ai/tui.json", "theme":"opencode", "plugin":[driver]}))
        self.env["OPENCODE_TUI_CONFIG"] = str(tui)
        setup = self.cli("agent", "setup", "opencode", "--config-dir", str(tui.parent))
        first = tui.read_text()
        self.cli("agent", "setup", "opencode", "--config-dir", str(tui.parent))
        assert first == tui.read_text(), "setup not idempotent"
        assert json.loads(first)["plugin"][0] == driver
        doctor = self.cli("agent", "doctor", "--config-dir", str(tui.parent))
        assert doctor["assets_match"] and doctor["reporter_available"] and doctor["version_supported"]
        # Explicitly synthetic legacy wire-format fixture, not a semantic
        # agent record or native binding. Doctor must flag the missing exporter
        # prerequisite instead of treating adapter installation as readiness.
        legacy_file = root / "states/legacy-exporter-fixture.json"
        legacy_file.write_text(json.dumps({"name":"legacy-exporter-fixture","is_current":True,"tabs":[],"active_pane":None}))
        try:
            legacy = self.cli("agent", "doctor", "--config-dir", str(tui.parent), "--session", "legacy-exporter-fixture")
            assert legacy["installation_ready"] and not legacy["monitoring_prerequisites_ready"]
            assert legacy["inventory_sessions"][0]["status"] == "legacy_snapshot"
        finally:
            legacy_file.unlink()
        url = "file:" + str(plugin)
        (root / "cache/zellij/permissions.kdl").write_text("\n".join(json.dumps(name) + " {\n ReadApplicationState\n ChangeApplicationState\n ReadCliPipes\n}\n"
                                                                          for name in (url, str(plugin))))
        self.zconfig.write_text(self.zconfig.read_text() + f'load_plugins {{\n {json.dumps(url)} {{\n state_dir "/host/states"\n control_dir "/host/control"\n }}\n}}\n')
        self.env["ZELLIJ_CONFIG_FILE"] = str(self.zconfig)
        self.report.update(verij=str(verij), zellij_version=subprocess.check_output([zellij, "--version"], text=True).strip(),
                           checks=[{"setup_idempotence_and_diagnostics":True,"setup":setup,"doctor":doctor},
                                   {"legacy_exporter_prerequisite_diagnosed":True,"snapshot_fixture_synthetic":True,"doctor":legacy}])

    def cli(self, *args):
        result = subprocess.run([str(self.verij), *args], env=self.env, cwd=self.root, capture_output=True, text=True, timeout=10)
        if result.returncode:
            raise RuntimeError(result.stderr)
        return json.loads(result.stdout)

    def records(self):
        records = []
        for file in (self.root / "agents/agents/v1").glob("*/identity.json"):
            identity = json.loads(file.read_text())
            state = json.loads(file.with_name("state.json").read_text())
            if runtime.process_identity(identity["process"]["pid"]):
                records.append({"identity":identity,"state":state})
        return records

    def record(self, pid, status=None) -> Any:
        return next((r for r in self.records() if r["identity"]["process"]["pid"] == pid and
                     (status is None or r["state"]["reduced"]["status"] == status)), None)

    def capture(self, name, *pids):
        records = [self.record(pid) for pid in pids]
        assert all(records)
        target = self.root / f"evidence-{len(self.report['checks']):02}.json"
        target.write_text(json.dumps(records, indent=2))
        self.report["checks"].append({name:True,"records":records,"evidence_file":str(target)})

    def control(self, pid, action):
        correlation = uuid.uuid4().hex
        file = self.root / f"h3-commands/{pid}.json"
        file.write_text(json.dumps({"id":correlation,"action":action}))
        return self.wait(lambda: next((e for e in self.observations() if e.get("correlation") == correlation), None))

    def select(self, pid, session):
        self.command(pid, [session], "session", session)
        return self.wait(lambda: (r := self.record(pid)) and r["state"]["reduced"].get("conversation_id") == session and r)

    def run(self):
        a, b, aa, bb = self.start()
        apid, bpid = aa["pid"], bb["pid"]
        self.stage("idle production rows")
        self.wait(lambda: len(self.records()) == 2 and self.record(apid, "idle") and self.record(bpid, "idle"))
        assert self.cli("agent", "doctor", "--session", self.name)["monitoring_prerequisites_ready"]
        assert all(not r["identity"]["is_synthetic"] and r["state"]["completion_revision"] == 0 for r in self.records())
        self.capture("two_live_idle_production_rows_before_prompt", apid, bpid)

        self.stage("one owned working turn and aggregate completion")
        self.fixture.gates["success"].clear()
        self.prompt(a, "success")
        self.wait(lambda: self.record(apid, "working"))
        assert self.record(bpid, "idle")
        self.capture("shared_server_working_isolation", apid, bpid)
        self.fixture.gates["success"].set()
        self.wait(lambda: self.record(apid, "done"))
        revision = self.record(apid)["state"]["completion_revision"]
        self.wait(lambda: self.record(apid)["state"]["sources"]["opencode"]["source_revision"] > 4)
        time.sleep(2)
        assert self.record(apid)["state"]["completion_revision"] == revision == 1
        self.capture("terminal_success_is_idempotent", apid, bpid)

        self.stage("terminal error")
        self.prompt(b, "error")
        self.wait(lambda: self.record(bpid, "error"))
        assert self.record(bpid)["state"]["completion_revision"] == 0
        self.capture("terminal_error_without_done", apid, bpid)

        self.stage("permission and reply")
        permission_session = self.http("/session", {})["id"]
        self.select(apid, permission_session)
        self.prompt(permission_session, "permission")
        permission = self.wait(lambda: next((p for p in self.http("/permission") if p["sessionID"] == permission_session), None))
        self.wait(lambda: self.record(apid, "needs_input"))
        assert self.record(bpid, "error")
        self.action("focus-pane-id", "terminal_0")
        self.capture("native_permission_needs_input_and_foreign_isolation", apid, bpid)
        self.http(f"/permission/{permission['id']}/reply", {"reply":"once"})
        self.wait(lambda: self.record(apid, "done"))
        self.capture("permission_resolution_continues_then_completes", apid)

        self.stage("question and rejection")
        question_session = self.http("/session", {})["id"]
        self.select(bpid, question_session)
        self.prompt(question_session, "question")
        question = self.wait(lambda: next((q for q in self.http("/question") if q["sessionID"] == question_session), None))
        self.wait(lambda: self.record(bpid, "needs_input"))
        self.capture("native_question_needs_input", apid, bpid)
        self.http(f"/question/{question['id']}/reject", {})
        self.wait(lambda: self.record(bpid, "idle"))
        assert self.record(bpid)["state"]["completion_revision"] == 0
        self.capture("question_rejection_idle_without_terminal_success", bpid)
        question_reply = self.http("/session", {})["id"]
        self.select(bpid, question_reply)
        self.prompt(question_reply, "question")
        question = self.wait(lambda: next((q for q in self.http("/question") if q["sessionID"] == question_reply), None))
        self.wait(lambda: self.record(bpid, "needs_input"))
        self.http(f"/question/{question['id']}/reply", {"answers":[["Alpha"]]})
        self.wait(lambda: self.record(bpid, "done"))
        self.capture("question_reply_resumes_to_terminal_success", bpid)

        self.stage("retry")
        retry = self.http("/session", {})["id"]
        self.select(apid, retry)
        self.fixture.gates["retry"].clear()
        self.prompt(retry, "retry")
        self.wait(lambda: (r := self.record(apid, "working")) and r["state"]["sources"]["opencode"]["activity"] == "retrying")
        self.capture("retry_is_working_without_completion", apid)
        self.fixture.gates["retry"].set()
        self.wait(lambda: self.record(apid, "done"))

        self.stage("abort")
        cancelled = self.http("/session", {})["id"]
        self.select(apid, cancelled)
        self.fixture.gates["cancel"].clear()
        previous = self.record(apid)["state"]["completion_revision"]
        self.prompt(cancelled, "cancel")
        self.wait(lambda: self.record(apid, "working"))
        self.http(f"/session/{cancelled}/abort", {})
        self.wait(lambda: self.record(apid, "error"))
        assert self.record(apid)["state"]["completion_revision"] == previous
        self.capture("abort_error_never_done", apid)
        self.fixture.gates["cancel"].set()

        self.stage("native task child")
        family = self.http("/session", {})["id"]
        self.select(apid, family)
        self.fixture.gates["child"].clear()
        self.fixture.gates["parent"].clear()
        previous = self.record(apid)["state"]["completion_revision"]
        self.prompt(family, "family")
        self.wait(self.fixture.entered["child"].is_set)
        self.wait(lambda: (r := self.record(apid, "working")) and r["state"]["reduced"]["background_task_count"] == 1)
        self.capture("child_work_is_aggregated_without_extra_pane_row", apid, bpid)
        self.fixture.gates["child"].set()
        self.wait(self.fixture.entered["parent"].is_set)
        assert self.record(apid, "working") and self.record(apid)["state"]["completion_revision"] == previous
        self.capture("child_completion_keeps_parent_working", apid)
        self.fixture.gates["parent"].set()
        self.wait(lambda: self.record(apid, "done"))

        self.stage("plugin reload and conversation reselection")
        identity = self.record(apid)["identity"]
        previous = self.record(apid)["state"]["completion_revision"]
        self.control(apid, "disable")
        self.wait(lambda: self.record(apid, "unknown"))
        self.control(apid, "enable")
        self.wait(lambda: self.record(apid, "done"))
        assert self.record(apid)["identity"] == identity
        assert self.record(apid)["state"]["completion_revision"] == previous
        self.capture("plugin_reload_preserves_instance_and_completion_revision", apid)
        self.stage("source health while TUI is alive but callbacks stop")
        os.kill(apid, signal.SIGSTOP)
        try:
            doctor = self.wait(lambda: (d := self.cli("agent", "doctor", "--config-dir", str(self.root / "config/opencode")))
                and any(r["pid"] == apid and r["status"] == "unknown" and not r["source_healthy"] for r in d["records"]) and d,
                timeout=25)
            assert runtime.process_identity(apid) is not None
            self.report["checks"].append({"source_silence_is_unknown_not_exit_or_done":True,"doctor":doctor})
        finally:
            os.kill(apid, signal.SIGCONT)
        self.wait(lambda: self.record(apid, "done") and self.record(apid)["state"]["updated_at_ms"] > int(time.time() * 1000) - 2000)
        assert self.record(apid)["state"]["completion_revision"] == previous
        self.capture("lease_recovery_does_not_replay_completion", apid)
        self.command(apid, [family], "home")
        self.wait(lambda: self.record(apid, "idle") and not self.record(apid)["state"]["reduced"].get("conversation_id"))
        self.select(apid, a)
        self.wait(lambda: self.record(apid, "done"))
        assert self.record(apid)["state"]["completion_revision"] == previous
        assert self.record(apid)["state"]["reduced"]["completion_revision"] == 1
        self.capture("conversation_reselection_restores_original_revision", apid)

        self.stage("session rename and reporter rebind")
        self.action("rename-session", "h3-renamed")
        self.name = "h3-renamed"
        self.wait(lambda: any(s["name"] == self.name for s in self.cli("inventory")))
        self.control(apid, "disable")
        self.control(apid, "enable")
        self.wait(lambda: self.record(apid, "done"))
        assert self.record(apid)["identity"] == identity
        self.capture("session_rename_reload_rebind_ignores_stale_environment", apid)

        self.stage("midturn TUI replacement")
        recovering = self.http("/session", {})["id"]
        self.select(apid, recovering)
        self.fixture.gates["recovery"].clear()
        self.prompt(recovering, "recovery")
        self.wait(lambda: self.record(apid, "working"))
        requests = self.fixture.counts["recovery"]
        os.kill(apid, signal.SIGKILL)
        self.wait(lambda: runtime.process_identity(apid) is None)
        pane = self.action("new-pane", "--in-place", "--pane-id", "terminal_0", "--close-replaced-pane", "--",
                           "opencode", "attach", self.url, "--session", recovering)
        if pane:
            replacement_pane = int(pane.split("_")[-1])
        else:
            init = self.wait(lambda: next((e for e in self.observations() if e["kind"] == "init"
                and e["pid"] not in (apid, bpid) and runtime.process_identity(e["pid"])), None))
            replacement_pane = int(init["pane"])
        replacement = self.ready(replacement_pane, apid)
        apid = replacement["pid"]
        self.children.append(apid)
        self.wait(lambda: self.record(apid, "working"))
        assert self.record(apid)["identity"]["agent_instance_id"] != identity["agent_instance_id"]
        assert self.fixture.counts["recovery"] == requests
        self.capture("midturn_recovery_new_process_no_provider_replay", apid, bpid)
        self.fixture.gates["recovery"].set()
        self.wait(lambda: self.record(apid, "done"))

        self.stage("agent exits while shell pane survives")
        shell_pane = self.action("new-pane", "--", "/bin/sh", "-c", 'opencode; exec /bin/sh')
        shell_tui = self.ready(int(shell_pane.split("_")[-1]))
        shell_pid = shell_tui["pid"]
        self.children.append(shell_pid)
        self.wait(lambda: self.record(shell_pid))
        self.wait(lambda: self.record(shell_pid, "idle"))
        assert self.record(shell_pid)["state"]["completion_revision"] == 0
        self.capture("normal_opencode_home_presence_before_prompt", shell_pid)
        os.kill(shell_pid, signal.SIGTERM)
        self.wait(lambda: runtime.process_identity(shell_pid) is None)
        self.wait(lambda: all(r["identity"]["process"]["pid"] != shell_pid for r in self.records()))
        panes = json.loads(self.action("list-panes", "--all", "--json"))
        assert any(p["id"] == int(shell_pane.split("_")[-1]) and not p["exited"] for p in panes)
        pruned = self.cli("agent", "prune")
        assert identity["agent_instance_id"] in pruned["pruned"]
        self.report["checks"].append({"agent_exit_removes_record_while_shell_survives":True,"pruned":pruned})

        self.stage("uninstall preservation")
        self.cli("agent", "uninstall", "opencode", "--config-dir", str(self.root / "config/opencode"))
        config = json.loads((self.root / "config/opencode/tui.json").read_text())
        assert config["theme"] == "opencode" and config["plugin"] == [str(Path(__file__).with_name("driver.mjs"))]
        self.report["checks"].append({"uninstall_preserves_unrelated_plugin_and_theme":True})
        self.report["provider_requests"] = dict(self.fixture.counts)
        self.report["limitations"] = ["Model output is controlled synthetic HTTP input; all TUI events, tool permission/question, child execution, store and bindings are real installed runtime.",
            "This harness verifies production records and native panes. Whole-host input/visit UI remains a separate human H3 review."]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--zellij", required=True, type=Path)
    parser.add_argument("--verij", required=True, type=Path)
    parser.add_argument("--plugin", required=True, type=Path)
    parser.add_argument("--scratch-dir", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = Path(tempfile.mkdtemp(prefix="vj-h3-", dir=args.scratch_dir)).resolve()
    with tempfile.TemporaryDirectory(prefix="vj-h3-sock-") as sockets:
        probe = Probe(args.zellij.resolve(), args.verij.resolve(), args.plugin.resolve(), root, sockets)
        try:
            probe.run()
            probe.report.update(status="PASS")
            probe.report.pop("stage", None)
        except Exception as error:
            probe.report.update(status="FAIL", error=str(error))
            probe.report["failure_records"] = probe.records()
        finally:
            probe.close()
    text = json.dumps(probe.report, indent=2) + "\n"
    (root / "results.json").write_text(text)
    if args.output:
        args.output.write_text(text)
    print(json.dumps({"status":probe.report["status"], "stage":probe.report.get("stage"),
                      "error":probe.report.get("error"), "checks":len(probe.report["checks"]),
                      "results":str(root / "results.json")}, indent=2))
    return 0 if probe.report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
