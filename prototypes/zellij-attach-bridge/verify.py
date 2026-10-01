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
import sys
import tempfile
import termios
import time
import uuid


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
    except (OSError, IndexError, ValueError):
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
                if result >= 2**64:
                    raise ValueError("protobuf varint exceeds uint64")
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
            if length > len(data) - offset:
                raise ValueError("truncated protobuf length-delimited field")
            value, offset = data[offset:offset + length], offset + length
        else:
            raise ValueError("unexpected wire type")
        result.append((tag >> 3, value))
    return result


def receive_reply(connection, reply_field, timeout=5):
    deadline = time.monotonic() + timeout
    def exact(length):
        data = bytearray()
        while len(data) < length:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("control result deadline expired")
            connection.settimeout(remaining)
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
            if field == reply_field:
                return value


def exchange(socket_path, wire, reply_field):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(5)
        connection.connect(str(socket_path))
        connection.sendall(struct.pack("<I", len(wire)) + wire)
        return receive_reply(connection, reply_field)


def control(socket_path, record, pane_id):
    payload = integer(1, record["client_id"]) + blob(2, record["connection_id"].encode()) + integer(3, pane_id)
    return [line.decode() for number, line in fields(exchange(socket_path, blob(29, payload), 5)) if number == 1]


def completion_wire(record, pane_id, sequence, query_only, request_id):
    for name, value, limit in (("client_id", record["client_id"], 2**16),
                               ("pane_id", pane_id, 2**32), ("sequence", sequence, 2**64)):
        if type(value) is not int or not 0 <= value < limit:
            raise ValueError(f"invalid {name} wire range")
    payload = (blob(1, request_id.encode()) + integer(2, record["client_id"])
               + blob(3, record["connection_id"].encode()) + integer(4, pane_id)
               + integer(5, sequence) + integer(6, int(query_only)))
    return blob(30, payload)


def completion_result(wire, record, pane_id, sequence, query_only, request_id):
    reply = dict(fields(wire))
    echoed = dict(fields(reply[1]))
    assert echoed[1].decode() == request_id
    assert echoed[2] == record["client_id"]
    assert echoed[3].decode() == record["connection_id"]
    assert echoed.get(4, 0) == pane_id
    assert echoed.get(5, 0) == sequence
    assert bool(echoed.get(6, 0)) == query_only
    assert reply[6] == record["server_pid"], "completion must identify the bound server"
    return {"request_id": request_id, "client_id": record["client_id"],
            "sequence": sequence, "query_only": query_only, "requested_pane": pane_id,
            "status": reply[2].decode(), "focused_pane_id": reply.get(3),
            "focused_is_plugin": bool(reply.get(4, 0)), "tab_id": reply.get(5),
            "latest_sequence": reply.get(7)}


