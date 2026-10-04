#!/usr/bin/env python3
"""Private H1 production CLI/plugin topology and routing harness.

Receiver processes are synthetic control targets, not real agent adapters.
Only generated config, private XDG roots and private sockets are used.
"""
import argparse
import hashlib
import importlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import time

SOURCES = Path(__file__).resolve().parents[1] / "stock-navigation"
RECEIVER = Path(__file__).with_name("receiver.py").resolve()
sys.path.insert(0, str(SOURCES))
controller = importlib.import_module("controller")


def run(argv, env, root, timeout=15, data=None):
    result = subprocess.run([str(arg) for arg in argv], cwd=root, env=env,
                            input=data, capture_output=True, text=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError((argv[:4], result.returncode, result.stderr[-1200:], result.stdout[-500:]))
    return result.stdout.strip()


def wait(predicate, seconds=30):
    end = time.monotonic() + seconds
    last = None
    while time.monotonic() < end:
        try:
            result = predicate()
            if result:
                return result
        except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
            last = error
        time.sleep(.1)
    raise TimeoutError(str(last))


def environment(root, socket):
    env = {key: value for key, value in os.environ.items() if not key.startswith(("ZELLIJ", "TMUX", "VERIJ"))}
    for kind in ("CONFIG", "DATA", "CACHE", "STATE"):
        env[f"XDG_{kind}_HOME"] = str(root / kind.lower())
    env.update(TERM="xterm-256color", ZELLIJ_SOCKET_DIR=str(socket),
               VERIJ_STATES_DIR=str(root / "states"), VERIJ_CONTROL_DIR=str(root / "control"),
               VERIJ_AGENT_STATE_DIR=str(root / "agents"), VERIJ_WASI_CONTROL_DIR="/host/control",
               VERIJ_WASI_STATES_DIR="/host/states")
    return env


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--plugin", type=Path, required=True)
    parser.add_argument("--scratch-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--keep-live", action="store_true", help="Retain only a passing private instance for human H1 review")
    args = parser.parse_args()
    cli, plugin = args.cli.resolve(strict=True), args.plugin.resolve(strict=True)
    binary_path = shutil.which("zellij")
    if binary_path is None:
        raise RuntimeError("stock zellij is not installed")
    binary = Path(binary_path).resolve(strict=True)
    assert "workspaces/verij-agent-monitoring/zellij/" not in str(binary)
    root = Path(tempfile.mkdtemp(prefix="vj-h1-", dir=args.scratch_dir)).resolve()
    root.chmod(0o700)
    sockets = Path(tempfile.mkdtemp(prefix="vj-h1-sock-")).resolve()
    env = environment(root, sockets)
    sessions = ["h1-shared", "h1-other", "h1-host-a", "h1-host-b"]
    report = {"status": "PASS", "root": str(root), "stock_binary": str(binary),
              "stock_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "checks": [], "human_gate_approved": False}
    known = {}
    tmux = ["tmux", "-f", "/dev/null", "-S", str(sockets / "terminal")]
    def mux(*parts): return run([*tmux, *parts], env, root)
    def action(session, *parts): return run([binary, "-s", session, "action", *parts], env, root)
    def native(*parts, data=None): return json.loads(run([cli, *parts], env, root, data=data))
    def inventory(session):
        records = native("inventory", "--session", session)
        return records[0]["inventory"] if records and records[0].get("inventory") else None
    def seed(session, config):
        command = [binary, "--config", config, "--layout", root / "inner-layout.kdl", "attach", "-c", session,
                   "--", sys.executable, RECEIVER, root, f"{session}-0"]
        mux("new-session", "-d", "-s", session, "-x", "180", "-y", "55", "-c", str(root), "exec " + shlex.join(map(str, command)))
        wait(lambda: f"STOCK DEMO TARGET {session}-0" in mux("capture-pane", "-p", "-t", session))
    def token(host, pane, value):
        mux("send-keys", "-t", f"host-{host}", "-l", value)
        mux("send-keys", "-t", f"host-{host}", "Enter")
        wait(lambda: value in (root / f"input-h1-shared-{pane}").read_text())
    try:
        for directory in ("states", "control", "agents", "cache/zellij"):
            (root / directory).mkdir(parents=True, exist_ok=True)
        url = "file:" + str(plugin)
        (root / "cache/zellij/permissions.kdl").write_text("\n".join(
            json.dumps(name) + " {\n ReadApplicationState\n ChangeApplicationState\n ReadCliPipes\n}\n"
            for name in (url, str(plugin))))
        base = ('default_shell "/bin/sh"\nshow_startup_tips false\nshow_release_notes false\n'
                'mirror_session false\nsession_serialization false\n'
                f'load_plugins {{\n {json.dumps(url)} {{\n state_dir "/host/states"\n control_dir "/host/control"\n }}\n}}\n'
                f'keybinds {{\n normal {{\n bind "Ctrl b" {{\n LaunchOrFocusPlugin {json.dumps(url)} {{\n floating true\n move_to_focused_tab true\n registration_surface true\n state_dir "/host/states"\n control_dir "/host/control"\n }}\n }}\n }}\n}}\n')
        (root / "inner.kdl").write_text(base)
        (root / "outer.kdl").write_text(base + 'default_mode "locked"\nnested_session_handling "descend"\n')
        (root / "inner-layout.kdl").write_text("layout {\n pane\n}\n")
        env["ZELLIJ_CONFIG_FILE"] = str(root / "inner.kdl")
        seed("h1-shared", root / "inner.kdl")
        seed("h1-other", root / "inner.kdl")
        for pane in (1,):
            assert action("h1-shared", "new-pane", "--", sys.executable, RECEIVER, root, f"h1-shared-{pane}") == f"terminal_{pane}"
        assert action("h1-shared", "new-pane", "--floating", "--", sys.executable, RECEIVER, root, "h1-shared-2") == "terminal_2"
        action("h1-shared", "new-tab", "--name", "stack", "--", sys.executable, RECEIVER, root, "h1-shared-3")
        assert action("h1-shared", "new-pane", "--", sys.executable, RECEIVER, root, "h1-shared-4") == "terminal_4"
        action("h1-shared", "stack-panes", "--", "terminal_3", "terminal_4")
        initial = wait(lambda: (value := inventory("h1-shared")) and len(value["panes"]) == 5 and value)
        assert initial["session_instance_id"] and all(p["pane_process"] for p in initial["panes"])
        assert {p["terminal_id"] for p in initial["panes"]} == set(range(5))
        assert next(p for p in initial["panes"] if p["terminal_id"] == 2)["is_floating"]
        assert all(p["stack_id"] is None for p in initial["panes"])
        controller.atomic(root / "inventory-initial.json", initial)
        report["checks"].append("full_terminal_inventory_native_births_hidden_float_stack_unknown")
        for host in ("a", "b"):
            sidebar = [cli, "ui"]
            workspace = [cli, "workspace", "h1-shared", "--host", host, "--plugin", plugin, "--client-config", root / "inner.kdl"]
            layout = ('layout {\n pane split_direction="vertical" {\n'
                      + f'pane size="28%" name="Verij H1" command={json.dumps(str(cli))} {{\n args "ui";\n}}\n'
                      + f'pane name="Workspace" command={json.dumps(str(cli))} {{\n args '
                      + ' '.join(json.dumps(str(arg)) for arg in workspace[1:]) + ';\n}\n}\n}\n')
            (root / f"host-{host}.kdl").write_text(layout)
            cmd = [binary, "--config", root / "outer.kdl", "--layout", root / f"host-{host}.kdl", "attach", "-c", f"h1-host-{host}"]
            mux("new-session", "-d", "-s", f"host-{host}", "-x", "180", "-y", "55", "-c", str(root), "exec " + shlex.join(map(str, cmd)))
            marker = wait(lambda: json.loads((root / f"control/attachment-{host}.json").read_text()))
            wait(lambda: "STOCK DEMO TARGET h1-shared" in mux("capture-pane", "-p", "-t", f"host-{host}"))
            known[marker["attachment_process"]["pid"]] = marker["attachment_process"]
            registration = native("navigate", "register", "--host", host, "--host-session", f"h1-host-{host}",
                                  "--workspace-pane", "1", "--client-pid", str(marker["attachment_process"]["pid"]),
                                  "--session", "h1-shared", "--plugin", str(plugin))
            controller.atomic(root / f"registration-{host}.json", registration)
        a = json.loads((root / "registration-a.json").read_text())
        b = json.loads((root / "registration-b.json").read_text())
        assert a["context"]["client_id"] != b["context"]["client_id"]
        report["checks"].append("two_owned_workspace_wrappers_exact_keyboard_registration")
        for host, pane in (("a", 0), ("b", 1), ("a", 2), ("a", 3), ("a", 4)):
            result = native("navigate", "focus", "--host", host, "--session", "h1-shared", "--pane", str(pane))
            assert result["status"] == "inner_focus_observed" and not result["acknowledge_done"]
            token(host, pane, f"h1-{host}-pane-{pane}")
        report["checks"].append("tiled_float_stack_actual_outer_keyboard")
        native("navigate", "focus", "--host", "a", "--pane", "0")
        mux("send-keys", "-t", "host-a", "C-p", "f")
        wait(lambda: (value := inventory("h1-shared")) is not None and any(p["terminal_id"] == 0 and p["is_fullscreen"] for p in value["panes"]))
        native("navigate", "focus", "--host", "a", "--pane", "1")
        token("a", 1, "h1-fullscreen-other-target")
        report["checks"].append("fullscreen_other_target_revealed_and_accepts_outer_input")
        journal = (root / "control/journal-a.json").read_bytes()
        query = native("navigate", "query", "--host", "a")
        assert query["status"] == "observed" and (root / "control/journal-a.json").read_bytes() == journal
        report["checks"].append("passive_query_does_not_allocate_sequence_or_ack")
        # Synthetic semantic records are explicitly marked; process/tty and
        # navigation below still come from real SDK/process observations.
        native("navigate", "focus", "--host", "a", "--pane", "0")
        inv = inventory("h1-shared")
        assert inv is not None
        process = next(p for p in inv["panes"] if p["terminal_id"] == 0)["pane_process"]
        verified = native("agent", "verify", "--session", "h1-shared", "--pane", "0", "--pid", str(process["pid"]))
        assert verified["foreground_binding_verified"]
        token("a", 0, "h1-spawn-redirected")
        child = wait(lambda: json.loads((root / "redirected-child.json").read_text()))
        known[child["pid"]] = controller.birth(child["pid"])
        rejected = subprocess.run([str(cli), "agent", "verify", "--session", "h1-shared", "--pane", "0", "--pid", str(child["pid"])],
                                  cwd=root, env=env, capture_output=True, text=True, timeout=15)
        assert rejected.returncode and "redirected" in rejected.stderr
        report["checks"].append("actual_foreground_tty_verified_redirected_same_family_child_rejected")
        registered = native("agent", "register", "--session", "h1-shared", "--pane", "0", "--pid", str(process["pid"]),
                            "--kind", "opencode", "--synthetic")
        instance = registered["agent_instance_id"]
        source = {"schema_version": 1, "source_id": "fixture-opencode", "source_epoch": 1,
                  "source_revision": 1, "turn_epoch": 1, "turn_revision": 1, "turn_id": "fixture-turn-1",
                  "activity": "working"}
        working = native("agent", "report", "--instance", instance, data=json.dumps(source))
        assert working["reduced_status"] == "working" and working["completion_revision"] == 0
        source.update(source_revision=2, activity="idle", successful_completion={"turn_id": "fixture-turn-1",
                      "turn_epoch": 1, "turn_revision": 1, "completed_at_ms": time.time_ns() // 1_000_000})
        done = native("agent", "report", "--instance", instance, data=json.dumps(source))
        assert done["reduced_status"] == "done" and done["completion_revision"] == 1
        ack = native("agent", "ack", "--instance", instance, "--host", "a", "--revision", "1")
        assert ack["acknowledged_revision"] == 1
        assert not (root / "state/verij/agent-monitoring/hosts/b.json").exists()
        source.update(source_revision=3, turn_revision=2, turn_id="fixture-turn-2",
                      successful_completion={"turn_id": "fixture-turn-2", "turn_epoch": 1, "turn_revision": 2,
                      "completed_at_ms": time.time_ns() // 1_000_000})
        done2 = native("agent", "report", "--instance", instance, data=json.dumps(source))
        assert done2["completion_revision"] == 2
        old_ack = native("agent", "ack", "--instance", instance, "--host", "a", "--revision", "1")
        assert old_ack["acknowledged_revision"] == 1
        native("navigate", "focus", "--host", "a", "--pane", "1")
        refused = subprocess.run([str(cli), "agent", "ack", "--instance", instance, "--host", "a", "--revision", "2"],
                                 env=env, cwd=root, capture_output=True, text=True, timeout=15)
        assert refused.returncode and "another pane" in refused.stderr
        report["checks"].append("synthetic_reducer_real_native_visit_host_local_ack_race_and_wrong_pane_refusal")
        native("navigate", "focus", "--host", "a", "--pane", "0")
        birth_before = json.loads((root / "control/attachment-a.json").read_text())["attachment_process"]
        # Sidebar focus and selection do not count as a visit. The existing
        # production session action must refuse a second owned attach too.
        action("h1-host-a", "focus-pane-id", "terminal_0")
        refused_visit = subprocess.run([str(cli), "agent", "ack", "--instance", instance, "--host", "a", "--revision", "2"],
                                      env=env, cwd=root, capture_output=True, text=True, timeout=15)
        assert refused_visit.returncode and "visit unavailable" in refused_visit.stderr
        mux("send-keys", "-t", "host-a", "g", "Enter")
        time.sleep(.5)
        assert json.loads((root / "control/attachment-a.json").read_text())["attachment_process"] == birth_before
        native("navigate", "focus", "--host", "a", "--pane", "0")
        token("a", 0, "h1-owned-attachment-still-single")
        report["checks"].append("sidebar_focus_not_visit_and_initial_attachment_not_reinjected")
        # Public source-session switch retains its attachment PID; destination
        # registration restores exact new server/client context.
        switched = native("navigate", "switch", "--host", "a", "--session", "h1-other", "--pane", "0")
        assert switched["status"] == "switch_dispatched_unverified"
        peer = native("navigate", "query", "--host", "b")
        assert peer["observation"]["session_name"] == "h1-shared"
        returned = native("navigate", "switch", "--host", "a", "--session", "h1-shared", "--pane", "0")
        assert returned["status"] == "switch_dispatched_unverified"
        refocused = native("navigate", "focus", "--host", "a", "--pane", "0")
        assert refocused["status"] == "inner_focus_observed"
        token("a", 0, "h1-after-return-sequence-resynced")
        report["checks"].append("cross_session_rebind_existing_attachment_and_peer_preserved")
        before_reload = inventory("h1-shared")
        assert before_reload is not None
        def exporter_epochs():
            records = [json.loads(file.read_text()) for file in (root / "states").glob("*.json") if not file.name.startswith(".")]
            return {tuple(record["inventory"]["producer"][key] for key in ("server_pid", "plugin_id", "client_id")):
                    record["inventory"]["producer"]["epoch"] for record in records
                    if record["name"] == "h1-shared" and record.get("inventory") and record["inventory"]["producer"]["plugin_id"] == 0}
        old_epochs = exporter_epochs()
        assert old_epochs
        action("h1-shared", "start-or-reload-plugin", "--configuration", "state_dir=/host/states,control_dir=/host/control",
               "file:" + str(plugin))
        wait(lambda: any(key in old_epochs and epoch != old_epochs[key] for key, epoch in exporter_epochs().items()), seconds=30)
        after_reload = wait(lambda: inventory("h1-shared"))
        assert after_reload["session_instance_id"] == before_reload["session_instance_id"]
        assert {p["terminal_id"] for p in after_reload["panes"]} == set(range(5))
        report["checks"].append("public_plugin_reload_preserves_native_server_pane_identity")
        native("navigate", "focus", "--host", "a", "--pane", "0")
        before = inventory("h1-shared")
        assert before is not None
        action("h1-shared", "rename-pane", "--pane-id", "terminal_2", "renamed hidden float")
        wait(lambda: (value := inventory("h1-shared")) is not None and next(p for p in value["panes"] if p["terminal_id"] == 2)["title"] == "renamed hidden float")
        action("h1-shared", "rename-session", "h1-renamed")
        sessions[sessions.index("h1-shared")] = "h1-renamed"
        renamed = wait(lambda: inventory("h1-renamed"))
        assert renamed["session_instance_id"] == before["session_instance_id"]
        assert {p["terminal_id"] for p in renamed["panes"]} == set(range(5))
        assert [p["tab_id"] for p in renamed["panes"]] == [p["tab_id"] for p in before["panes"]]
        report["checks"].append("session_and_background_title_rename_preserve_server_pane_tab_identity")
        # A rename can replace stock routing/client context even though native
        # server identity survives. Re-establish the receipt before targeting.
        for host in ("a","b"):
            marker=json.loads((root / f"control/attachment-{host}.json").read_text())
            native("navigate","register","--host",host,"--host-session",f"h1-host-{host}",
                   "--workspace-pane","1","--client-pid",str(marker["attachment_process"]["pid"]),
                   "--session","h1-renamed","--plugin",str(plugin))
        renamed_focus=native("navigate","focus","--host","a","--pane","1")
        assert renamed_focus["status"]=="inner_focus_observed"
        token("a",1,"h1-after-rename-still-addressed")
        controller.atomic(root / "review-manifest.json", {"root": str(root), "socket_dir": str(sockets),
                          "tmux_socket": str(sockets / "terminal"), "cli": str(cli), "binary": str(binary),
                          "plugin": str(plugin), "sessions": sessions, "inner_session": "h1-renamed",
                          "synthetic_instance": instance, "known_births": known})
        report["status"] = "PASS"
    except Exception as error:
        report.update(status="FAIL", error=str(error))
        screens = {}
        for host in ("a", "b"):
            try:
                screens[host] = mux("capture-pane", "-p", "-t", f"host-{host}")
                (root / f"failure-host-{host}.txt").write_text(screens[host])
            except Exception:
                pass
        controller.atomic(root / "failure-screens.json", screens)
    finally:
        if args.keep_live and report["status"] == "PASS":
            report["review_live"] = True
            report["host_a_attach"] = f"tmux -S {sockets / 'terminal'} attach -t host-a"
            report["host_b_attach"] = f"tmux -S {sockets / 'terminal'} attach -t host-b"
            controller.atomic(root / "verification.json", report)
            args.output.write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report, indent=2))
        else:
            for file in (root / "control").glob("focus-*.json"):
                try:
                    pid = json.loads(file.read_text())["context"]["server_pid"]
                    if value := controller.birth(pid): known[pid] = value
                except (OSError, ValueError, KeyError): pass
            for session in sessions:
                subprocess.run([str(binary), "kill-session", session], cwd=root, env=env, capture_output=True, timeout=8)
            subprocess.run([*tmux, "kill-server"], capture_output=True, timeout=8)
            shutil.rmtree(sockets, ignore_errors=True)
            def exited(pid, expected):
                actual = controller.birth(pid)
                return actual is None or any(actual.get(key) != expected[key] for key in ("pid", "boot_id", "start_jiffies"))
            try:
                gone = wait(lambda: all(exited(pid, expected) for pid, expected in known.items()), seconds=10)
            except TimeoutError:
                gone = False
                report.update(status="FAIL", error="known private process births remain after cleanup")
            report["cleanup"] = {"private_socket_removed": not sockets.exists(), "known_processes_exited": bool(gone)}
            controller.atomic(root / "verification.json", report)
            args.output.write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report, indent=2))
    return 0 if report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
