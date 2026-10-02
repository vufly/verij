#!/usr/bin/env python3
"""Create/verify/review/clean disposable stock-Zellij two-host navigation demo.

No private IPC, dependency build, installed config change or real agent record.
The review sidebar is a clearly labelled control fixture, not the production UI.
"""
import argparse
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
import uuid

import controller


def run(argv, env=None, cwd=None, timeout=8):
    result = subprocess.run(argv, env=env, cwd=cwd, capture_output=True, text=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError((argv[0], result.returncode, result.stdout[-150:], result.stderr[-150:]))
    return result.stdout.strip()


def wait(predicate, seconds=25):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        try:
            value = predicate()
            if value:
                return value
        except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired):
            pass
        time.sleep(.1)
    raise TimeoutError("stock demo condition deadline")


def mux(root, *parts):
    manifest = controller.config(root)
    return run(["tmux", "-f", "/dev/null", "-S", manifest["tmux_socket"], *parts],
               controller.environment(root, manifest), root)


def focus_map(root, session):
    return {int(row.split()[0]): row.split()[1] for row in controller.action(root, session, "list-clients").splitlines()[1:] if row.strip()}


def snapshots(root, session=None):
    items = [json.loads(path.read_text()) for path in (root / "stock-state").glob("snapshot-*.json")]
    return [item for item in items if item.get("ready") and (session is None or item.get("session") == session)
            and time.time_ns() - int(item["time_ns"]) < 5_000_000_000]


def rebind(root, host):
    manifest = controller.config(root)
    entry = manifest["hosts"][host]
    marker = root / "stock-state" / f"binding-{entry['nonce']}.json"
    before = marker.read_text() if marker.exists() else None
    controller.action(root, entry["session"], "write", "--pane-id", f"terminal_{entry['workspace_pane']}", "2")  # Ctrl+b on this display's stdin.
    wait(lambda: any(item.get("focused_pane") == f"plugin_{item['plugin_id']}" for item in snapshots(root)))
    controller.action(root, entry["session"], "write-chars", "--pane-id", f"terminal_{entry['workspace_pane']}", entry["nonce"] + "\r")
    wait(lambda: marker.exists() and marker.read_text() != before)
    value = controller.register(root, host)
    entry["inner_session"] = value["observation"]["session"]
    controller.atomic(root / "manifest.json", manifest)
    return value


def receiver(root, name):
    return [sys.executable, str(Path(__file__).with_name("receiver.py").resolve()), str(root), name]


