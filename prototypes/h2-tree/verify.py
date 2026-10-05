#!/usr/bin/env python3
"""Private two-host H2 tree/completion fixture using production CLI and stock SDK."""
import argparse
import importlib
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import time

H1 = Path(__file__).resolve().parents[1] / "h1-navigation"
sys.path.insert(0, str(H1))
support = importlib.import_module("verify")
controller = support.controller
RECEIVER = H1 / "receiver.py"
SIDEBAR = Path(__file__).with_name("sidebar.py")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", required=True, type=Path)
    parser.add_argument("--plugin", required=True, type=Path)
    parser.add_argument("--scratch-dir", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--keep-live", action="store_true")
    args = parser.parse_args()
    cli, plugin = args.cli.resolve(strict=True), args.plugin.resolve(strict=True)
    binary = Path(shutil.which("zellij") or "zellij").resolve(strict=True)
    assert "workspaces/verij-agent-monitoring/zellij/" not in str(binary)
    root = Path(tempfile.mkdtemp(prefix="vj-h2-", dir=args.scratch_dir)).resolve()
    root.chmod(0o700)
    sockets = Path(tempfile.mkdtemp(prefix="vj-h2-sock-")).resolve()
    env = support.environment(root, sockets)
    env["XDG_RUNTIME_DIR"] = str(root / "runtime")
    sessions = ["h2-shared", "h2-host-a", "h2-host-b"]
    report = {"status": "PASS", "root": str(root), "checks": [], "semantic_records_synthetic": True,
              "human_h2_approved": False, "stock_binary": str(binary),
              "stock_sha256":hashlib.sha256(binary.read_bytes()).hexdigest()}
    known = {}
    stage = "private fixture setup"
    sources, instances = {}, {}
    tmux = ["tmux", "-f", "/dev/null", "-S", str(sockets / "terminal")]
    def run(argv, timeout=20, data=None): return support.run(argv, env, root, timeout, data)
    def mux(*parts): return run([*tmux, *parts])
    def action(session, *parts): return run([binary, "-s", session, "action", *parts])
    def native(*parts, data=None): return json.loads(run([cli, *parts], data=data))
    def screen(host): return mux("capture-pane", "-p", "-t", f"host-{host}")
    def side(host):
        mouse(host, 15, 10)
        support.wait(lambda: "terminal_0" in action(f"h2-host-{host}","list-clients"))
    def rows(host):
        geometry = next(pane for pane in json.loads(action(f"h2-host-{host}","list-panes","--all","--json")) if not pane["is_plugin"] and pane["id"] == 0)
        lines = screen(host).splitlines()[geometry["pane_content_y"]:geometry["pane_content_y"] + geometry["pane_content_rows"]]
        return [line[geometry["pane_content_x"]:geometry["pane_content_x"] + geometry["pane_content_columns"]] for line in lines]
    def mouse(host, row, column):
        geometry=next(pane for pane in json.loads(action(f"h2-host-{host}","list-panes","--all","--json")) if not pane["is_plugin"] and pane["id"]==0)
        for end in ("M","m"):
            wire=f"\x1b[<0;{geometry['pane_content_x']+column+1};{geometry['pane_content_y']+row+1}{end}".encode()
            mux("send-keys","-t",f"host-{host}","-H",*[f"{byte:02x}" for byte in wire]);time.sleep(.1)
    def find_agent(host, label):
        matches = [index for index, line in enumerate(rows(host)) if label in line and "[fixture]" in line]
        return matches[0] if matches else None
    def select_agent(host, label):
        side(host)
        index = support.wait(lambda: (index := find_agent(host, label)) is not None and (index + 1)) - 1
        mux("send-keys", "-t", f"host-{host}", "g", *("j" for _ in range(index)))
    def report_source(pane, **changes):
        sources[pane]["source_revision"] += 1
        sources[pane].update(changes)
        return native("agent", "report", "--instance", instances[pane], data=json.dumps(sources[pane]))
    def host_ack(host, pane):
        path = root / f"state/verij/agent-monitoring/hosts/{host}.json"
        if not path.exists(): return 0
        return json.loads(path.read_text())["acknowledgements"].get(instances[pane], {}).get("acknowledged_revision", 0)
    def has_done(host, label):
        index = find_agent(host, label)
        return index is not None and "✓" in rows(host)[index]
    def frame(name):
        for host in ("a", "b"):
            (root / f"{name}-host-{host}.txt").write_text(screen(host))
    def ready(host,process):
        value=json.loads((root / f"ui-ready-{host}.json").read_text())
        return all(value.get(key)==expected for key,expected in process.items())
    def ack_evidence(name):
        values={}
        for host in ("a","b"):
            path=root / f"state/verij/agent-monitoring/hosts/{host}.json"
            values[host]=json.loads(path.read_text()) if path.exists() else None
        controller.atomic(root / f"ack-{name}.json",values)
    try:
        for directory in ("states", "control", "agents", "runtime", "cache/zellij", "state/verij", "config/verij"):
            (root / directory).mkdir(parents=True, exist_ok=True)
        (root / "state/verij/host-registry.toml").write_text('[hosts."h2-host-a"]\nmarker_key="a"\n[hosts."h2-host-b"]\nmarker_key="b"\n')
        (root / "runtime").chmod(0o700)
        (root / "config/verij/config.toml").write_text('[agents]\ntitle_sources=["conversation","pane","agent"]\n[tui]\nsingle_click_action=true\n')
        url = "file:" + str(plugin)
        (root / "cache/zellij/permissions.kdl").write_text("\n".join(json.dumps(name) + ' {\n ReadApplicationState\n ChangeApplicationState\n ReadCliPipes\n}\n' for name in (url, str(plugin))))
        base = ('default_shell "/bin/sh"\nshow_startup_tips false\nshow_release_notes false\nmirror_session false\nsession_serialization false\n'
                f'load_plugins {{\n {json.dumps(url)} {{\n state_dir "/host/states"\n control_dir "/host/control"\n trace_control true\n }}\n}}\n'
                f'keybinds {{\n normal {{\n bind "Ctrl b" {{\n LaunchOrFocusPlugin {json.dumps(url)} {{\n floating true\n move_to_focused_tab true\n registration_surface true\n state_dir "/host/states"\n control_dir "/host/control"\n trace_control true\n }}\n }}\n }}\n}}\n')
        (root / "inner.kdl").write_text(base)
        (root / "outer.kdl").write_text(base + 'default_mode "locked"\nnested_session_handling "descend"\n')
        (root / "inner-layout.kdl").write_text('layout {\n pane\n}\n')
        env["ZELLIJ_CONFIG_FILE"] = str(root / "inner.kdl")
        seed = [binary, "--config", root / "inner.kdl", "--layout", root / "inner-layout.kdl", "attach", "-c", "h2-shared", "--", sys.executable, RECEIVER, root, "h2-shared-0"]
        mux("new-session", "-d", "-s", "seed", "-x", "180", "-y", "55", "-c", str(root), "exec " + shlex.join(map(str, seed)))
        support.wait(lambda: "STOCK DEMO TARGET" in mux("capture-pane", "-p", "-t", "seed"))
        action("h2-shared", "new-pane", "--", sys.executable, RECEIVER, root, "h2-shared-1")
        action("h2-shared", "new-pane", "--floating", "--", sys.executable, RECEIVER, root, "h2-shared-2")
        action("h2-shared", "new-tab", "--name", "stack", "--", sys.executable, RECEIVER, root, "h2-shared-3")
        action("h2-shared", "new-pane", "--", sys.executable, RECEIVER, root, "h2-shared-4")
        action("h2-shared", "stack-panes", "--", "terminal_3", "terminal_4")
        inventory = support.wait(lambda: (value := native("inventory", "--session", "h2-shared")) and value[0].get("inventory") and len(value[0]["inventory"]["panes"]) == 5 and value[0]["inventory"])
        controller.atomic(root / "inventory-initial.json", inventory)
        for pane in inventory["panes"]:
            if process:=pane.get("pane_process"):known[process["pid"]]=process
        for host in ("a", "b"):
            stage = f"register host {host}"
            workspace = ["workspace", "h2-shared", "--host", host, "--plugin", str(plugin), "--client-config", str(root / "inner.kdl")]
            layout = ('layout {\n pane split_direction="vertical" {\n'
                      + f'pane size="32%" name="H2 synthetic review" command={json.dumps(sys.executable)} {{\n args '
                      + ' '.join(json.dumps(str(arg)) for arg in (SIDEBAR, cli, root, host)) + ';\n}\n'
                      + f'pane name="Workspace" command={json.dumps(str(cli))} {{\n args ' + ' '.join(json.dumps(arg) for arg in workspace) + ';\n}\n}\n}\n')
            (root / f"host-{host}.kdl").write_text(layout)
            command = [binary, "--config", root / "outer.kdl", "--layout", root / f"host-{host}.kdl", "attach", "-c", f"h2-host-{host}"]
            mux("new-session", "-d", "-s", f"host-{host}", "-x", "180", "-y", "55", "-c", str(root), "exec " + shlex.join(map(str, command)))
            marker = support.wait(lambda: json.loads((root / f"control/attachment-{host}.json").read_text()))
            known[marker["attachment_process"]["pid"]] = marker["attachment_process"]
            support.wait(lambda: "STOCK DEMO TARGET" in screen(host))
            time.sleep(.5)
            registration = native("navigate", "register", "--host", host, "--host-session", f"h2-host-{host}", "--workspace-pane", "1", "--client-pid", str(marker["attachment_process"]["pid"]), "--session", "h2-shared", "--plugin", str(plugin))
            controller.atomic(root / f"registration-{host}.json", registration)
            side(host)
            time.sleep(.5)
        stage = "publish normalized synthetic agent reports"
        for pane in range(5):
            process = next(item for item in inventory["panes"] if item["terminal_id"] == pane)["pane_process"]
            kind = "agy" if pane in (1, 3) else "opencode"
            registered = native("agent", "register", "--session", "h2-shared", "--pane", str(pane), "--pid", str(process["pid"]), "--kind", kind, "--synthetic")
            instances[pane] = registered["agent_instance_id"]
            sources[pane] = {"schema_version": 1, "source_id": f"fixture-{kind}", "source_epoch": 1, "source_revision": 1,
                             "turn_epoch": 1, "turn_revision": 1, "turn_id": f"fixture-turn-{pane}-1", "conversation_title": f"Fixture-{pane}",
                             "conversation_id": f"fixture-conversation-{pane}", "activity": "idle"}
            if pane == 1: sources[pane].update(activity="working", pending_requests=[{"id": "fixture-permission", "kind": "permission"}])
            if pane == 2: sources[pane].update(activity="working")
            if pane == 3: sources[pane].update(execution_error={"turn_id": "fixture-turn-3-1", "turn_epoch": 1, "turn_revision": 1, "message": "fixture terminal failure", "is_terminal": True})
            if pane == 4: sources[pane].update(activity="initializing")
            native("agent", "report", "--instance", instances[pane], data=json.dumps(sources[pane]))
        support.wait(lambda: all(find_agent("a", f"Fixture-{pane}") is not None for pane in range(5)))
        for pane, icon in ((0, "○"), (1, "!"), (3, "×"), (4, "?")):
            support.wait(lambda: (index := find_agent("a", f"Fixture-{pane}")) is not None and icon in rows("a")[index])
        support.wait(lambda: (index := find_agent("a", "Fixture-2")) is not None
                     and any(icon in rows("a")[index] for icon in "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"))
        frame("initial-statuses")
        report["checks"].append("production_three_level_tree_all_statuses_and_synthetic_labels")
        stage = "highlight without acknowledgement"
        report_source(0, activity="working")
        done = report_source(0, activity="idle", successful_completion={"turn_id": "fixture-turn-0-1", "turn_epoch": 1, "turn_revision": 1, "completed_at_ms": time.time_ns() // 1_000_000})
        assert done["completion_revision"] == 1
        support.wait(lambda: has_done("a", "Fixture-0") and has_done("b", "Fixture-0"))
        select_agent("a", "Fixture-0")
        time.sleep(1.2)
        assert host_ack("a", 0) == 0 and host_ack("b", 0) == 0
        report["checks"].append("cursor_highlight_and_sidebar_focus_leave_done_for_both_hosts")
        frame("done-before-visit")
        ack_evidence("before-visits")
        stage = "Enter activates and acknowledges host a"
        mux("send-keys", "-t", "host-a", "Enter")
        support.wait(lambda: host_ack("a", 0) == 1)
        assert host_ack("b", 0) == 0
        mux("send-keys", "-t", "host-a", "-l", "h2-enter-agent-target")
        mux("send-keys", "-t", "host-a", "Enter")
        support.wait(lambda: "h2-enter-agent-target" in (root / "input-h2-shared-0").read_text())
        report["checks"].append("agent_enter_native_keyboard_transfer_clears_done_only_in_this_host")
        ack_evidence("after-enter-a")
        stage = "agent leaf folds parent tab"
        select_agent("a", "Fixture-0")
        mux("send-keys", "-t", "host-a", "Space")
        support.wait(lambda: find_agent("a", "Fixture-0") is None and "Tab #1" in screen("a"))
        assert find_agent("a", "Fixture-3") is not None
        report["checks"].append("agent_leaf_space_folds_parent_tab_without_other_tab")
        stage = "mouse activates and acknowledges host b"
        mux("send-keys", "-t", "host-a", "l")
        support.wait(lambda: find_agent("a", "Fixture-0") is not None)
        side("b")
        row=support.wait(lambda:(value:=find_agent("b","Fixture-0")) is not None and value+1)-1
        mouse("b", row, 20)
        support.wait(lambda:host_ack("b",0)==1)
        report["checks"].append("mouse_agent_activation_acknowledges_other_host_independently")
        frame("after-both-visits")
        ack_evidence("after-both-visits")
        stage = "sidebar restart preserves folds, selection and host acknowledgements"
        side("a");side("b")
        report_source(2,activity="idle",successful_completion={"turn_id":"fixture-turn-2-1","turn_epoch":1,"turn_revision":1,"completed_at_ms":time.time_ns()//1_000_000})
        select_agent("a","Fixture-0");mux("send-keys","-t","host-a","Space")
        support.wait(lambda:find_agent("a","Fixture-2") is None and any("Tab #1" in line and "✓1" in line for line in rows("a")))
        saved_path=root / "state/verij/agent-monitoring/tree/a.json"
        saved=support.wait(lambda:json.loads(saved_path.read_text()))
        assert saved["collapsed_tabs"]
        # The private wrapper keeps the pane/host lifetime. Verify a genuinely
        # new sidebar process before accepting its restored rendering.
        sidebar=json.loads((root / "sidebar-a.json").read_text())
        assert controller.birth(sidebar["process"]["pid"]) == sidebar["process"]
        action("h2-host-a","write-chars","--pane-id","terminal_0","q")
        def restarted():
            current=json.loads((root / "sidebar-a.json").read_text())
            return (current["generation"] > sidebar["generation"]
                    and current["process"] != sidebar["process"]
                    and controller.birth(sidebar["process"]["pid"]) != sidebar["process"]
                    and controller.birth(current["process"]["pid"]) == current["process"]
                    and current)
        current_sidebar=support.wait(restarted)
        support.wait(lambda:ready("a",current_sidebar["process"]))
        support.wait(lambda:find_agent("a","Fixture-2") is None and "Tab #1" in screen("a"))
        restored=json.loads(saved_path.read_text())
        assert restored["selected"]==saved["selected"] and restored["collapsed_tabs"]==saved["collapsed_tabs"]
        assert host_ack("a",0)==1 and host_ack("b",0)==1
        controller.atomic(root / "sidebar-restart.json", {"before":sidebar,"after":current_sidebar,
                          "persisted_before":saved,"persisted_after":restored})
        report["checks"].append("collapsed_summary_and_host_ack_selection_folds_survive_sidebar_restart")
        stage = "native navigation automatically acknowledges host a"
        mux("send-keys","-t","host-a","l")
        support.wait(lambda:find_agent("a","Fixture-2") is not None)
        # Native Zellij navigation is a visit only when the full outer/inner
        # observation confirms this host's actual target.
        native("navigate","focus","--host","a","--pane","2")
        support.wait(lambda:host_ack("a",2)==1)
        assert host_ack("b",2)==0
        ack_evidence("after-native-a")
        report["checks"].append("native_navigation_visit_auto_acknowledges_current_host_only")
        stage = "Needs input survives visit"
        select_agent("a","Fixture-1")
        activation_started=time.time_ns()//1_000_000
        mux("send-keys","-t","host-a","Enter")
        def outer_focus_completed():
            values=[json.loads(line) for line in (root / "control/control-results.ndjson").read_text().splitlines()]
            return next((value for value in reversed(values)
                         if value["status"]=="inner_focus_observed"
                         and value["request"]["operation"]=={"type":"focus","terminal":1}
                         and value["observation"]["session_name"]=="h2-host-a"
                         and value["observation"]["terminal"]==1
                         and value["observation"]["observed_at_ms"]>=activation_started),None)
        controller.atomic(root / "needs-input-focus.json",support.wait(outer_focus_completed))
        support.wait(lambda:"terminal_1" in action("h2-host-a","list-clients"))
        mux("send-keys","-t","host-a","-l","h2-needs-input-agent-target")
        mux("send-keys","-t","host-a","Enter")
        support.wait(lambda:"h2-needs-input-agent-target" in (root / "input-h2-shared-1").read_text())
        permission_row=support.wait(lambda:(index:=find_agent("a","Fixture-1")) is not None and index+1)-1
        assert "!" in rows("a")[permission_row]
        report["checks"].append("visiting_needs_input_does_not_resolve_permission_request")
        stage = "narrow viewport resize"
        side("a")
        # Narrow viewport clips text but leaves default fold/status targets.
        mux("set-option","-t","host-a","window-size","manual")
        mux("resize-window","-t","host-a","-x","90","-y","20")
        time.sleep(.6)
        compact=screen("a")
        assert "opencode" in compact or "agy" in compact
        assert any(icon in compact for icon in ("!","○","✓","×","?"))
        mux("resize-window","-t","host-a","-x","180","-y","55")
        support.wait(lambda:find_agent("a","Fixture-0") is not None)
        report["checks"].append("narrow_resize_preserves_status_and_fold_structure")
        stage = "agent pane closes with valid parent and selection"
        select_agent("a","Fixture-4")
        action("h2-shared","close-pane","--pane-id","terminal_4")
        support.wait(lambda:find_agent("a","Fixture-4") is None)
        assert "stack" in screen("a") and find_agent("a","Fixture-3") is not None
        frame("final-tree")
        report["checks"].append("pane_closure_removes_agent_and_retains_parent_with_valid_selection")
        controller.atomic(root / "agent-evidence.json",{pane:{
            "identity":json.loads((root / f"agents/agents/v1/{instance}/identity.json").read_text()),
            "state":json.loads((root / f"agents/agents/v1/{instance}/state.json").read_text()),
        } for pane,instance in instances.items()})
        for file in (root / "control").glob("focus-*.json"):
            pid=json.loads(file.read_text())["context"]["server_pid"]
            if value:=controller.birth(pid):known[pid]=value
        ui_ready_receipts={host:json.loads((root / f"ui-ready-{host}.json").read_text()) for host in ("a","b")}
        for process in ui_ready_receipts.values():known[process["pid"]]=process
        controller.atomic(root / "review-manifest.json", {"root": str(root), "socket_dir": str(sockets), "tmux_socket": str(sockets / "terminal"),
                          "cli": str(cli), "binary": str(binary), "plugin": str(plugin), "sessions": sessions, "inner_session": "h2-shared",
                           "instances": instances, "sources": sources, "known_births": known,
                           "ui_ready_receipts":ui_ready_receipts})
        report["status"] = "PASS"
    except Exception as error:
        report.update(status="FAIL", stage=stage, error=f"{type(error).__name__}: {error}")
        for host in ("a", "b"):
            try: (root / f"failure-host-{host}.txt").write_text(screen(host))
            except Exception: pass
        private_processes={}
        for file in (root / "control").glob("focus-*.json"):
            try:
                pid=json.loads(file.read_text())["context"]["server_pid"]
                todo=[pid];seen=set()
                while todo:
                    current=todo.pop()
                    if current in seen:continue
                    seen.add(current)
                    stat=Path(f"/proc/{current}/stat").read_text().rsplit(")",1)[1].split()
                    private_processes[current]={"state":stat[0],"parent":int(stat[1]),"name":Path(f"/proc/{current}/comm").read_text().strip()}
                    if private_processes[current]["name"]=="verij":
                        threads={}
                        for task in Path(f"/proc/{current}/task").iterdir():
                            try:threads[task.name]={"name":(task/"comm").read_text().strip(),"wait":(task/"wchan").read_text().strip()}
                            except OSError:pass
                        private_processes[current]["threads"]=threads
                    for task in Path(f"/proc/{current}/task").iterdir():
                        try:todo.extend(map(int,(task / "children").read_text().split()))
                        except OSError:pass
            except OSError:pass
        (root / "private-processes.json").write_text(json.dumps(private_processes,indent=2)+"\n")
        for session in sessions:
            try:
                (root / f"failure-clients-{session}.txt").write_text(action(session,"list-clients"))
                (root / f"failure-panes-{session}.json").write_text(action(session,"list-panes","--all","--json"))
            except Exception:pass
    finally:
        if args.keep_live and report["status"] == "PASS":
            report.update(review_live=True, host_a_attach=f"tmux -S {sockets / 'terminal'} attach -t host-a", host_b_attach=f"tmux -S {sockets / 'terminal'} attach -t host-b")
        else:
            for file in (root / "control").glob("focus-*.json"):
                try:
                    pid = json.loads(file.read_text())["context"]["server_pid"]
                    if value := controller.birth(pid): known[pid] = value
                except Exception: pass
            for session in sessions: subprocess.run([str(binary), "kill-session", session], env=env, cwd=root, capture_output=True, timeout=8)
            subprocess.run([*tmux, "kill-server"], capture_output=True, timeout=8)
            shutil.rmtree(sockets, ignore_errors=True)
            def gone():
                for pid,birth in known.items():
                    current=controller.birth(pid)
                    if current is not None and all(current[key]==birth[key] for key in ("pid","boot_id","start_jiffies")):return False
                return True
            try: support.wait(gone, seconds=10); exited = True
            except TimeoutError: exited = False; report.update(status="FAIL", cleanup_error="known private actors still alive")
            report["cleanup"] = {"private_socket_removed": not sockets.exists(), "known_processes_exited": exited}
        controller.atomic(root / "verification.json", report)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2))
    return 0 if report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
