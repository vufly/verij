#!/usr/bin/env python3
"""Verify a patched Zellij binary using disposable sockets, config, and clients.

No installed binary or existing session is selected. The raw protobuf control
helper is deliberately prototype-only, not a supported Verij CLI command.
"""
import argparse
from contextlib import nullcontext
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import socket
import struct
import subprocess
import tempfile
import termios
import time


def process_identity(pid):
    """Linux liveness qualified by boot and process birth, including zombies."""
    try:
        stat = Path(f"/proc/{pid}/stat").read_text()
        # comm can contain spaces and parentheses; fields after its final ')'
        # start at field 3 (state), making field 22 (starttime) offset 19.
        parts = stat[stat.rindex(")") + 2:].split()
        if parts[0] in ("Z", "X"):
            return None
        return {"pid": pid, "start_jiffies": int(parts[19]),
                "boot_id": Path("/proc/sys/kernel/random/boot_id").read_text().strip()}
    except FileNotFoundError:
        return None


def varint(number):
    result = bytearray()
    while number >= 128:
        result.append((number & 127) | 128)
        number >>= 7
    result.append(number)
    return bytes(result)


def integer(field, number):
    return varint(field << 3) + varint(number)


def blob(field, value):
    return varint((field << 3) | 2) + varint(len(value)) + value


def fields(data):
    def read_varint(offset):
        result, shift = 0, 0
        while offset < len(data) and shift < 70:
            byte = data[offset]
            offset += 1
            result |= (byte & 127) << shift
            if byte < 128:
                return result, offset
            shift += 7
        raise ValueError("invalid protobuf varint")

    result, offset = [], 0
    while offset < len(data):
        tag, offset = read_varint(offset)
        if tag & 7 == 0:
            value, offset = read_varint(offset)
        elif tag & 7 == 2:
            length, offset = read_varint(offset)
            value, offset = data[offset:offset + length], offset + length
        else:
            raise ValueError("unexpected wire type")
        result.append((tag >> 3, value))
    return result