def start(binary, plugin, scratch):
    root = Path(tempfile.mkdtemp(prefix="vj-stock-", dir=scratch)).resolve()
    os.chmod(root, 0o700)
    sockets = Path(tempfile.mkdtemp(prefix="vj-stock-sock-")).resolve()
    state = root / "stock-state"
    state.mkdir(mode=0o700)
    manifest = {"schema_version": 1, "root": str(root), "socket_dir": str(sockets), "tmux_socket": str(sockets / "terminal"),
                "binary": str(binary), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                "stock_version": run([str(binary), "--version"]),
                "plugin": str(plugin), "hosts": {}, "session_names": ["review-host-a", "review-host-b", "shared", "other"]}
    controller.atomic(root / "manifest.json", manifest)
    env = controller.environment(root, manifest)
    # Cache grants apply ONLY to this generated plugin in the private XDG cache.
    cache = root / "cache" / "zellij"
    cache.mkdir(parents=True)
    url = "file:" + str(plugin)
    permissions = "\n".join(json.dumps(name) + ' {\n ReadApplicationState\n ChangeApplicationState\n ReadCliPipes\n}\n' for name in (url, str(plugin)))
    (cache / "permissions.kdl").write_text(permissions)
    (cache / "permissions.kdl").chmod(0o600)
    base = 'default_shell "/bin/sh"\nshow_startup_tips false\nshow_release_notes false\nmirror_session false\nsession_serialization false\n'
    (root / "inner-base.kdl").write_text(base)
    (root / "outer.kdl").write_text(base + 'default_mode "locked"\nnested_session_handling "descend"\n'
                                   + 'keybinds {\n locked {\n bind "Alt v" { MoveFocus "left"; }\n'
                                   + 'bind "F11" { MoveFocus "left"; }\n bind "F12" { MoveFocus "right"; }\n }\n}\n')
    (root / "inner-layout.kdl").write_text('layout {\n pane\n}\n')
    for host in ("a", "b"):
        nonce = uuid.uuid4().hex
        client_config = root / f"inner-{host}.kdl"
        client_config.write_text(base + f'keybinds {{\n normal {{\n bind "Ctrl b" {{\n LaunchOrFocusPlugin {json.dumps(url)} {{\n floating true\n move_to_focused_tab true\n }}\n }}\n }}\n}}\n')
        wrapper = [sys.executable, str(Path(__file__).with_name("workspace.py").resolve()), str(root), host]
        sidebar = [sys.executable, str(Path(__file__).with_name("sidebar.py").resolve()), str(root), host]
        (root / f"outer-{host}.kdl").write_text('layout {\n pane split_direction="vertical" {\n'
            + f'pane size="32%" name="Stock demo controls" command={json.dumps(sidebar[0])} {{\n args ' + ' '.join(json.dumps(arg) for arg in sidebar[1:]) + ';\n}\n'
            + f'pane name="Workspace" command={json.dumps(wrapper[0])} {{\n args ' + ' '.join(json.dumps(arg) for arg in wrapper[1:]) + ';\n}\n}\n}\n')
        manifest["hosts"][host] = {"nonce": nonce, "session": f"review-host-{host}", "sidebar_pane": 0, "workspace_pane": 1,
                                    "inner_session": "shared", "client_config": str(client_config)}
    controller.atomic(root / "manifest.json", manifest)
    try:
        for host in ("a", "b"):
            manifest = controller.config(root)
            mux(root, "new-session", "-d", "-s", f"host-{host}", "-x", "180", "-y", "55", "-c", str(root), "exec " + shlex.join([
                str(binary), "--config", str(root / "outer.kdl"), "--layout", str(root / f"outer-{host}.kdl"), "attach", "-c", f"review-host-{host}"]))
            marker = wait(lambda: json.loads((root / f"{host}-workspace.json").read_text()))
            entry = manifest["hosts"][host]
            entry.update(client_pid=marker["client_pid"], client_birth=controller.birth(marker["client_pid"]),
                         workspace_pane=int(marker["host_pane"]), host_wrapper=marker)
            assert entry["client_birth"]
            controller.atomic(root / "manifest.json", manifest)
            # Avoid stock control connections/keyboard during initial session
            # bootstrap; the actual receiver frame proves the guest rendered.
            wait(lambda: "STOCK DEMO TARGET shared-0" in mux(root, "capture-pane", "-p", "-t", f"host-{host}"))
            value = rebind(root, host)
            assert value["observation"]["session"] == "shared"
            mux(root, "send-keys", "-t", f"host-{host}", "F12")
            wait(lambda: list(focus_map(root, entry["session"]).values()) == [f"terminal_{entry['workspace_pane']}"])
        a, b = controller.binding(root, "a"), controller.binding(root, "b")
        assert a["client_id"] != b["client_id"], "keybind registration did not isolate displays"
        for pane in (1,):
            target = controller.action(root, "shared", "new-pane", "--", *receiver(root, f"shared-{pane}"))
            assert target == f"terminal_{pane}", target
        floating = controller.action(root, "shared", "new-pane", "--floating", "--", *receiver(root, "shared-2"))
        assert floating == "terminal_2", floating
        controller.action(root, "shared", "new-tab", "--name", "stack review", "--", *receiver(root, "shared-3"))
        fourth = controller.action(root, "shared", "new-pane", "--", *receiver(root, "shared-4"))
        assert fourth == "terminal_4", fourth
        controller.action(root, "shared", "stack-panes", "--", "terminal_3", "terminal_4")
        # A second live inner session, with a separate control fixture client.
        other_config = root / "other.kdl"
        other_config.write_text((root / "inner-a.kdl").read_text())
        mux(root, "new-session", "-d", "-s", "other-seed", "-x", "140", "-y", "45", "-c", str(root), "exec " + shlex.join([
            str(binary), "--config", str(other_config), "--layout", str(root / "inner-layout.kdl"), "attach", "-c", "other", "--", *receiver(root, "other-0")]))
        wait(lambda: "STOCK DEMO TARGET other-0" in mux(root, "capture-pane", "-p", "-t", "other-seed"))
        target = controller.action(root, "other", "new-pane", "--", *receiver(root, "other-1"))
        assert target == "terminal_1", target
        for host, pane in (("a", 0), ("b", 1)):
            result = controller.activate(root, host, pane)
            assert result["status"] == "inner_focus_observed", result
        return root
    except Exception as error:
        try:
            screens = {}
            for host in ("a", "b"):
                try:
                    screens[host] = mux(root, "capture-pane", "-p", "-t", f"host-{host}")
                except Exception:
                    pass
            controller.atomic(root / "setup-screens.json", screens)
        except Exception:
            pass
        controller.atomic(root / "setup-failure.json", {"status": "FAIL", "error": str(error), "root": str(root)})
        cleanup(root)
        raise RuntimeError(f"setup failed; retained diagnostics at {root}: {error}") from error


