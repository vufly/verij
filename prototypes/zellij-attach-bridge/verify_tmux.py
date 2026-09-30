#!/usr/bin/env python3
"""Test nested keyboard routing using a dedicated real tmux terminal server.

The tmux socket, all Zellij sockets, and every session are disposable. No active
user terminal is selected, and no host-pane `write-chars` injection is used.
"""
import argparse
from contextlib import nullcontext
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

from verify import completion_control, control


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--scratch-dir", type=Path, help="Parent for disposable probe files; defaults to durable prototype .scratch directory")
    parser.add_argument("--completion", action="store_true", help="Use correlated screen results and verify passive focus queries")
    args = parser.parse_args()
    binary = str(args.binary.resolve(strict=True))
    report = {"binary": binary, "input_transport": "tmux send-keys through outer display terminal", "checks": []}
    scratch_parent = args.scratch_dir or Path(__file__).resolve().parent / ".scratch"
    scratch_parent.mkdir(parents=True, exist_ok=True)
    # Keep only short-lived sockets in /tmp for Unix pathname-length limits.
    with tempfile.TemporaryDirectory(prefix="vj-sock-") as sockets, nullcontext(tempfile.mkdtemp(prefix="vj-mux-", dir=scratch_parent)) as scratch:
        root = Path(scratch)
        report["artifacts_dir"] = str(root)
        env = {k: v for k, v in os.environ.items() if not k.startswith(("ZELLIJ", "VERIJ_ZELLIJ", "TMUX"))}
        for kind in ("CONFIG", "DATA", "STATE", "CACHE"):
            env[f"XDG_{kind}_HOME"] = str(root / kind.lower())
        env.update(TERM="xterm-256color", ZELLIJ_SOCKET_DIR=sockets)
        config = root / "inner.kdl"
        config.write_text('default_shell "/bin/sh"\nshow_startup_tips false\nshow_release_notes false\nmirror_session false\nsession_serialization false\n')
        outer_config = root / "outer.kdl"
        outer_config.write_text(config.read_text() + 'default_mode "locked"\nnested_session_handling "descend"\n')
        layout = root / "layout.kdl"
        layout.write_text("layout {\n pane\n}\n")
        receiver = root / "receiver.py"
        receiver.write_text("import sys\nfor line in sys.stdin:\n with open(sys.argv[1],'a') as f: f.write(line); f.flush()\n")
        wrapper = root / "wrapper.py"
        wrapper.write_text(
            "import os,sys,json\nfrom pathlib import Path\n"
            "d=Path(sys.argv[1])\n"
            "(d/'host.json').write_text(json.dumps({'host_session':os.environ['ZELLIJ_SESSION_NAME'],'host_pane':os.environ['ZELLIJ_PANE_ID'],'client_pid':os.getpid()}))\n"
            "e={k:v for k,v in os.environ.items() if not k.startswith('ZELLIJ')}\n"
            "e['ZELLIJ_SOCKET_DIR']=sys.argv[2]\ne['VERIJ_ZELLIJ_IDENTITY_DIR']=str(d)\n"
            "os.execvpe(sys.argv[3],sys.argv[3:],e)\n"
        )
        mux = ["tmux", "-S", str(Path(sockets) / "mux.sock"), "-f", "/dev/null"]
        deadline = time.monotonic() + 100
        def run(command):
            result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=8)
            if result.returncode or result.stdout.startswith(("Session '", "Please specify")):
                raise RuntimeError((command[0], result.returncode, result.stdout[:150], result.stderr[:150]))
            return result.stdout.strip()
        def action(session, *parts):
            return run([binary, "--config", str(config), "-s", session, "action", *parts])
        def wait(predicate):
            end = min(deadline, time.monotonic() + 20)
            while time.monotonic() < end:
                try:
                    value = predicate()
                    if value:
                        return value
                except (FileNotFoundError, RuntimeError, ValueError):
                    pass
                time.sleep(.1)
            raise TimeoutError("tmux-backed nested probe condition timed out")
        hosts = []
        def start(label, create=False, existing=None):
            directory = root / label
            directory.mkdir(mode=0o700)
            if existing is None:
                host_session = label
                pane = run([*mux, "new-session", "-d", "-s", label, "-x", "160", "-y", "50", "-P", "-F", "#{pane_id}", "--", binary, "--config", str(outer_config), "--layout", str(layout), "attach", "-c", label])
                hosts.append(host_session)
                wait(lambda: "terminal_0" in action(host_session, "list-clients"))
                old_pane = "terminal_0"
            else:
                host_session, pane = existing["host"]["host_session"], existing["mux_pane"]
                old_pane = "terminal_" + existing["host"]["host_pane"]
            command = [binary, "--config", str(config)]
            if create:
                command += ["--layout", str(layout)]
            command += ["attach", "-c", "inner"] if create else ["attach", "inner"]
            if create:
                command += ["--", "python3", str(receiver), str(root / "input0")]
            action(host_session, "new-pane", "--in-place", "--pane-id", old_pane, "--close-replaced-pane", "--", "python3", str(wrapper), str(directory), env["ZELLIJ_SOCKET_DIR"], *command)
            host = wait(lambda: json.loads((directory / "host.json").read_text()))
            def identity():
                for file in directory.glob("*.json"):
                    value = json.loads(file.read_text())
                    if value.get("attached") and value["client_pid"] == host["client_pid"]:
                        return value, file
            record, path = wait(identity)
            assert "terminal_" + host["host_pane"] in action(host_session, "list-clients")
            print(f"bound {host_session}/{host['host_pane']} -> inner client {record['client_id']}", flush=True)
            return {"record": record, "record_path": path, "host": host, "mux_pane": pane}
        socket_path = Path(sockets) / "contract_version_1" / "inner"
        sequences = {}
        def completed(child, pane, query_only=False):
            record = child["record"]
            generation = record["connection_id"]
            sequence = 0 if query_only else sequences.get(generation, 0) + 1
            if not query_only:
                sequences[generation] = sequence
            result = completion_control(socket_path, record, pane, sequence, query_only)
            with (root / "completion-results.ndjson").open("a") as fixture:
                fixture.write(json.dumps(result) + "\n")
            return result
        def focus_map():
            return {int(line.split()[0]): line.split()[1] for line in action("inner", "list-clients").splitlines()[1:] if line.strip()}
        def target(child, pane):
            if args.completion:
                result = completed(child, pane)
                assert result["status"] == "focused" and result["focused_pane_id"] == pane and not result["focused_is_plugin"], result
            else:
                assert control(socket_path, child["record"], pane) == ["verij:accepted"]
            wait(lambda: focus_map().get(child["record"]["client_id"]) == f"terminal_{pane}")
        def send(child, token, pane):
            run([*mux, "send-keys", "-t", child["mux_pane"], "-l", token])
            run([*mux, "send-keys", "-t", child["mux_pane"], "Enter"])
            wait(lambda: token in (root / f"input{pane}").read_text())
            print(f"outer terminal input: {token} -> inner pane {pane}", flush=True)
        try:
            a, b = start("host-a", True), start("host-b")
            assert a["record"]["client_id"] != b["record"]["client_id"]
            assert a["record"]["server_pid"] == b["record"]["server_pid"]
            action("inner", "new-pane", "--", "python3", str(receiver), str(root / "input1"))
            target(a, 0)
            target(b, 1)
            send(a, "outer-a-zero", 0)
            send(b, "outer-b-one", 1)
            target(a, 1)
            target(b, 0)
            send(a, "outer-a-one", 1)
            send(b, "outer-b-zero", 0)
            report["checks"].append({"real_host_identity_and_independent_keyboard_focus": True})
            if args.completion:
                before = focus_map()
                current = completed(a, 1, query_only=True)
                other = completed(a, 0, query_only=True)
                assert current["status"] == "focused" and current["tab_id"] == 0, current
                assert other["status"] == "not_focused" and other["focused_pane_id"] == 1, other
                missing = completed(a, 2**32 - 1)
                assert missing["status"] == "pane_missing" and missing["focused_pane_id"] == 1, missing
                assert focus_map() == before
                send(a, "outer-completion-missing-stayed", 1)
                report["checks"].append({"nested_correlated_focus_completion": True,
                                         "nested_passive_query_does_not_focus": True, "nested_missing_target_failure": True})
            floating = action("inner", "new-pane", "--floating", "--", "python3", str(receiver), str(root / "input2"))
            assert floating == "terminal_2", floating
            target(a, 2)
            send(a, "outer-float-visible", 2)
            run([*mux, "send-keys", "-t", a["mux_pane"], "M-f"])
            wait(lambda: focus_map().get(a["record"]["client_id"]) != "terminal_2")
            target(a, 2)
            send(a, "outer-float-revealed", 2)
            target(a, 0)
            run([*mux, "send-keys", "-t", a["mux_pane"], "C-p", "f"])
            wait(lambda: any(p["id"] == 0 and not p["is_plugin"] and p["is_fullscreen"] for p in json.loads(action("inner", "list-panes", "--all", "--json"))))
            target(a, 1)
            send(a, "outer-fullscreen-target", 1)
            action("inner", "stack-panes", "--", "terminal_0", "terminal_1")
            target(a, 0)
            send(a, "outer-stack-zero", 0)
            target(a, 1)
            send(a, "outer-stack-one", 1)
            report["checks"].append({"nested_float_fullscreen_stack_keyboard": True})
            stale = dict(a["record"])
            run([*mux, "send-keys", "-t", a["mux_pane"], "C-o", "d"])
            wait(lambda: not json.loads(a["record_path"].read_text())["attached"])
            c = start("host-a-new", existing=a)
            assert c["record"]["client_id"] == stale["client_id"]
            assert c["record"]["connection_id"] != stale["connection_id"]
            target(c, 0)
            before = focus_map()
            assert control(socket_path, stale, 1) == ["verij:rejected"]
            if args.completion:
                stale_result = completion_control(socket_path, stale, 1, 1)
                assert stale_result["status"] == "stale_attachment", stale_result
            assert focus_map() == before
            target(c, 1)
            send(c, "outer-reconnected", 1)
            report["checks"].append({"nested_reconnect_stale_token_rejected": True})
            report["status"] = "PASS"
        except Exception as error:
            report.update(status="FAIL", error=str(error))
            try:
                report["failure_focus"] = focus_map()
                report["received"] = {p.name: p.read_text() for p in root.glob("input*")}
            except Exception:
                pass
        finally:
            for session in ["inner", *hosts]:
                subprocess.run([binary, "kill-session", session], env=env, capture_output=True, timeout=8)
            subprocess.run([*mux, "kill-server"], env=env, capture_output=True, timeout=8)
            report["private_tmux_and_zellij_cleanup_requested"] = True
    text = json.dumps(report, indent=2)
    (root / "results.json").write_text(text + "\n")
    print(text)
    if args.output:
        args.output.write_text(text + "\n")
    return 0 if report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