def control(socket_path, record, pane_id):
    payload = integer(1, record["client_id"]) + blob(2, record["connection_id"].encode()) + integer(3, pane_id)
    wire = blob(29, payload)
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(5)
        connection.connect(str(socket_path))
        connection.sendall(struct.pack("<I", len(wire)) + wire)
        def exact(length):
            data = bytearray()
            while len(data) < length:
                chunk = connection.recv(length - len(data))
                if not chunk:
                    raise RuntimeError("control socket closed without response")
                data.extend(chunk)
            return bytes(data)
        while True:
            length = struct.unpack("<I", exact(4))[0]
            if length > 1024 * 1024:
                raise ValueError("control response too large")
            for field, value in fields(exact(length)):
                if field == 5:  # ServerToClientMsg.Log
                    return [line.decode() for number, line in fields(value) if number == 1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--scratch-dir", type=Path, help="Parent for disposable probe files; defaults to durable prototype .scratch directory")
    parser.add_argument("--nested", action="store_true", help="Run inner attachments in real disposable host Zellij panes")
    parser.add_argument("--host-injected-input", action="store_true", help="Diagnostic: write to the actual host pane instead of simulating outer keyboard input")
    parser.add_argument("--placements", action="store_true", help="Also exercise floating, fullscreen and ordinary stack targets")
    parser.add_argument("--lifecycle", action="store_true", help="Also exercise cross-tab focus, rename, missing targets, abrupt death and zero-client reattachment")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    report = {"checks": [], "binary": str(binary), "limitations": []}
    clients = []
    host_sessions = []
    scratch_parent = args.scratch_dir or Path(__file__).resolve().parent / ".scratch"
    scratch_parent.mkdir(parents=True, exist_ok=True)
    # Unix sockets have a short pathname limit. Only disposable socket files
    # use /tmp; source, build, fixtures and saved outputs stay in the workspace.
    with tempfile.TemporaryDirectory(prefix="vj-sock-") as sockets, nullcontext(tempfile.mkdtemp(prefix="vj-bridge-", dir=scratch_parent)) as scratch:
        root = Path(scratch)
        report["artifacts_dir"] = str(root)
        env = {key: value for key, value in os.environ.items() if not key.startswith(("ZELLIJ", "VERIJ_ZELLIJ"))}
        for kind in ("CONFIG", "CACHE", "DATA", "STATE"):
            env[f"XDG_{kind}_HOME"] = str(root / kind.lower())
        env["TERM"] = "xterm-256color"
        env["ZELLIJ_SOCKET_DIR"] = sockets
        name = "probe"
        socket_path = Path(sockets) / "contract_version_1" / name
        config = root / "config.kdl"
        config.write_text('default_shell "/bin/sh"\nshow_startup_tips false\nshow_release_notes false\nmirror_session false\nsession_serialization false\n')
        outer_config = root / "host-config.kdl"
        outer_config.write_text(config.read_text() + 'default_mode "locked"\nnested_session_handling "descend"\n')
        layout = root / "layout.kdl"
        layout.write_text("layout {\n pane\n}\n")
        receiver = root / "receiver.py"
        receiver.write_text("import sys\nfor line in sys.stdin:\n with open(sys.argv[1], 'a') as f: f.write(line); f.flush()\n")
        wrapper = root / "wrapper.py"
        wrapper.write_text(
            "import json, os, sys\n"
            "from pathlib import Path\n"
            "d=Path(sys.argv[1])\n"
            "(d/'host-marker.json').write_text(json.dumps({'host_session':os.environ['ZELLIJ_SESSION_NAME'],'host_pane':os.environ['ZELLIJ_PANE_ID'],'client_pid':os.getpid()}))\n"
            "env={k:v for k,v in os.environ.items() if not k.startswith('ZELLIJ')}\n"
            "env['ZELLIJ_SOCKET_DIR']=sys.argv[2]\n"
            "env['VERIJ_ZELLIJ_IDENTITY_DIR']=str(d)\n"
            "os.execvpe(sys.argv[3],sys.argv[3:],env)\n"
        )
        base = [str(binary), "--config", str(config)]
        def action(*parts):
            result = subprocess.run([*base, "-s", name, "action", *parts], env=env, capture_output=True, text=True, timeout=8)
            if result.returncode or result.stdout.startswith(("Session '", "Please specify")):
                raise RuntimeError((parts, result.returncode, result.stdout[:120], result.stderr[:120]))
            return result.stdout.strip()
        def drain():
            for child in clients:
                if child["master"] is not None and select.select([child["master"]], [], [], 0)[0]:
                    try:
                        data = os.read(child["master"], 65536)
                        with (root / f"{child['directory'].name}.terminal").open("ab") as transcript:
                            transcript.write(data)
                    except OSError:
                        pass
        def wait(predicate, timeout=25):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                drain()
                try:
                    value = predicate()
                    if value:
                        return value
                except (FileNotFoundError, ValueError, RuntimeError, subprocess.TimeoutExpired):
                    pass
                time.sleep(.1)
            raise TimeoutError("disposable probe condition timed out")
        def attach(label, create=False):
            report["stage"] = f"attach {label}"
            directory = root / label
            directory.mkdir(mode=0o700)
            child_env = dict(env, VERIJ_ZELLIJ_IDENTITY_DIR=str(directory))
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 50, 160, 0, 0))
            command = [*base]
            if create:
                command += ["--layout", str(layout)]
            command += ["attach", "-c", name] if create else ["attach", name]
            if create:
                command += ["--", "python3", str(receiver), str(root / "pane0-input")]
            host = label
            if args.nested:
                host_sessions.append(host)
                outer_command = [str(binary), "--config", str(outer_config), "--layout", str(layout), "attach", "-c", host]
            else:
                outer_command = command
            process = subprocess.Popen(
                outer_command, env=env if args.nested else child_env,
                stdin=slave, stdout=slave, stderr=slave, start_new_session=True,
                preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0),
            )
            os.close(slave)
            child = {"process": process, "master": master, "directory": directory}
            clients.append(child)
            if args.nested:
                def host_action(*parts):
                    result = subprocess.run([*base, "-s", host, "action", *parts], env=env, capture_output=True, text=True, timeout=8)
                    if result.returncode or result.stdout.startswith(("Session '", "Please specify")):
                        raise RuntimeError("disposable host action unavailable")
                    return result.stdout.strip()
                wait(lambda: "terminal_0" in host_action("list-clients"))
                pane_id = host_action("new-pane", "--in-place", "--pane-id", "terminal_0", "--close-replaced-pane", "--", "python3", str(wrapper), str(directory), env["ZELLIJ_SOCKET_DIR"], *command)
                wait(lambda: (directory / "host-marker.json").exists())
                marker = json.loads((directory / "host-marker.json").read_text())
                # In-place CLI creation may return empty stdout. The wrapper's
                # inherited real pane ID plus inventory establish existence.
                if pane_id:
                    assert pane_id == f"terminal_{marker['host_pane']}", (pane_id, marker)
                panes = json.loads(host_action("list-panes", "--all", "--json"))
                assert any(not p["is_plugin"] and str(p["id"]) == marker["host_pane"] for p in panes)
                child["host_marker"] = marker
                child["host_action"] = host_action
                # Outer client starts Locked using its own config, while inner
                # client starts Normal. Sending Ctrl+g here can instead lock
                # the inner client when native nested passthrough is active.
            def record():
                expected_pid = child.get("host_marker", {}).get("client_pid", process.pid)
                for file in directory.glob("*.json"):
                    value = json.loads(file.read_text())
                    if value.get("attached") and value["client_pid"] == expected_pid:
                        child["record_path"] = file
                        child["record"] = value
                        return value
            wait(record)
            child["client_birth"] = process_identity(child["record"]["client_pid"])
            child["server_birth"] = process_identity(child["record"]["server_pid"])
            assert child["client_birth"] and child["server_birth"], "attachment processes must be live"
            assert directory.stat().st_mode & 0o777 == 0o700
            assert child["record_path"].stat().st_mode & 0o777 == 0o600
            print(f"attached {label}: client {child['record']['client_id']}", flush=True)
            return child
        def focus_map():
            result = {}
            for line in action("list-clients").splitlines()[1:]:
                words = line.split()
                if len(words) >= 2:
                    result[int(words[0])] = words[1]
            return result
        def target(child, pane):
            reply = control(socket_path, child["record"], pane)
            assert reply == ["verij:accepted"], reply
            wait(lambda: focus_map().get(child["record"]["client_id"]) == f"terminal_{pane}")
            print(f"focused client {child['record']['client_id']} -> pane {pane}", flush=True)
        def send(child, token, pane):
            if args.host_injected_input and args.nested:
                child["host_action"]("write-chars", "--pane-id", "terminal_" + child["host_marker"]["host_pane"], token + "\r")
            else:
                os.write(child["master"], b"\x1b[I")  # terminal focus-in report
                drain()
                time.sleep(.1)
                for character in token + "\r":
                    os.write(child["master"], character.encode())
                    drain()
                    time.sleep(.03)
            wait(lambda: token in (root / f"pane{pane}-input").read_text())
            print(f"input confirmed: {token} -> pane {pane}", flush=True)
        def keys(child, data):
            if args.host_injected_input and args.nested:
                child["host_action"]("write-chars", "--pane-id", "terminal_" + child["host_marker"]["host_pane"], data.decode())
            else:
                os.write(child["master"], data)
        try:
            a = attach("host-a", True)
            wait(lambda: focus_map())
            b = attach("host-b")
            assert a["record"]["client_id"] != b["record"]["client_id"]
            assert a["record"]["server_pid"] == b["record"]["server_pid"]
            assert a["record"]["connection_id"] != b["record"]["connection_id"]
            report["checks"].append({"distinct_display_identity": True, "ids": [a["record"]["client_id"], b["record"]["client_id"]]})
            report["checks"].append({"live_boot_qualified_client_and_server_birth": True,
                                     "private_directory_and_record_modes": "0700/0600"})
            if args.nested:
                report["checks"].append({"actual_host_pane_binding": True, "hosts": [a["host_marker"]["host_session"], b["host_marker"]["host_session"]]})
            pane1 = action("new-pane", "--", "python3", str(receiver), str(root / "pane1-input"))
            assert pane1 == "terminal_1", pane1
            target(a, 0)
            target(b, 1)
            send(a, "first-a", 0)
            send(b, "first-b", 1)
            target(a, 1)
            target(b, 0)
            send(a, "swapped-a", 1)
            send(b, "swapped-b", 0)
            report["checks"].append({"independent_tiled_focus_and_input": True, "focus": focus_map(), "input_transport": "host-pane CLI injection" if args.host_injected_input and args.nested else "display PTY keyboard"})
            if args.placements:
                floating = action("new-pane", "--floating", "--", "python3", str(receiver), str(root / "pane2-input"))
                assert floating == "terminal_2", floating
                target(a, 2)
                send(a, "float-visible", 2)
                keys(a, b"\x1bf")  # Alt+f: hide floating layer in client A
                wait(lambda: focus_map().get(a["record"]["client_id"]) != "terminal_2")
                target(a, 2)
                send(a, "float-revealed", 2)
                target(a, 0)
                send(a, "tiled-after-float", 0)
                report["checks"].append({"hidden_float_revealed_with_input": True})
                keys(a, b"\x10f")  # Ctrl+p, f: fullscreen current tiled pane
                wait(lambda: any(p["id"] == 0 and not p["is_plugin"] and p["is_fullscreen"] for p in json.loads(action("list-panes", "--all", "--json"))))
                target(a, 1)
                send(a, "fullscreen-other-target", 1)
                report["checks"].append({"target_from_other_fullscreen_pane": True})
                action("stack-panes", "--", "terminal_0", "terminal_1")
                target(a, 0)
                send(a, "stack-member-zero", 0)
                target(a, 1)
                send(a, "stack-member-one", 1)
                report["checks"].append({"ordinary_stack_member_focus_and_input": True})
            stale = dict(a["record"])
            report["stage"] = "clean detach record invalidation"
            if args.host_injected_input and args.nested:
                a["host_action"]("write-chars", "--pane-id", "terminal_" + a["host_marker"]["host_pane"], "\x0fd")
            else:
                os.write(a["master"], b"\x0fd")  # client's own Session-mode Detach
            if not args.nested:
                wait(lambda: a["process"].poll() is not None)
            wait(lambda: not json.loads(a["record_path"].read_text())["attached"])
            c = attach("host-a-reconnected")
            assert c["record"]["client_id"] == stale["client_id"]
            assert c["record"]["connection_id"] != stale["connection_id"]
            target(c, 0)
            before = focus_map()
            reply = control(socket_path, stale, 1)
            assert reply == ["verij:rejected"], reply
            assert focus_map() == before
            target(c, 1)
            send(c, "reconnected-a", 1)
            report["checks"].append({"reused_id_new_generation": True, "stale_focus_rejected": True, "clean_detach_invalidated": True})
            if args.lifecycle:
                report["stage"] = "cross-tab focus"
                prior_panes = {p["id"] for p in json.loads(action("list-panes", "--all", "--json")) if not p["is_plugin"]}
                action("new-tab", "--name", "background-target", "--", "python3", str(receiver), str(root / "cross-tab-input"))
                def new_tab_pane():
                    panes = [p for p in json.loads(action("list-panes", "--all", "--json")) if not p["is_plugin"] and p["id"] not in prior_panes]
                    return panes[0] if len(panes) == 1 else None
                cross_tab = wait(new_tab_pane)
                cross_pane = cross_tab["id"]
                # Receiver filenames follow actual inventory IDs, not tab indices.
                (root / f"pane{cross_pane}-input").symlink_to(root / "cross-tab-input")
                target(c, 0)
                target(b, 1)
                before = focus_map()
                target(c, cross_pane)
                assert focus_map()[b["record"]["client_id"]] == before[b["record"]["client_id"]]
                send(c, "cross-tab-c", cross_pane)
                send(b, "cross-tab-b-stayed", 1)
                target(c, 0)
                send(c, "returned-tab-zero", 0)
                other_focus = focus_map()[b["record"]["client_id"]]
                if args.placements:
                    assert other_focus == "terminal_0", other_focus
                    send(b, "shared-stack-expansion-b", 0)
                    report["limitations"].append("Native stack expansion moves other clients focused in the same stack, even with mirror_session=false.")
                    report["checks"].append({"unmirrored_stack_expansion_changes_other_client": True,
                                             "other_client_keyboard_confirmed_in_expanded_member": True})
                else:
                    assert other_focus == "terminal_1", other_focus
                report["checks"].append({"cross_tab_client_specific_focus_and_input": True, "target_pane": cross_pane,
                                         "other_client_focus_after_return": other_focus})

                # Stack expansion can affect another client's active member.
                # Establish distinct tabs before testing rename in isolation.
                target(c, cross_pane)
                target(b, 1)
                report["stage"] = "rename"
                focus_before_rename = focus_map()
                server_birth = c["server_birth"]
                records_before = [json.loads(child["record_path"].read_text()) for child in (b, c)]
                old_name = name
                action("rename-session", "probe-renamed")
                name = "probe-renamed"
                socket_path = Path(sockets) / "contract_version_1" / name
                wait(lambda: socket_path.exists() and focus_map())
                assert not (Path(sockets) / "contract_version_1" / old_name).exists()
                assert process_identity(c["record"]["server_pid"]) == server_birth
                assert records_before == [json.loads(child["record_path"].read_text()) for child in (b, c)]
                assert focus_map() == focus_before_rename
                target(c, cross_pane)
                send(c, "renamed-c", cross_pane)
                send(b, "renamed-b", 1)
                report["checks"].append({"rename_preserves_server_birth_and_attachment_generation": True, "focus_and_input_after_rename": True})

                before = focus_map()
                report["stage"] = "missing target"
                missing_reply = control(socket_path, c["record"], 2**32 - 1)
                assert missing_reply == ["verij:accepted"], missing_reply
                # Verify a live keyboard path still reaches the prior target.
                # Acceptance alone cannot establish the requested missing focus.
                send(c, "missing-target-did-not-focus", cross_pane)
                assert focus_map() == before
                report["checks"].append({"missing_target_acceptance_is_not_completion": True, "reply": missing_reply, "focus_unchanged": True})
                report["limitations"].append("A valid generation with a nonexistent pane is accepted at dispatch; no execution completion/failure response exists.")

                abrupt = dict(c["record"])
                report["stage"] = "abrupt client death and ID reuse"
                assert process_identity(abrupt["client_pid"]) == c["client_birth"]
                os.kill(abrupt["client_pid"], signal.SIGKILL)
                wait(lambda: process_identity(abrupt["client_pid"]) is None)
                wait(lambda: abrupt["client_id"] not in focus_map())
                if not args.nested:
                    c["process"].wait(timeout=4)
                assert json.loads(c["record_path"].read_text())["attached"], "SIGKILL should leave a stale published record"
                assert control(socket_path, abrupt, 0) == ["verij:rejected"]
                d = attach("host-after-abrupt")
                assert d["record"]["client_id"] == abrupt["client_id"]
                assert d["record"]["connection_id"] != abrupt["connection_id"]
                target(d, 0)
                before = focus_map()
                assert control(socket_path, abrupt, cross_pane) == ["verij:rejected"]
                assert focus_map() == before
                send(d, "after-abrupt-reuse", 0)
                report["checks"].append({"abrupt_death_leaves_attached_record": True, "independent_process_liveness_detects_death": True, "stale_generation_rejected_before_and_after_reuse": True})

                report["stage"] = "zero-client first reattachment"
                for child in (b, d):
                    keys(child, b"\x0fd")
                    wait(lambda: not json.loads(child["record_path"].read_text())["attached"])
                wait(lambda: not focus_map())
                assert process_identity(d["record"]["server_pid"]) == server_birth
                empty_before = focus_map()
                assert control(socket_path, d["record"], 0) == ["verij:rejected"]
                assert focus_map() == empty_before
                e = attach("host-first-after-zero")
                assert e["record"]["client_id"] == 1
                assert e["server_birth"] == server_birth
                assert e["record"]["connection_id"] not in {b["record"]["connection_id"], d["record"]["connection_id"]}
                target(e, cross_pane)
                send(e, "first-after-zero-clients", cross_pane)
                report["checks"].append({"zero_client_server_lifetime_preserved": True, "zero_client_old_target_rejected": True, "first_reattachment_focus_and_input": True})
            report.pop("stage", None)
            report["status"] = "PASS"
        except Exception as error:
            report.update(status="FAIL", error=str(error))
            print(f"probe failed: {error}", flush=True)
            try:
                report["failure_focus"] = focus_map()
                report["attachment_records_at_failure"] = [
                    {"record": json.loads(child["record_path"].read_text()),
                     "client_live": process_identity(child["record"]["client_pid"]) == child["client_birth"]}
                    for child in clients if "record_path" in child
                ]
                report["input_files"] = {p.name: p.read_text() for p in root.glob("pane*-input")}
                if args.nested:
                    report["host_focus"] = [c["host_action"]("list-clients") for c in clients if "host_action" in c]
                    report["host_screens"] = [c["host_action"]("dump-screen", "--pane-id", "terminal_" + c["host_marker"]["host_pane"])[-1000:] for c in clients if "host_action" in c]
            except Exception:
                pass
        finally:
            subprocess.run([str(binary), "kill-session", name], env=env, capture_output=True, timeout=8)
            for host in host_sessions:
                subprocess.run([str(binary), "kill-session", host], env=env, capture_output=True, timeout=8)
            for child in clients:
                try:
                    child["process"].wait(timeout=4)
                except subprocess.TimeoutExpired:
                    child["process"].terminate()
                    try:
                        child["process"].wait(timeout=4)
                    except subprocess.TimeoutExpired:
                        child["process"].kill()
                        child["process"].wait(timeout=4)
                os.close(child["master"])
                child["master"] = None
            report["all_clients_exited"] = all(child["process"].poll() is not None for child in clients)
    text = json.dumps(report, indent=2)
    (root / "results.json").write_text(text + "\n")
    print(text)
    if args.output:
        args.output.write_text(text + "\n")
    return 0 if report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
