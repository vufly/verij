#!/usr/bin/env python3
"""P4: actual Pi, stock Zellij exporter inventory, two sidebars, private provider.

Run against explicitly built assets. Never modifies global Pi/Verij installation.
Retains only bounded state/trace summaries and hashes, not terminal/provider content.
"""
import argparse
from collections import Counter
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time
import traceback

from runtime import HERE, REPO, Runtime, birth, opaque, sha

CASES = ("success", "parallel", "nested", "tool_error", "provider_error", "retry_success", "retry_exhausted",
         "cancel", "queued", "late_queued", "continuation", "auto_compact", "ui", "ui_nested", "postrun_error", "continuation_error")
EXPECTED = {"provider_error": "error", "retry_exhausted": "error", "cancel": "idle",
            "postrun_error": "unknown", "continuation_error": "unknown"}


class Acceptance(Runtime):
    def record(self, name, facts, samples=None):
        self.report["cases"].append({"case": name, "facts": facts, "samples": samples or []})
        print(name + ": PASS", flush=True)

    def run_case(self, name, pane=0):
        print("running " + name, flush=True)
        start = len(self.events(pane))
        settled_before = sum(e["event"] == "agent_settled" and e["source"] == "observer" for e in self.events(pane))
        initial = self.state(pane)
        before = initial["completion_revision"]
        offset = len(self.samples)
        providers = len(self.provider.observations())
        # Every model case starts fresh, independent of earlier fixture requests.
        with self.provider.lock:
            self.provider.counts[name] = 0
        self.send("/probe run " + name, pane)
        dismissed, request_ids, cancelled, visible = set(), set(), False, 0
        def settled():
            nonlocal cancelled, visible
            events = self.events(pane)[start:]
            if name == "cancel" and not cancelled and any(r["model"] == name for r in self.provider.observations()[providers:]):
                self.escape(pane)
                cancelled = True
            prompts = [e for e in events if e["event"] == "ui_prompt_start"]
            if name == "ui" and len(prompts) > len(dismissed):
                # Wait for actual native NeedsInput before replying to the real dialog.
                if (self.state(pane)["reduced"]["status"] == "needs_input"
                        and self.state(pane)["reduced"]["pending_requests"][0]["id"] not in request_ids):
                    request = self.state(pane)["reduced"]["pending_requests"]
                    request_ids.add(request[0]["id"])
                    assert len(request) == 1 and request[0]["kind"] != "permission"
                    assert "detail" not in request[0]
                    visible += 1
                    # Pi emits prompt_start before installing the TUI component.
                    # As in P0, let it render before injecting the reply key.
                    time.sleep(.12)
                    self.escape(pane)
                    dismissed.add(len(prompts))
            # No native completion before final settlement in independent real traces.
            if not any(e["event"] == "agent_settled" for e in events):
                assert self.state(pane)["completion_revision"] == before, "premature Done"
            return any(e["event"] == "agent_settled" for e in events)
        self.wait(settled, seconds=35)
        expected = EXPECTED.get(name, "done")
        self.wait(lambda: self.state(pane)["reduced"]["status"] == expected
                  and self.state(pane)["completion_revision"] == before + int(expected == "done")
                  and self.state(pane)["record_revision"] > initial["record_revision"] + 1, seconds=20)
        self.snapshot()  # Capture terminal transaction if it raced the wait's pre-predicate sample.
        after = self.state(pane)["completion_revision"]
        assert after == before + (1 if expected == "done" else 0)
        trace = self.events(pane)[start:]
        counts = dict(Counter(e["event"] for e in trace if e["source"] == "observer"))
        assert counts.get("before_agent_start") == counts.get("agent_settled") == 1
        assert any(s["status"] == "working" for s in self.samples[offset:] if s["pane"] == pane)
        if name == "ui":
            assert visible == 5
        if name == "ui_nested":
            assert counts.get("ui_prompt_start") == counts.get("ui_prompt_end") == 1
            assert any(s["status"] == "needs_input" for s in self.samples[offset:])
        self.record(name, {"expected_status": expected, "completion_before": before, "completion_after": after,
                          "no_premature_completion": True, "event_counts": counts, "pane": pane,
                          "settlements_before": settled_before,
                          "native_dialogs_observed_before_reply": visible,
                          "provider_requests": self.provider.observations()[providers:]}, self.samples[offset:])

    def metadata_action(self, operation, event):
        start = len(self.events())
        state = self.state()
        revision, instance = state["completion_revision"], state["agent_instance_id"]
        epoch = state["sources"]["pi"]["source_epoch"]
        old_conversation = state["reduced"].get("conversation_id")
        self.send("/probe " + operation)
        self.wait(lambda: any(e["event"] == event for e in self.events()[start:]))
        if operation in ("new", "resume", "fork", "reload"):
            self.wait(lambda: self.state()["sources"]["pi"]["source_epoch"] > epoch)
        elif operation == "name":
            self.wait(lambda: self.state()["reduced"].get("conversation_title") == "PRIVATE_SESSION_NAME_SENTINEL")
        else:
            self.wait(lambda: self.state()["record_revision"] > state["record_revision"]
                      and "latest_completion" not in self.state()["reduced"]
                      and "current_turn_id" not in self.state()["reduced"])
        state = self.state()
        assert state["completion_revision"] == revision
        assert state["agent_instance_id"] == instance
        if operation in ("new", "fork"):
            assert state["reduced"]["conversation_id"] != old_conversation
        self.record(operation, {"stable_process_row": True, "no_historical_done": True,
                                "completion_revision": revision, "conversation": opaque(state["reduced"].get("conversation_id")),
                                "epoch_advanced": state["sources"]["pi"]["source_epoch"] > epoch})

    def park(self, host):
        self.native("navigate", "focus", "--host", host, "--session", self.session, "--pane", "1")

    def visit(self, host, pane=0):
        state = self.state(pane)
        self.native("navigate", "agent", "--host", host, "--instance", state["agent_instance_id"],
                    "--revision", str(state["completion_revision"]))
        self.wait(lambda: self.ack(host).get(state["agent_instance_id"], {}).get("acknowledged_revision") == state["completion_revision"])

    def two_hosts(self):
        self.run_case("parallel")
        state = self.state()
        instance, revision = state["agent_instance_id"], state["completion_revision"]
        assert not self.ack("a") and not self.ack("b")
        self.visit("a")
        assert not self.ack("b")
        self.visit("b")
        assert self.ack("a")[instance]["acknowledged_revision"] == self.ack("b")[instance]["acknowledged_revision"] == revision
        self.park("a")
        self.park("b")
        self.record("two_hosts", {"host_local_acknowledgements": True, "observed_revision_only": True,
                                  "completion_revision": revision, "unvisited_peer_preserved": True})

    def input_visit(self):
        start = len(self.events())
        revision = self.state()["completion_revision"]
        self.send("/probe idle_ui")
        self.wait(lambda: self.state()["reduced"]["status"] == "needs_input")
        request = self.state()["reduced"]["pending_requests"]
        self.native("navigate", "agent", "--host", "a", "--instance", self.state()["agent_instance_id"])
        assert self.state()["reduced"]["pending_requests"] == request
        self.escape()
        self.wait(lambda: any(e["event"] == "ui_prompt_end" for e in self.events()[start:]))
        self.wait(lambda: not self.state()["reduced"]["pending_requests"])
        assert self.state()["completion_revision"] == revision
        self.park("a")
        self.record("input_visit", {"visit_preserves_request": True, "no_permission_provenance": True,
                                     "closing_dialog_does_not_complete": True})

    def reconnect(self):
        state = self.state()
        revision, instance = state["completion_revision"], state["agent_instance_id"]
        epoch = state["sources"]["pi"]["source_epoch"]
        completed = state["reduced"]["latest_completion"]["completed_at_ms"]
        self.discover()
        reporters = [pid for pid, i in self.known.items() if i["role"] == "reporter" and birth(pid) == (pid, i["birth"])]
        assert len(reporters) == 1
        os.kill(reporters[0], signal.SIGKILL)
        start = time.monotonic()
        self.wait(lambda: self.state()["sources"]["pi"]["source_epoch"] > epoch, seconds=10)
        self.wait(lambda: self.state()["reduced"]["status"] == "done")
        assert self.state()["completion_revision"] == revision
        assert self.state()["reduced"]["latest_completion"]["completed_at_ms"] == completed
        assert self.ack("a")[instance]["acknowledged_revision"] == revision
        self.record("reconnect", {"no_replayed_done": True, "same_completed_at": True,
                                  "reconnect_seconds": round(time.monotonic() - start, 3), "epoch_advanced": True})

    def loss(self):
        # Reporter binary is a PRIVATE copy. Prevent reconnect long enough to
        # observe the real sidebar's lease expiry while Pi continues its work.
        self.wait(lambda: self.state()["reduced"]["status"] == "idle")
        self.discover()
        state = self.state()
        epoch, revision = state["sources"]["pi"]["source_epoch"], state["completion_revision"]
        self.reporter.rename(self.root / "reporter-off")
        reporters = [pid for pid, i in self.known.items() if i["role"] == "reporter" and birth(pid) == (pid, i["birth"])]
        assert len(reporters) == 1
        os.kill(reporters[0], signal.SIGKILL)
        start = time.monotonic()
        events_at = len(self.events())
        self.send("/probe run success")
        self.wait(lambda: any(e["event"] == "agent_settled" for e in self.events()[events_at:]), seconds=10)
        self.wait(lambda: time.time() * 1000 - self.state()["sources"]["pi"]["observed_at_ms"] > 15000, seconds=22)
        # Lease degradation is a watcher/read-side projection, not a persisted
        # fabricated source transaction. Verify the actual sidebar presentation.
        self.wait(lambda: all("? pi" in self.sidebar(host)
                              for host in ("a", "b")), seconds=8)
        elapsed = time.monotonic() - start
        assert self.state()["identity"]["process"]["pid"] and self.state()["completion_revision"] == revision
        (self.root / "reporter-off").rename(self.reporter)
        self.wait(lambda: self.state()["sources"]["pi"]["source_epoch"] > epoch, seconds=35)
        # The genuinely new run while disconnected may deliver exactly one Done.
        self.wait(lambda: self.state()["completion_revision"] == revision + 1, seconds=20)
        self.record("reporter_loss", {"pi_kept_working": True, "live_row_unknown": True,
                                      "lease_degradation_seconds": round(elapsed, 3), "recovered_new_run_once": True})

    def second_pane(self):
        pane = self.action(self.session, "new-pane", "--tab-id", "0", "--no-focus", "--", sys.executable,
                           HERE / "launch.py", self.root / "launch.json")
        pane = int(pane.removeprefix("terminal_"))
        self.wait(lambda: self.state(pane) and self.state(pane)["reduced"]["status"] == "idle")
        self.wait(lambda: any(e["event"] == "fixture_ready" for e in self.events(pane)))
        first, second = self.state(), self.state(pane)
        assert first["identity"]["process"]["pid"] != second["identity"]["process"]["pid"]
        assert first["reduced"]["conversation_id"] != second["reduced"]["conversation_id"]
        assert not any(s["identity"]["is_synthetic"] for s in self.states())
        self.native("agent", "verify", "--session", self.session, "--pane", str(pane), "--pid", str(second["identity"]["process"]["pid"]))
        self.record("two_processes", {"same_cwd": True, "separate_verified_rows": True, "separate_conversations": True,
                                      "panes": [0, pane], "synthetic_inventory": False})
        return pane

    def detached(self):
        count = len(self.states())
        self.send("/p4 detach")
        meta = self.wait(lambda: json.loads((self.root / "detached.json").read_text()))
        self.remember(meta["pid"], "detached_pi")
        self.wait(lambda: (self.root / "detached-trace.ndjson").exists())
        result = self.run([self.cli, "agent", "verify", "--session", self.session, "--pane", str(meta["candidate_pane"]),
                           "--pid", str(meta["pid"])], check=False)
        assert result.returncode != 0
        # Actual TUI adapter attempts registration and is rejected too.
        self.wait(lambda: (self.root / "detached-done").exists(), seconds=15)
        assert len(self.states()) == count
        self.record("inherited_background", {"actual_pi_tui": True, "inherited_pane_hint": True,
                                              "foreground_binding_rejected": True, "no_extra_row": True})

    def topology(self, second):
        instance = self.state()["agent_instance_id"]
        self.action(self.session, "rename-pane", "--pane-id", "terminal_0", "P4 renamed")
        self.native("navigate", "focus", "--host", "a", "--session", self.session, "--pane", "0")
        self.action(self.session, "move-pane", "right")
        self.action(self.session, "toggle-pane-embed-or-floating", "--pane-id", "terminal_0")
        self.wait(lambda: next(p for p in self.inventory()["panes"] if p["terminal_id"] == 0)["is_floating"])
        self.park("a")
        self.native("navigate", "agent", "--host", "a", "--instance", instance)
        self.action(self.session, "toggle-pane-embed-or-floating", "--pane-id", "terminal_0")
        self.action(self.session, "stack-panes", "--", "terminal_0", f"terminal_{second}")
        self.park("a")
        self.native("navigate", "agent", "--host", "a", "--instance", instance)
        assert self.state()["agent_instance_id"] == instance
        self.park("a")
        self.record("pane_targeting", {"rename": True, "move": True, "floating": True, "stacked": True,
                                       "background_tab_activation": True, "immutable_instance_preserved": True,
                                       "sidebar_selection": "native activation verified; keyboard selection not qualified"})

    def modes(self):
        count = len(self.states())
        modes = {"print": ["--print", "PRIVATE_USER_PROMPT_SENTINEL"],
                 "json": ["--mode", "json", "--print", "PRIVATE_USER_PROMPT_SENTINEL"],
                 "rpc": ["--mode", "rpc"], "help": ["--help"], "version": ["--version"],
                 "export": ["--export", (self.root / "session-0").read_text()]}
        for mode, flags in modes.items():
            args = [*self.command, "--offline", "--no-builtin-tools", "--no-skills", "--no-context-files", "--no-mcp", *flags]
            metrics = self.root / f"mode-{mode}-metrics"
            metrics.mkdir(mode=0o700)
            child = subprocess.Popen(args, cwd=self.root, env=dict(self.env, ZELLIJ_PANE_ID="0", ZELLIJ_SESSION_NAME=self.session,
                                     VERIJ_RESOURCE_METRICS_DIR=str(metrics)),
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
            try:
                # RPC exercises session_start, then exits on EOF.
                output, _ = child.communicate("" if mode == "rpc" else None, timeout=20)
                assert child.returncode == 0
                assert '"source_epoch"' not in output and '"ready":true' not in output
                assert len(self.states()) == count
                assert not list(metrics.glob("native-*.json")), "non-TUI spawned reporter"
            finally:
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=3)
        metrics = self.root / "mode-discovery-metrics"
        metrics.mkdir(mode=0o700)
        disabled = self.action(self.session, "new-pane", "--tab-id", "1", "--no-focus", "--", sys.executable,
                               HERE / "launch.py", self.root / "launch.json", "disabled")
        disabled = int(disabled.removeprefix("terminal_"))
        self.wait(lambda: any(e["event"] == "session_start" for e in self.events(disabled)))
        assert len(self.states()) == count and not list(metrics.glob("native-*.json"))
        self.action(self.session, "close-pane", "--pane-id", f"terminal_{disabled}")
        self.record("non_tui", {"modes": list(modes), "no_rows": True, "no_protocol_contamination": True,
                                "disabled_discovery_no_reporter": True,
                                "unsupported_versions": "guard tests only; no other release runtime qualification",
                                "no_reporter_invocations": True})

    def lifetimes(self, second):
        old = self.state(second)
        os.kill(old["identity"]["process"]["pid"], signal.SIGKILL)
        self.wait(lambda: self.state(second) is None, seconds=15)
        self.action(self.session, "close-pane", "--pane-id", f"terminal_{second}")
        # Relaunch normally in the same original pane. Old host
        # acknowledgements cannot transfer to the new process lifetime.
        original = self.state()
        self.send("/probe quit")
        self.wait(lambda: self.state() is None, seconds=15)
        self.wait(lambda: all(" pi " not in self.sidebar(host)
                              for host in ("a", "b")), seconds=8)
        import shlex
        self.send(shlex.join([sys.executable, str(HERE / "launch.py"), str(self.root / "launch.json")]))
        self.wait(lambda: self.state() and self.state()["reduced"]["status"] == "idle")
        fresh = self.state()
        assert fresh["agent_instance_id"] != original["agent_instance_id"]
        assert fresh["completion_revision"] == 0
        assert all(fresh["agent_instance_id"] not in self.ack(host) for host in ("a", "b"))
        self.action(self.session, "close-pane", "--pane-id", "terminal_0")
        self.wait(lambda: not self.states(), seconds=15)
        self.wait(lambda: all(" pi " not in self.sidebar(host)
                              for host in ("a", "b")), seconds=8)
        self.record("lifetimes", {"kill_removed_row": True, "quit_removed_row": True, "pane_close_removed_row": True,
                                  "same_pane_new_process_new_identity": True, "no_ack_transfer": True,
                                  "forced_pid_reuse_and_session_resurrection": "not exercised"})

    def execute(self):
        self.start()
        self.wait(lambda: self.state()["reduced"]["status"] == "idle")
        state = self.state()
        assert state["identity"]["kind"] == "pi" and not state["identity"]["is_synthetic"]
        self.native("agent", "verify", "--session", self.session, "--pane", "0", "--pid", str(state["identity"]["process"]["pid"]))
        self.record("startup", {"owned_autoload": True, "real_exporter": True, "foreground_verified": True,
                                 "completion_revision": state["completion_revision"], "row_count": len(self.states())})
        self.hosts()
        doctor = self.native("agent", "doctor", "pi", "--config-dir", self.root / "agent", "--pi-bin", self.pi_probe)
        assert doctor["installation_ready"] and doctor["adapter_loaded"]
        for host in ("a", "b"):
            screen = self.sidebar(host)
            assert "Pi" in screen or "pi" in screen
        self.record("late_sidebars", {"one_pi_row": True, "doctor_loaded": True, "two_sidebar_displays": True})
        self.two_hosts()
        self.reconnect()
        time.sleep(3)  # Allow metric publication and one-off Workspace title updates.
        heartbeat = self.measure("acknowledged_heartbeat")
        assert self.state()["completion_revision"] == 1
        assert heartbeat["counters"].get("proof_attempts", 0) == 0
        self.metadata_action("reload", "session_start")
        assert self.state()["completion_revision"] == 1
        self.park("a"); self.park("b")
        idle = self.measure("idle")
        assert idle["counters"].get("proof_attempts", 0) == 0
        for name in CASES:
            self.run_case(name)
        # Manual compaction/name/tree/session replacement do not mint Done.
        self.run_case("success")
        revision = self.state()["completion_revision"]
        self.send("/probe compact manual_compact")
        self.wait(lambda: any(e["event"] == "manual_compact_done" for e in self.events()[-12:]))
        self.wait(lambda: self.state()["reduced"]["status"] != "working")
        assert self.state()["completion_revision"] == revision
        self.record("manual_compact", {"no_completion": True})
        self.metadata_action("name", "session_info_changed")
        self.input_visit()
        self.metadata_action("tree", "session_tree")
        self.metadata_action("new", "session_start")
        self.metadata_action("resume", "session_start")
        self.metadata_action("fork", "session_start")
        self.loss()
        second = self.second_pane()
        time.sleep(3)
        idle = self.measure("two_panes_two_hosts_idle")
        assert idle["counters"].get("proof_attempts", 0) == 0
        assert all(k.endswith(":list-sessions") for k in idle["counters"] if k.startswith("cli/"))
        assert idle["counters"].get("native_record_writes", 0) <= 2 * (idle["seconds"] / 5 + 2)
        self.discover()
        assert sum(1 for pid, i in self.known.items() if i["role"] == "reporter" and birth(pid) == (pid, i["birth"])) == 2
        self.detached()
        self.topology(second)
        self.modes()
        before = {s["agent_instance_id"]: s["completion_revision"] for s in self.states()}
        offsets = {pane: len(self.events(pane)) for pane in (0, second)}
        self.send("/p4 burst")
        self.send("/p4 burst", second)
        burst = self.measure("two_panes_tool_burst", seconds=max(6, self.seconds))
        self.wait(lambda: all((self.root / f"burst-done-{pane}").exists() for pane in (0, second)))
        self.wait(lambda: all(s["completion_revision"] == before[s["agent_instance_id"]] + 1 for s in self.states()))
        counts = {str(pane): dict(Counter(e["event"] for e in self.events(pane)[offsets[pane]:]
                  if e["source"] == "observer")) for pane in (0, second)}
        assert all(c["tool_execution_start"] == c["tool_execution_end"] == 161
                   and c["agent_settled"] == 1 for c in counts.values())
        # Counter snapshots have 1s resolution, with two extra handshake writes
        # tolerated. Deterministic transport tests enforce each 100ms interval.
        assert burst["counters"].get("native_record_writes", 0) <= 2 * (10 * burst["seconds"] + 4)
        self.record("burst_budget", {"nested_tools_per_pane": 160, "exactly_one_aggregate_completion": True,
                                      "event_counts_by_pane": counts, "writes_within_two_reporter_budget": True,
                                      "queue_and_timer_bounds": "deterministic transport tests; no runtime introspection"})
        self.lifetimes(second)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--plugin", type=Path, required=True)
    parser.add_argument("--pi-bin", default="pi")
    parser.add_argument("--node-cli", type=Path)
    parser.add_argument("--runtime", choices=("compiled", "node", "both"), default="both")
    parser.add_argument("--seconds", type=float, default=6)
    parser.add_argument("--output", type=Path, default=HERE / "results.json")
    parser.add_argument("--debug", action="store_true", help="Private diagnostic output; never retained")
    args = parser.parse_args()
    args.cli = args.cli.resolve(strict=True)
    args.plugin = args.plugin.resolve(strict=True)
    if args.seconds < 5 or args.seconds > 60:
        parser.error("measurement seconds must be between 5 and 60")
    targets = []
    if args.runtime in ("compiled", "both"):
        targets.append(("compiled", [str(Path(shutil.which(args.pi_bin) or args.pi_bin).resolve(strict=True))]))
    if args.runtime in ("node", "both"):
        if not args.node_cli:
            parser.error("--node-cli required for private Node distribution")
        targets.append(("node", [str(Path(shutil.which("node")).resolve()), str(args.node_cli.resolve(strict=True))]))
    report = {"schema_version": 1, "scope": "P4 actual stock Zellij/Pi/exporter/two sidebars; loopback provider and active test extension",
              "assets": {str(path.relative_to(REPO)) if path.is_relative_to(REPO) else path.name: sha(path) for path in
                         [args.cli.resolve(), args.plugin.resolve(), *sorted(HERE.glob("*.py")), HERE / "fixture.ts",
                          REPO / "integrations/pi/index.ts", REPO / "integrations/pi/core.mjs",
                          REPO / "prototypes/pi-runtime/provider.py", REPO / "prototypes/pi-runtime/fixture.ts",
                          REPO / "prototypes/pi-runtime/observer.ts", REPO / "prototypes/pi-runtime/trace.mjs"]},
              "runs": [], "limitations": ["No forced PID/server reuse or serialized session resurrection",
              "No keyboard sidebar stable-selection qualification", "No other-release runtime",
              "No status-bar amplification or long-duration stress acceptance; broad resource roadmap remains pending",
              "Queue/timer bounds are deterministic tests, not runtime telemetry", "No remote hosts; hosts are two local Verij outer sessions"]}
    for name, command in targets:
        runtime = Acceptance(args.cli, args.plugin, command, args.seconds)
        runtime.report.update(runtime=name, no_global_installation=True)
        report["runs"].append(runtime.report)
        try:
            # Private executable permits fault injection without touching built/global assets.
            runtime.reporter = runtime.root / "reporter"
            shutil.copy2(runtime.cli, runtime.reporter)
            runtime.env["VERIJ_BIN"] = str(runtime.reporter)
            runtime.report.update(pi_version=runtime.run([*command, "--version"]),
                                  executable_sha256=sha(Path(command[-1])),
                                  zellij_version=runtime.run([runtime.zellij, "--version"]), zellij_sha256=sha(runtime.zellij),
                                  environment_allowlist=sorted(runtime.env))
            if runtime.report["pi_version"] != "1.2.0":
                raise RuntimeError("unqualified Pi version")
            runtime.execute()
            runtime.report["passed"] = True
        except Exception as error:
            runtime.report.update(passed=False, failure=type(error).__name__)
            if args.debug:
                traceback.print_exc()
                for terminal in runtime.terminals:
                    try:
                        print(runtime.mux("capture-pane", "-p", "-t", terminal)[-5000:])
                    except Exception:
                        pass
        finally:
            runtime.close()
        if not runtime.report["passed"]:
            break
    report["complete"] = len(report["runs"]) == len(targets) and all(r["passed"] and all(
        v for k, v in r["cleanup"].items() if k != "forced_kills") for r in report["runs"])
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"complete": report["complete"], "runs": len(report["runs"]), "output": str(args.output)}))
    return 0 if report["complete"] else 1


if __name__ == "__main__":
    sys.exit(main())
