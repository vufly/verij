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
import socket
import struct
import subprocess
import tempfile
import termios
import time


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
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    report = {"checks": [], "binary": str(binary)}
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
                        os.read(child["master"], 65536)
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
            report["status"] = "PASS"
        except Exception as error:
            report.update(status="FAIL", error=str(error))
            print(f"probe failed: {error}", flush=True)
            try:
                report["failure_focus"] = focus_map()
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