def keyboard(root, host, token, input_name):
    mux(root, "send-keys", "-t", f"host-{host}", "-l", token)
    mux(root, "send-keys", "-t", f"host-{host}", "Enter")
    wait(lambda: token in (root / f"input-{input_name}").read_text())


def verify(root):
    report = {"status": "PASS", "root": str(root), "stock_binary": controller.config(root)["binary"],
              "checks": [], "limitations": [], "acknowledge_done": False}
    try:
        report["stage"] = "registration and tiled keyboards"
        report["stock_version"] = controller.config(root)["stock_version"]
        a, b = controller.binding(root, "a"), controller.binding(root, "b")
        report["checks"].append({"focused_plugin_keyboard_registration_distinct_clients": True, "a": a, "b": b,
                                  "epochs_are_verij_plugin_loads_not_stock_socket_nonces": True})
        manifest = controller.config(root)
        nonce = manifest["hosts"]["a"]["nonce"]
        original_binding = (root / "stock-state" / f"binding-{nonce}.json").read_text()
        controller.action(root, "shared", "pipe", "--name", "verij_stock_bind", "--", nonce)
        assert (root / "stock-state" / f"binding-{nonce}.json").read_text() == original_binding
        report["checks"].append({"CLI_pipe_cannot_register_host_context": True})
        for host, pane, name in (("a", 0, "shared-0"), ("b", 1, "shared-1")):
            result = controller.activate(root, host, pane)
            assert result["status"] == "inner_focus_observed" and result["demo_outer_focus_observed"], result
            keyboard(root, host, f"verified-{host}-{pane}", name)
        assert controller.host_focus(root, "a", workspace=False)
        report["checks"].append({"explicit_stock_sidebar_focus_observed": True})
        report["limitations"].append("Earlier F11 return was intercepted in Descend mode; use host-pane mouse focus or explicit sidebar helper.")
        report["checks"].append({"two_actual_hosts_addressed_tiled_focus_and_keyboard": True})
        # Local highlighting changes only outer focus; inner remembered focus is not a visit.
        assert controller.host_focus(root, "a", workspace=False)
        inner = controller.request(root, "a", 0, "query")
        assert inner["observation"]["focused_pane"] == "terminal_0" and not inner["acknowledge_done"]
        report["checks"].append({"sidebar_focus_does_not_promote_remembered_inner_focus_to_visit": True, "query": inner})
        frozen = (root / "a-sequence.json").read_bytes()
        controller.request(root, "a", 0, "query")
        assert (root / "a-sequence.json").read_bytes() == frozen
        report["checks"].append({"passive_query_keeps_local_journal_unchanged": True})
        report["stage"] = "native stack keyboard"
        sequence_before = json.loads((root / "a-sequence.json").read_text())["sequence"]
        mux(root, "send-keys", "-t", "host-a", "j")
        time.sleep(.2)
        assert json.loads((root / "a-sequence.json").read_text())["sequence"] == sequence_before
        assert controller.request(root, "a", 0, "query")["observation"]["focused_pane"] == "terminal_0"
        mux(root, "send-keys", "-t", "host-a", "Enter")
        wait(lambda: json.loads((root / "a-sequence.json").read_text())["sequence"] > sequence_before)
        wait(lambda: list(focus_map(root, "review-host-a").values()) == ["terminal_1"])
        keyboard(root, "a", "sidebar-enter-stock", "shared-1")
        report["checks"].append({"actual_sidebar_selection_only_then_enter_activation": True})
        for host, pane in (("a", 2), ("a", 0)):
            result = controller.activate(root, host, pane)
            assert result["status"] == "inner_focus_observed", result
            keyboard(root, host, f"layer-{host}-{pane}", f"shared-{pane}")
        report["checks"].append({"hidden_float_and_return_tiled_keyboard": True})
        # Real guest keybinding toggles fullscreen; exact other-pane focus
        # must leave it and still deliver normal outer keyboard input.
        mux(root, "send-keys", "-t", "host-a", "C-p", "f")
        wait(lambda: any(p["id"] == 0 and not p["is_plugin"] and p["is_fullscreen"] for p in json.loads(controller.action(root, "shared", "list-panes", "--all", "--json"))))
        full = controller.activate(root, "a", 1)
        assert full["status"] == "inner_focus_observed", full
        keyboard(root, "a", "fullscreen-other-stock", "shared-1")
        report["checks"].append({"fullscreen_other_target_revealed_and_keyboard": True})
        wait(lambda: any(p["id"] == 3 and not p["is_plugin"] for p in json.loads(controller.action(root, "shared", "list-panes", "--all", "--json"))))
        controller.activate(root, "b", 1)
        controller.activate(root, "a", 0)
        before_peer = controller.request(root, "b", 1, "query")["observation"]["focused_pane"]
        cross = controller.activate(root, "a", 3)
        assert cross["status"] == "inner_focus_observed", cross
        keyboard(root, "a", "background-tab-stock", "shared-3")
        assert controller.request(root, "b", 1, "query")["observation"]["focused_pane"] == before_peer
        report["checks"].append({"background_tab_target_client_and_keyboard": True, "peer_tiled_focus_unchanged": True})
        controller.activate(root, "a", 0)
        # Observe explicit API failure without acknowledging anything.
        missing = controller.request(root, "a", 2**32 - 1)
        assert missing["status"] == "pane_missing" and not missing["acknowledge_done"], missing
        duplicate = controller.request(root, "a", 1, override={"sequence": missing["request"]["sequence"]})
        assert duplicate["status"] == "superseded", duplicate
        stale = controller.request(root, "a", 1, override={"epoch": "stale-verij-epoch"})
        assert stale["status"] == "stale_plugin_epoch", stale
        report["checks"].append({"missing_target_duplicate_and_stale_plugin_epoch_fail_closed": True,
                                  "results": [missing, duplicate, stale]})
        before_b = controller.request(root, "b", 1, "query")["observation"]["focused_pane"]
        report["stage"] = "cross-session switch and rebind"
        switching = controller.request(root, "a", 1, "switch", "other")
        assert switching["status"] == "switch_dispatched_unverified", switching
        wait(lambda: "STOCK DEMO TARGET other-1" in mux(root, "capture-pane", "-p", "-t", "host-a"))
        value = wait(lambda: (v := rebind(root, "a"))["observation"].get("session") == "other" and v)
        assert value["observation"]["server_pid"] != a["server_pid"]
        target = controller.activate(root, "a", 1)
        assert target["status"] == "inner_focus_observed", target
        keyboard(root, "a", "cross-session-a", "other-1")
        assert controller.request(root, "b", 1, "query")["observation"]["focused_pane"] == before_b
        controller.activate(root, "b", 1)
        keyboard(root, "b", "peer-remained-shared", "shared-1")
        controller.request(root, "a", 0, "switch", "shared")
        wait(lambda: "STOCK DEMO TARGET shared-0" in mux(root, "capture-pane", "-p", "-t", "host-a"))
        wait(lambda: (v := rebind(root, "a"))["observation"].get("session") == "shared" and v)
        controller.activate(root, "a", 0)
        keyboard(root, "a", "returned-shared-a", "shared-0")
        report["checks"].append({"stock_plugin_cross_session_rebind_and_peer_keyboard": True,
                                  "dispatch_itself_not_completion": True})
        for pane in (3, 4):
            result = controller.activate(root, "a", pane)
            assert result["status"] == "inner_focus_observed", result
            keyboard(root, "a", f"stack-a-{pane}", f"shared-{pane}")
        report["stage"] = "sidebar mouse activation"
        sequence_before = json.loads((root / "a-sequence.json").read_text())["sequence"]
        pane = next(p for p in json.loads(controller.action(root, "review-host-a", "list-panes", "--all", "--json"))
                    if not p["is_plugin"] and p["id"] == 0)
        for end in ("M", "m"):
            wire = f"\x1b[<0;{pane['pane_content_x'] + 3};{pane['pane_content_y'] + 18}{end}".encode()
            mux(root, "send-keys", "-t", "host-a", "-H", *[f"{byte:02x}" for byte in wire])
            time.sleep(.1)
        wait(lambda: list(focus_map(root, "review-host-a").values()) == ["terminal_0"])
        assert json.loads((root / "a-sequence.json").read_text())["sequence"] == sequence_before
        report["checks"].append({"native_mouse_returns_to_sidebar_without_activation": True})
        x, y = pane["pane_content_x"] + 3, pane["pane_content_y"] + 5
        for end in ("M", "m"):
            wire = f"\x1b[<0;{x};{y}{end}".encode()
            mux(root, "send-keys", "-t", "host-a", "-H", *[f"{byte:02x}" for byte in wire])
            time.sleep(.1)
        wait(lambda: json.loads((root / "a-sequence.json").read_text())["sequence"] > sequence_before)
        wait(lambda: list(focus_map(root, "review-host-a").values()) == ["terminal_1"])
        keyboard(root, "a", "sidebar-click-stock", "shared-0")
        report["checks"].append({"actual_outer_mouse_click_fixture_row_activates_workspace": True})
        report["checks"].append({"native_stack_reveal_and_keyboard_without_unstacking": True,
                                  "shared_stack_side_effects_accepted": True})
        report["limitations"] += ["No production Done/acknowledgement logic. Every control result keeps acknowledge_done=false.",
                                   "Local plugin epoch/sequence filtering cannot cancel an action already queued in stock Zellij.",
                                   "Explicit focused-plugin keyboard registration is required; no client binding inferred from server environment or cwd.",
                                   "Outer confirmation covers this single-display private host fixture; plugin observation is point-in-time, not whole-host guarantee."]
        report.pop("stage", None)
    except Exception as error:
        report.update(status="FAIL", error=str(error))
    controller.atomic(root / "verification.json", report)
    return report