def completion_control(socket_path, record, pane_id, sequence=0, query_only=False, request_id=None):
    request_id = request_id or uuid.uuid4().hex
    wire = completion_wire(record, pane_id, sequence, query_only, request_id)
    return completion_result(exchange(socket_path, wire, 21), record, pane_id, sequence, query_only, request_id)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--scratch-dir", type=Path, help="Parent for disposable probe files; defaults to durable prototype .scratch directory")
    parser.add_argument("--nested", action="store_true", help="Run inner attachments in real disposable host Zellij panes")
    parser.add_argument("--host-injected-input", action="store_true", help="Diagnostic: write to the actual host pane instead of simulating outer keyboard input")
    parser.add_argument("--placements", action="store_true", help="Also exercise floating, fullscreen and ordinary stack targets")
    parser.add_argument("--lifecycle", action="store_true", help="Also exercise cross-tab focus, rename, missing targets, abrupt death and zero-client reattachment")
    parser.add_argument("--completion", action="store_true", help="Use correlated execution results and test read-only queries/superseded requests")
    parser.add_argument("--races", action="store_true", help="Deterministic live pre-execution and pre-delivery races (debug binary only)")
    parser.add_argument("--recovery", action="store_true", help="Exercise independent controller processes and durable sequence recovery")
    parser.add_argument("--control-stress", action="store_true", help="Exercise rapid legacy rejection/typed-query connection turnover")
    parser.add_argument("--stack-list", action="store_true", help="Use title-list stacks and verify hidden member geometry/query/reveal")
    parser.add_argument("--mirrored", action="store_true", help="Verify native shared-focus behavior with mirror_session=true")
    args = parser.parse_args()
    if (args.races or args.recovery) and not args.completion:
        parser.error("--races/--recovery require --completion")
    if args.races and args.nested:
        parser.error("live race barriers currently support direct disposable displays")
    if args.control_stress and not args.races:
        parser.error("--control-stress requires --races")
    if args.stack_list and not (args.placements and args.completion):
        parser.error("--stack-list requires --placements --completion")
    if args.mirrored and (args.races or args.recovery or args.lifecycle):
        parser.error("--mirrored currently covers completion and placements without lifecycle/races/recovery")
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
        probe_dir = root / "race-events"
        if args.races:
            probe_dir.mkdir(mode=0o700)
            env["VERIJ_ZELLIJ_PROBE_DIR"] = str(probe_dir)
        name = "probe"
        socket_path = Path(sockets) / "contract_version_1" / name
        config = root / "config.kdl"
        config.write_text('default_shell "/bin/sh"\nshow_startup_tips false\nshow_release_notes false\n'
                          + f'mirror_session {str(args.mirrored).lower()}\n'
                          + f'stacked_pane_list {str(args.stack_list).lower()}\n'
                          + 'session_serialization false\n')
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
        sequences = {}
        def completed(record, pane, query_only=False, sequence=None):
            generation = record["connection_id"]
            if sequence is None:
                sequence = 0 if query_only else sequences.get(generation, 0) + 1
            if not query_only:
                sequences[generation] = max(sequence, sequences.get(generation, 0))
            result = completion_control(socket_path, record, pane, sequence, query_only)
            with (root / "completion-results.ndjson").open("a") as fixture:
                fixture.write(json.dumps(result) + "\n")
            return result
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
            if args.completion:
                reply = completed(child["record"], pane)
                assert reply["status"] == "focused" and reply["focused_pane_id"] == pane and not reply["focused_is_plugin"], reply
            else:
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
        pending_controls = []
        def begin(record, pane, query_only=False, phase=None):
            request_id = uuid.uuid4().hex
            generation = record["connection_id"]
            sequence = 0 if query_only else sequences.get(generation, 0) + 1
            if not query_only:
                sequences[generation] = sequence
            hold = probe_dir / f"{request_id}.{phase}.hold" if phase else None
            if hold:
                hold.touch()
            connection = socket.socket(socket.AF_UNIX)
            connection.settimeout(5)
            connection.connect(str(socket_path))
            wire = completion_wire(record, pane, sequence, query_only, request_id)
            connection.sendall(struct.pack("<I", len(wire)) + wire)
            request = {"id": request_id, "connection": connection, "record": record,
                       "pane": pane, "sequence": sequence, "query_only": query_only, "hold": hold}
            pending_controls.append(request)
            queued = event(request_id + ".queued")
            assert queued["failure"] is None, queued
            request["queued"] = queued
            if phase:
                event(request_id + "." + phase)
            return request
        def event(name):
            return wait(lambda: json.loads((probe_dir / f"{name}.json").read_text()), timeout=8)
        def release(request):
            request["hold"].unlink(missing_ok=True)
        def finish(request):
            try:
                reply = receive_reply(request["connection"], 21)
                return completion_result(reply, request["record"], request["pane"],
                                         request["sequence"], request["query_only"], request["id"])
            finally:
                request["connection"].close()
        def races(a, b):
            report["stage"] = "queued navigation supersession"
            target(a, 0)
            first = begin(a["record"], 1, phase="execute")
            newest = begin(a["record"], 0)
            release(first)
            old_result, new_result = finish(first), finish(newest)
            assert old_result["status"] == "superseded", old_result
            assert event(first["id"] + ".executed")["result"]["status"] == "superseded"
            assert new_result["status"] == "focused", new_result
            send(a, "race-queued-newest-only", 0)
            report["checks"].append({"live_queued_supersession_before_execution": True,
                                     "old": old_result, "new": new_result})

            report["stage"] = "in-flight completion supersession"
            first = begin(a["record"], 1, phase="result")
            execution = event(first["id"] + ".executed")["result"]
            assert execution["status"] == "focused" and execution["focused_pane_id"] == 1, execution
            newest = begin(a["record"], 0)
            release(first)
            old_result, new_result = finish(first), finish(newest)
            assert old_result["status"] == "superseded", old_result
            assert old_result["focused_pane_id"] is None, old_result
            assert new_result["status"] == "focused", new_result
            send(a, "race-inflight-no-stale-success", 0)
            report["checks"].append({"live_delivery_supersession_after_mutation": True,
                                     "prior_mutation_observed": execution["focused_pane_id"],
                                     "old": old_result, "new": new_result})

            report["stage"] = "cancelled requester and numeric control ID reuse"
            cancelled = begin(a["record"], 1, phase="execute")
            cancelled["connection"].close()
            queued = cancelled["queued"]
            event("removed-" + queued["requester_generation"])
            replacement = begin(a["record"], 0, query_only=True)
            assert replacement["queued"]["requester"] == queued["requester"], (queued, replacement["queued"])
            assert replacement["queued"]["requester_generation"] != queued["requester_generation"]
            release(cancelled)
            executed = event(cancelled["id"] + ".executed")["result"]
            assert executed["status"] == "cancelled", executed
            new_result = finish(replacement)  # Correlation rejects a result delivered to the recycled socket.
            assert new_result["status"] == "focused" and new_result["focused_pane_id"] == 0, new_result
            assert not (probe_dir / f"{cancelled['id']}.delivered.json").exists()
            send(a, "race-cancelled-control-stayed", 0)
            report["checks"].append({"live_requester_disconnect_prevents_mutation": True,
                                     "recycled_control_id_does_not_receive_old_result": True,
                                     "requester_id": queued["requester"], "query": new_result})

            report["stage"] = "delayed EOF cleanup after explicit ClientExited and socket ID reuse"
            abandoned = begin(a["record"], 1, phase="execute")
            queued = abandoned["queued"]
            old_generation = queued["requester_generation"]
            route_hold = probe_dir / f"{old_generation}.route-end.hold"
            route_hold.touch()
            wire = blob(11, b"")  # ClientExited queues removal before route EOF cleanup.
            abandoned["connection"].sendall(struct.pack("<I", len(wire)) + wire)
            event(old_generation + ".route-end")
            event("removed-" + old_generation)
            replacement = begin(a["record"], 0, query_only=True)
            assert replacement["queued"]["requester"] == queued["requester"]
            assert replacement["queued"]["requester_generation"] != old_generation
            route_hold.unlink()
            event("cleanup-skipped-" + old_generation)
            release(abandoned)
            assert event(abandoned["id"] + ".executed")["result"]["status"] == "cancelled"
            new_result = finish(replacement)
            assert new_result["status"] == "focused" and new_result["focused_pane_id"] == 0, new_result
            abandoned["connection"].close()
            send(a, "race-delayed-eof-cleanup-stayed", 0)
            report["checks"].append({"delayed_eof_cleanup_cannot_remove_reused_socket": True,
                                     "recycled_control_id": queued["requester"], "query": new_result})

            for phase in ("execute", "result"):
                report["stage"] = "target display reuse before " + phase
                stale = dict(a["record"])
                first = begin(stale, 1, phase=phase)
                os.kill(stale["client_pid"], signal.SIGKILL)
                event("removed-" + stale["connection_id"])
                a = attach("race-display-replacement-" + phase)
                assert a["record"]["client_id"] == stale["client_id"]
                assert a["record"]["connection_id"] != stale["connection_id"]
                release(first)
                old_result = finish(first)
                assert old_result["status"] == "stale_attachment", old_result
                assert old_result["latest_sequence"] is None, old_result
                execution = event(first["id"] + ".executed")["result"]
                assert execution["status"] == ("stale_attachment" if phase == "execute" else "focused"), execution
                # RemoveClient/AddClient are still queued behind the old task;
                # the live registry must reject it despite stale screen state.
                wait(lambda: a["record"]["client_id"] in focus_map())
                replacement_focus = completed(a["record"], 0, query_only=True)
                assert replacement_focus["latest_sequence"] == 0, replacement_focus
                target(a, 0)
                send(a, "race-disconnected-generation-" + phase, 0)
                report["checks"].append({"live_target_disconnect_and_reuse": phase,
                                         "display_id_reused_while_screen_paused": True,
                                         "stale": old_result, "replacement_generation_starts_at_one": True})
            target(b, 1)
            send(b, "race-other-display-stayed", 1)
            return a
        controller_processes = []
        def recovery(child, label):
            report["stage"] = "controller sequence recovery " + label
            from controller import durable_write
            directory = root / ("controller-" + label)
            directory.mkdir(mode=0o700)
            binding = directory / "binding.json"
            binding.write_text(json.dumps({"record": child["record"], "server_birth": child["server_birth"],
                                           "client_birth": child["client_birth"]}))
            journal = root / "controller-journal"
            probe = directory / "barriers"
            probe.mkdir(mode=0o700)
            command = [sys.executable, str(Path(__file__).with_name("controller.py")),
                       "--socket", str(socket_path), "--binding", str(binding), "--journal-dir", str(journal)]
            def start(pane, pause=False):
                process = subprocess.Popen([*command, "--pane", str(pane), *(["--probe-dir", str(probe)] if pause else [])],
                                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                controller_processes.append(process)
                return process
            def result(process) -> dict:
                stdout, stderr = process.communicate(timeout=8)
                assert process.returncode == 0, (stdout, stderr)
                return json.loads(stdout)
            def failed(process):
                stdout, stderr = process.communicate(timeout=8)
                assert process.returncode != 0, (stdout, stderr)
                return stderr
            before = completed(child["record"], 0, query_only=True)["latest_sequence"]
            first, restarted = result(start(0)), result(start(1))
            assert first["sequence"] == before + 1 and restarted["sequence"] == first["sequence"] + 1
            send(child, "recovery-restarted-" + label, 1)

            # Crash after fsync but before send. Recovery skips the unsent reservation.
            (probe / "reserved.hold").touch()
            crashed = start(0, pause=True)
            reservation = wait(lambda: json.loads((probe / "reserved.json").read_text()), timeout=8)
            crashed.kill()
            crashed.communicate(timeout=4)
            (probe / "reserved.hold").unlink()
            recovered = result(start(1))
            assert recovered["sequence"] == reservation["sequence"] + 1, recovered
            send(child, "recovery-reserved-crash-" + label, 1)

            if args.races:
                # Crash with a reservation actually queued on the live screen.
                (probe / "reserved.json").unlink()
                (probe / "reserved.hold").touch()
                crashed = start(0, pause=True)
                queued_reservation = wait(lambda: json.loads((probe / "reserved.json").read_text()), timeout=8)
                request_id = queued_reservation["request_id"]
                screen_hold = probe_dir / f"{request_id}.execute.hold"
                screen_hold.touch()
                (probe / "reserved.hold").unlink()
                queued = event(request_id + ".queued")
                event(request_id + ".execute")
                crashed.kill()
                crashed.communicate(timeout=4)
                event("removed-" + queued["requester_generation"])
                screen_hold.unlink()
                assert event(request_id + ".executed")["result"]["status"] == "cancelled"
                recovered = result(start(1))
                assert recovered["sequence"] == queued_reservation["sequence"] + 1, recovered
                send(child, "recovery-queued-crash-" + label, 1)

            # Crash after server completion but before result publication.
            (probe / "completed.hold").touch()
            crashed = start(0, pause=True)
            lost_result = wait(lambda: json.loads((probe / "completed.json").read_text()), timeout=8)
            crashed.kill()
            crashed.communicate(timeout=4)
            (probe / "completed.hold").unlink()
            recovered = result(start(1))
            assert recovered["sequence"] == lost_result["sequence"] + 1, recovered
            send(child, "recovery-lost-result-" + label, 1)

            # Cold journal can recover from server reservations; corrupt journal fails closed.
            journal_path = Path(reservation["journal"])
            saved = json.loads(journal_path.read_text())
            journal_path.unlink()
            cold = result(start(0))
            assert cold["sequence"] == recovered["sequence"] + 1, cold
            send(child, "recovery-cold-journal-" + label, 0)
            journal_path.write_text("{corrupt")
            before_focus = focus_map()
            failure = failed(start(1))
            assert "JSONDecodeError" in failure and focus_map() == before_focus
            assert journal_path.read_text() == "{corrupt"
            saved["reserved_sequence"] = 2**64 - 1
            durable_write(journal_path, saved)
            assert "OverflowError" in failed(start(1)) and focus_map() == before_focus
            saved["reserved_sequence"] = cold["sequence"]
            durable_write(journal_path, saved)

            # Two independent producers share one inode lock and reserve distinct sequences.
            left, right = start(0), start(0)
            parallel = [result(left), result(right)]
            assert sorted(item["sequence"] for item in parallel) == [cold["sequence"] + 1, cold["sequence"] + 2]
            send(child, "recovery-two-producers-" + label, 0)
            latest = max(item["sequence"] for item in parallel)
            sequences[child["record"]["connection_id"]] = latest
            report["checks"].append({"controller_recovery": label, "distinct_process_restarts": True,
                                     "fsynced_unsent_reservation_skipped": True, "lost_completion_not_replayed": True,
                                     "controller_killed_while_live_queued": args.races,
                                     "missing_journal_recovered_from_server": True, "corrupt_journal_refused_without_mutation": True,
                                     "exhausted_sequence_refused_without_mutation": True,
                                     "shared_lock_serializes_two_producers": True, "initial_server_watermark": before,
                                     "last_sequence": latest, "journal": str(journal_path),
                                     "shared_directory_generation_isolation": True})
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
            if args.mirrored:
                assert all(focus_map().get(child["record"]["client_id"]) == "terminal_0" for child in (a, b))
                send(a, "mirrored-a-zero", 0)
                send(b, "mirrored-b-zero", 0)
                target(b, 1)
                assert all(focus_map().get(child["record"]["client_id"]) == "terminal_1" for child in (a, b))
                send(a, "mirrored-a-one", 1)
                send(b, "mirrored-b-one", 1)
                report["checks"].append({"native_mirrored_tiled_focus_shared": True, "both_clients_keyboard_confirmed": True})
                report["limitations"].append("Native mirrored focus is shared; this does not acknowledge Done in another host.")
            else:
                target(b, 1)
                send(a, "first-a", 0)
                send(b, "first-b", 1)
                target(a, 1)
                target(b, 0)
                send(a, "swapped-a", 1)
                send(b, "swapped-b", 0)
                report["checks"].append({"independent_tiled_focus_and_input": True, "focus": focus_map(), "input_transport": "host-pane CLI injection" if args.host_injected_input and args.nested else "display PTY keyboard"})
            if args.completion:
                report["stage"] = "correlated completion and query"
                before = focus_map()
                current = completed(a["record"], 1, query_only=True)
                assert current["status"] == "focused" and current["focused_pane_id"] == 1, current
                assert current["tab_id"] == 0, current
                other = completed(a["record"], 0, query_only=True)
                assert other["status"] == "not_focused" and other["focused_pane_id"] == 1, other
                assert focus_map() == before, "queries must not change any client focus"
                missing = completed(a["record"], 2**32 - 1)
                assert missing["status"] == "pane_missing" and missing["focused_pane_id"] == 1, missing
                assert focus_map() == before
                send(a, "completion-missing-stayed", 1)
                latest_sequence = sequences[a["record"]["connection_id"]]
                old = completed(a["record"], 0, sequence=latest_sequence - 1)
                duplicate = completed(a["record"], 0, sequence=latest_sequence)
                assert old["status"] == duplicate["status"] == "superseded", (old, duplicate)
                invalid = completed(a["record"], 0, sequence=0)
                assert invalid["status"] == "invalid_request", invalid
                assert focus_map() == before
                send(a, "completion-superseded-stayed", 1)
                target(a, 0)
                send(a, "completion-fresh-zero", 0)
                report["checks"].append({"correlated_screen_execution_result": True, "read_only_effective_focus_query": True,
                                         "pane_and_tab_zero_preserved": True, "missing_target_failed_at_execution": True,
                                         "older_and_duplicate_navigation_rejected": True, "zero_navigation_sequence_rejected": True})
            if args.races:
                if args.control_stress:
                    stale_control = dict(a["record"], connection_id=uuid.uuid4().hex)
                    for turn in range(32):
                        report["stage"] = f"legacy/control turnover {turn}"
                        assert control(socket_path, stale_control, 1) == ["verij:rejected"]
                        assert completed(a["record"], 0, query_only=True)["status"] == "focused"
                    report["checks"].append({"rapid_legacy_rejection_and_typed_query_turnover": 32})
                a = races(a, b)
            if args.recovery:
                recovery(a, "same-attachment")
            if args.placements:
                floating = action("new-pane", "--floating", "--", "python3", str(receiver), str(root / "pane2-input"))
                assert floating == "terminal_2", floating
                target(a, 2)
                send(a, "float-visible", 2)
                keys(a, b"\x1bf")  # Alt+f: hide floating layer in client A
                wait(lambda: focus_map().get(a["record"]["client_id"]) != "terminal_2")
                if args.completion:
                    before = focus_map()
                    hidden = completed(a["record"], 2, query_only=True)
                    assert hidden["status"] == "not_focused" and hidden["focused_pane_id"] != 2, hidden
                    assert focus_map() == before
                target(a, 2)
                send(a, "float-revealed", 2)
                target(a, 0)
                send(a, "tiled-after-float", 0)
                report["checks"].append({"hidden_float_revealed_with_input": True})
                keys(a, b"\x10f")  # Ctrl+p, f: fullscreen current tiled pane
                wait(lambda: any(p["id"] == 0 and not p["is_plugin"] and p["is_fullscreen"] for p in json.loads(action("list-panes", "--all", "--json"))))
                if args.completion:
                    hidden = completed(a["record"], 1, query_only=True)
                    assert hidden["status"] == "not_focused" and hidden["focused_pane_id"] == 0, hidden
                target(a, 1)
                send(a, "fullscreen-other-target", 1)
                report["checks"].append({"target_from_other_fullscreen_pane": True})
                action("stack-panes", "--", "terminal_0", "terminal_1")
                target(a, 0)
                send(a, "stack-member-zero", 0)
                if args.completion:
                    before = focus_map()
                    collapsed = completed(a["record"], 1, query_only=True)
                    assert collapsed["status"] == "not_focused" and collapsed["focused_pane_id"] == 0, collapsed
                    assert focus_map() == before
                    if args.stack_list:
                        panes = json.loads(action("list-panes", "--all", "--json"))
                        hidden_member = next(p for p in panes if p["id"] == 1 and not p["is_plugin"])
                        assert hidden_member["is_suppressed"], hidden_member
                        report["checks"].append({"hidden_stack_list_member_in_inventory": True,
                                                 "member_id": 1, "pane_rows": hidden_member["pane_rows"],
                                                 "content_rows": hidden_member["pane_content_rows"],
                                                 "is_suppressed": hidden_member["is_suppressed"],
                                                 "passive_query_not_effectively_focused": True})
                target(a, 1)
                send(a, "stack-member-one", 1)
                if args.stack_list:
                    panes = json.loads(action("list-panes", "--all", "--json"))
                    visible_member = next(p for p in panes if p["id"] == 1 and not p["is_plugin"])
                    assert not visible_member["is_suppressed"] and visible_member["pane_content_rows"] > 0, visible_member
                    report["checks"].append({"hidden_stack_list_member_revealed_with_keyboard": True,
                                             "content_rows_after_focus": visible_member["pane_content_rows"]})
                report["checks"].append({"ordinary_stack_member_focus_and_input": True})
                if args.completion:
                    report["checks"].append({"queries_respect_hidden_float_fullscreen_and_collapsed_stack": True})
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
            report["stage"] = "legacy stale request after reattachment"
            reply = control(socket_path, stale, 1)
            assert reply == ["verij:rejected"], reply
            if args.completion:
                old_result = completed(stale, 1)
                assert old_result["status"] == "stale_attachment", old_result
            assert focus_map() == before
            target(c, 1)
            send(c, "reconnected-a", 1)
            if args.recovery:
                recovery(c, "new-generation")
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
                if args.completion:
                    failure = completed(c["record"], 2**32 - 1)
                    assert failure["status"] == "pane_missing" and failure["focused_pane_id"] == cross_pane, failure
                    report["checks"].append({"legacy_missing_acceptance_replaced_by_typed_failure": True})
                report["limitations"].append("Legacy focus dispatch still accepts a nonexistent pane; use the new correlated request path for execution outcome.")

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
                if args.completion:
                    assert completed(abrupt, 0)["status"] == "stale_attachment"
                d = attach("host-after-abrupt")
                assert d["record"]["client_id"] == abrupt["client_id"]
                assert d["record"]["connection_id"] != abrupt["connection_id"]
                target(d, 0)
                before = focus_map()
                assert control(socket_path, abrupt, cross_pane) == ["verij:rejected"]
                if args.completion:
                    assert completed(abrupt, cross_pane)["status"] == "stale_attachment"
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
                if args.completion:
                    assert completed(d["record"], 0)["status"] == "stale_attachment"
                assert focus_map() == empty_before
                e = attach("host-first-after-zero")
                assert e["record"]["client_id"] == 1
                assert e["server_birth"] == server_birth
                assert e["record"]["connection_id"] not in {b["record"]["connection_id"], d["record"]["connection_id"]}
                target(e, cross_pane)
                send(e, "first-after-zero-clients", cross_pane)
                report["checks"].append({"zero_client_server_lifetime_preserved": True, "zero_client_old_target_rejected": True, "first_reattachment_focus_and_input": True})
            report.pop("stage", None)
            if args.races:
                assert not list(probe_dir.glob("*.expired.json")), "a live probe barrier expired"
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
            if args.races:
                for hold in probe_dir.glob("*.hold"):
                    hold.unlink(missing_ok=True)
            for request in pending_controls:
                if request["hold"]:
                    release(request)
                request["connection"].close()
            for process in controller_processes:
                if process.poll() is None:
                    process.kill()
                process.communicate(timeout=4)
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