def cleanup(root):
    manifest = controller.config(root)
    assert root.name.startswith("vj-stock-") and str(root) == manifest["root"]
    known = {value["server_pid"] for value in snapshots(root)}
    known.update(entry["client_pid"] for entry in manifest["hosts"].values() if "client_pid" in entry)
    births = {pid: controller.birth(pid) for pid in known if controller.birth(pid)}
    for session in manifest["session_names"]:
        subprocess.run([manifest["binary"], "kill-session", session], env=controller.environment(root, manifest), cwd=root,
                       capture_output=True, timeout=8)
    subprocess.run(["tmux", "-f", "/dev/null", "-S", manifest["tmux_socket"], "kill-server"], capture_output=True, timeout=8)
    shutil.rmtree(manifest["socket_dir"], ignore_errors=True)
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline and any(controller.birth(pid) == value for pid, value in births.items()):
        time.sleep(.1)
    gone = all(controller.birth(pid) != value for pid, value in births.items())
    controller.atomic(root / "cleanup.json", {"private_cleanup_requested": True, "known_processes_exited": gone,
                                               "births": births, "time_ns": time.time_ns()})
    if not gone:
        raise RuntimeError("private demo processes did not exit")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("verify", "start", "clean", "inspect"))
    parser.add_argument("--binary", type=Path, default=Path(shutil.which("zellij") or "zellij"))
    parser.add_argument("--plugin", type=Path)
    parser.add_argument("--scratch-dir", type=Path, default=Path(__file__).parent / ".scratch")
    parser.add_argument("--root", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.command in ("verify", "start"):
        if not args.plugin:
            parser.error("--plugin required")
        binary = args.binary.resolve(strict=True)
        if "workspaces/verij-agent-monitoring/zellij/" in str(binary):
            parser.error("archived patched dependency binary is not allowed")
        args.scratch_dir.mkdir(parents=True, exist_ok=True)
        root = start(binary, args.plugin.resolve(strict=True), args.scratch_dir)
        if args.command == "verify":
            try:
                result = verify(root)
            finally:
                cleanup(root)
        else:
            for host in ("a", "b"):
                assert controller.host_focus(root, host, workspace=False)
            result = {"root": str(root), "review_instructions": str(root / "REVIEW.md"),
                      "host_a_attach": f"tmux -S {controller.config(root)['tmux_socket']} attach -t host-a",
                      "host_b_attach": f"tmux -S {controller.config(root)['tmux_socket']} attach -t host-b",
                      "cleanup": shlex.join([sys.executable, str(Path(__file__).resolve()), "clean", "--root", str(root)])}
            (root / "REVIEW.md").write_text("# Disposable stock navigation demo\n\nThis is a control fixture, not production monitoring. No real Done state exists.\n\n"
                + "\n".join(f"- {name}: `{value}`" for name, value in result.items()) + "\n\n"
                + "## Five-minute check\n\n"
                + "1. Open host-a and host-b in separate terminals with the printed tmux commands.\n"
                + "2. Left controls: j/k or arrows move selection only. Enter activates; then type a unique word and Enter in Workspace.\n"
                + "3. Click an empty left area to return; click a row to activate. q is passive; it never acknowledges Done.\n"
                + "4. Try tiled rows0/1, floating row2 and stack rows3/4. Compare both hosts; native shared stack expansion is accepted.\n"
                + "5. Report approve/rework with host, target and keyboard failure. No real Done badge is implemented yet.\n\n"
                + "F11/Alt shortcuts can be intercepted by Descend; use mouse or the helper below.\n\n"
                + "```bash\n" + shlex.join([sys.executable, str(Path(__file__).with_name("controller.py").resolve()),
                    "--root", str(root), "--host", "a", "--operation", "sidebar"]) + "\n```\n\n"
                + "Clean only this demo with the printed cleanup command after review.\n")
    else:
        if not args.root:
            parser.error("--root required")
        root = args.root.resolve(strict=True)
        if args.command == "clean":
            cleanup(root)
            result = {"cleaned": str(root)}
        else:
            result = {"root": str(root), "manifest": controller.config(root), "snapshots": snapshots(root)}
    text = json.dumps(result, indent=2) + "\n"
    if args.output:
        args.output.write_text(text)
    print(text)
    return 0 if result.get("status", "PASS") == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
