#!/usr/bin/env python3
"""P0 CLI-loading smoke, not V2 monitoring qualification (Linux only).

Requires an explicitly selected, hash-pinned ELF. All configuration/storage and
TTYs are private. Output is allowlisted metadata; terminal bytes are discarded.
Only recorded private process births are signalled, through pidfds.
"""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pty
import select
import secrets
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import time

HERE = Path(__file__).resolve().parent
ARTIFACTS = ("pin.json", "contract-probe.mjs", "smoke.py", "verify_evidence.py")
MAX_TRACE = 128 * 1024


def sha(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def identity(pid):
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        if fields[0] in ("Z", "X", "x"):
            return None
        return {"pid": pid, "ppid": int(fields[1]), "pgrp": int(fields[2]),
                "session": int(fields[3]), "tty": int(fields[4]), "tpgid": int(fields[5]),
                "start": int(fields[19])}
    except (OSError, ValueError, IndexError):
        return None


def same_birth(before, now):
    return bool(before and now and (before["pid"], before["start"]) == (now["pid"], now["start"]))


def signal_birth(before, number):
    """Never signal a reused PID or an unrecorded process group."""
    if not before:
        return
    try:
        fd = os.pidfd_open(before["pid"])
    except ProcessLookupError:
        return
    try:
        if same_birth(before, identity(before["pid"])):
            try:
                signal.pidfd_send_signal(fd, number)
            except ProcessLookupError:
                pass
    finally:
        os.close(fd)


def environment(root):
    # Deliberately do not inherit PATH, credentials, proxy, runtime injection,
    # OPENCODE_*, server discovery, Zellij, or Verij overrides.
    env = {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color", "LANG": "C.UTF-8",
           "SHELL": "/bin/sh", "HOME": str(root / "home"), "TMPDIR": str(root / "tmp"),
           "OPENCODE_CONFIG_DIR": str(root / "config/opencode"),
           "OPENCODE_DISABLE_AUTOUPDATE": "1", "OPENCODE_DISABLE_MODELS_FETCH": "1",
           "OPENCODE_DISABLE_PROJECT_CONFIG": "1", "OPENCODE_DISABLE_CLAUDE_CODE": "1",
           "OPENCODE_PASSWORD": secrets.token_urlsafe(32), "VJ_V2_PROBE_ROOT": str(root),
           "VERIJ_AGENT_STATE_DIR": str(root / "verij/runtime"),
           "VERIJ_STATES_DIR": str(root / "verij/inventory")}
    for kind in ("CONFIG", "CACHE", "DATA", "STATE"):
        env[f"XDG_{kind}_HOME"] = str(root / kind.lower())
    return env


def prepare(root):
    # CLI plugin discovery has its own ancestor walk, independent of the server's
    # project-config flag. Refuse ambient discovery roots before any child starts.
    for parent in (root, *root.parents):
        if any((parent / name).exists() or (parent / name).is_symlink()
               for name in (".opencode", ".git", ".hg", "AGENTS.md")):
            raise ValueError("ambient_project_resources")
    for directory in ("home", "tmp", "config/opencode", "cache", "data", "state", "work", "fixture",
                      "verij/runtime", "verij/inventory"):
        (root / directory).mkdir(parents=True, mode=0o700)
    # No prompt/model turns. Disable updates explicitly, even if an env flag changes.
    (root / "config/opencode/opencode.json").write_text(json.dumps({"autoupdate": False}))
    # Tagged CLI skips direct file entries. Host.resolve loads a directory's
    # `tui` subpath; copy only the fixture, without repo/project dependencies.
    (root / "fixture/tui.js").write_bytes((HERE / "contract-probe.mjs").read_bytes())
    (root / "fixture/package.json").write_text(json.dumps({"name": "verij-v2-contract-fixture", "type": "module", "private": True}))
    (root / "config/opencode/cli.json").write_text(json.dumps({"plugins": [
        {"package": str(root / "fixture"), "options": {"fixture": "verij-v2-p0"}}
    ]}))
    return environment(root)


def records(path):
    if not path.exists():
        return []
    with path.open("rb") as stream:
        raw = stream.read(MAX_TRACE + 1)
    if len(raw) > MAX_TRACE:
        raise ValueError("trace_bound")
    lines = raw.splitlines(keepends=True)
    if len(lines) > 64 or any(len(line) > 2048 for line in lines):
        raise ValueError("frame_bound")
    # An in-progress final append is not evidence yet.
    from verify_evidence import bounded_json
    return [bounded_json(line, maximum=2048) for line in lines if line.endswith(b"\n")]


class PrivateRun:
    def __init__(self, binary, root, mode):
        self.binary, self.root, self.mode = binary, root, mode
        self.env = prepare(root)
        self.processes = []
        self.births = {}
        self.master = None
        self.report = {"mode": mode, "status": "INCOMPLETE", "failure": None, "frames": [],
                       "client": None, "listener_owned": None, "layout": [], "cleanup": False}

    def remember(self, process):
        self.processes.append(process)
        current = identity(process.pid)
        if current:
            self.births[process.pid] = current
        return process

    def descendants(self):
        current = {}
        for directory in Path("/proc").iterdir():
            if directory.name.isdigit():
                item = identity(int(directory.name))
                if item:
                    current[item["pid"]] = item
        parents = {pid for pid, birth in self.births.items() if same_birth(birth, current.get(pid))}
        while True:
            children = {pid for pid, item in current.items() if item["ppid"] in parents}
            new = children - parents
            if not new:
                return
            if len(set(self.births) | new) > 256:
                raise ValueError("process_bound")
            for pid in new:
                self.births[pid] = current[pid]
            parents |= new

    def drain(self):
        # Bounded bytes per scheduler iteration; never persist terminal content.
        for _ in range(16):
            if self.master is None or not select.select([self.master], [], [], 0)[0]:
                return
            try:
                data = os.read(self.master, 65536)
            except OSError:
                return
            if not data:
                return
            if b"\x1b[6n" in data:
                os.write(self.master, b"\x1b[1;1R")

    def wait(self, predicate, timeout=25):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.descendants()
            self.drain()
            result = predicate()
            if result:
                return result
            time.sleep(.05)
        raise TimeoutError("private_deadline")

    def version(self):
        read, write = os.pipe()
        try:
            child = self.remember(subprocess.Popen([str(self.binary), "--version"],
                cwd=self.root / "work", env=self.env, stdin=subprocess.DEVNULL,
                stdout=write, stderr=subprocess.DEVNULL, start_new_session=True))
        finally:
            os.close(write)
        data = bytearray()
        try:
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                self.descendants()
                if select.select([read], [], [], .05)[0]:
                    part = os.read(read, 4097)
                    data.extend(part)
                    if len(data) > 4096:
                        raise ValueError("version_bound")
                    if not part:
                        break
            else:
                raise TimeoutError("version_deadline")
            child.wait(timeout=1)
            if child.returncode or bytes(data).strip() != b"opencode v2.0.32":
                raise ValueError("version_mismatch")
        finally:
            os.close(read)

    def serve(self):
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        server = self.remember(subprocess.Popen([str(self.binary), "serve", "--hostname", "127.0.0.1", "--port", str(port)],
            cwd=self.root / "work", env=self.env, stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True))

        def owns_listener():
            if server.poll() is not None:
                raise RuntimeError("server_exit")
            # Refuse a port race: prove the listener belongs to this exact private server.
            listeners = {line.split()[9] for line in Path(f"/proc/{server.pid}/net/tcp").read_text().splitlines()[1:]
                         if line.split()[1] == f"0100007F:{port:04X}" and line.split()[3] == "0A"}
            targets = set()
            for fd in Path(f"/proc/{server.pid}/fd").iterdir():
                try:
                    targets.add(os.readlink(fd))
                except FileNotFoundError:
                    pass
            return bool(targets & {f"socket:[{inode}]" for inode in listeners})

        self.wait(owns_listener)
        self.report["listener_owned"] = True
        return f"http://127.0.0.1:{port}"

    def launch(self):
        self.version()
        args = [str(self.binary)]
        args += ["--standalone"] if self.mode == "standalone" else ["--server", self.serve()]
        master, slave = pty.openpty()
        self.master = master
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 140, 0, 0))
        # Avoid preexec_fn: acquire controlling TTY in a single-threaded helper.
        helper = "import fcntl,os,sys,termios;fcntl.ioctl(0,termios.TIOCSCTTY,0);os.execvpe(sys.argv[1],sys.argv[1:],os.environ)"
        try:
            client = self.remember(subprocess.Popen([sys.executable, "-c", helper, *args],
                cwd=self.root / "work", env=self.env, stdin=slave, stdout=slave,
                stderr=slave, start_new_session=True))
        finally:
            os.close(slave)

        def loaded():
            if client.poll() is not None:
                raise RuntimeError("client_exit")
            frames = records(self.root / "probe.ndjson")
            return frames if any(frame.get("kind") == "route" for frame in frames) else None

        frames = self.wait(loaded)
        current = identity(client.pid)
        if not current or not same_birth(self.births.get(client.pid), current):
            raise ValueError("client_birth")
        if not (current["tty"] != 0 and current["pgrp"] == current["tpgid"] == current["pid"] == current["session"]):
            raise ValueError("client_tty")
        if Path(f"/proc/{client.pid}/exe").resolve() != self.binary:
            raise ValueError("client_executable")
        self.report["client"] = current
        self.descendants()
        for birth in self.births.values():
            live = identity(birth["pid"])
            if not same_birth(birth, live):
                continue
            executable = Path(f"/proc/{birth['pid']}/exe").resolve()
            if not same_birth(birth, identity(birth["pid"])):
                raise ValueError("layout_birth_changed")
            role = "client" if birth["pid"] == client.pid else "server" if executable == self.binary else "helper"
            self.report["layout"].append({"role": role, "birth": live})
        if not any(item["role"] == "server" for item in self.report["layout"]):
            raise ValueError("private_server_birth")
        self.report["frames"] = frames
        # Ctrl-C is fixture lifecycle control, not an answer/cancel/navigation call.
        os.write(master, b"\x03")
        self.wait(lambda: any(frame["kind"] == "cleanup" for frame in records(self.root / "probe.ndjson")), timeout=8)
        self.report["frames"] = records(self.root / "probe.ndjson")
        self.report["status"] = "SMOKE_PASS"

    def close(self):
        # Discover while known parents still exist. No killpg, pkill, service stop,
        # or discovery of unrelated shared servers. Births are checked via pidfd.
        try:
            self.descendants()
            for number, timeout in ((signal.SIGTERM, 3), (signal.SIGKILL, 2)):
                for birth in reversed(list(self.births.values())):
                    signal_birth(birth, number)
                deadline = time.monotonic() + timeout
                while time.monotonic() < deadline:
                    self.descendants()
                    self.drain()
                    for child in self.processes:
                        child.poll()
                    live = [birth for birth in self.births.values() if same_birth(birth, identity(birth["pid"]))]
                    if not live:
                        break
                    # Include newly observed private descendants, not reused PIDs.
                    for birth in live:
                        signal_birth(birth, number)
                    time.sleep(.05)
            self.report["cleanup"] = not any(same_birth(birth, identity(birth["pid"])) for birth in self.births.values())
        finally:
            if self.master is not None:
                os.close(self.master)
                self.master = None
            for child in self.processes:
                try:
                    child.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    self.report["cleanup"] = False


def run(binary):
    if not binary.is_absolute() or binary.resolve() != binary:
        raise ValueError("Use an absolute non-symlink executable path, not a mutable shim")
    pin = json.loads((HERE / "pin.json").read_text())
    if sha(binary) != pin["executable_sha256"]:
        raise ValueError("Executable hash differs from pin.json")
    with binary.open("rb") as stream:
        if stream.read(4) != b"\x7fELF":
            raise ValueError("Select the actual ELF executable, not its launcher")
    report = {"schema": 1, "target": pin, "artifact_hashes": {name: sha(HERE / name) for name in ARTIFACTS},
              "status": "INCOMPLETE", "production_admission": False,
              "blockers": ["bounded_transport", "terminal_identity", "fresh_complete_family"], "runs": []}
    for mode in ("standalone", "server"):
        with tempfile.TemporaryDirectory(prefix="verij-v2-p0-") as temporary:
            probe = PrivateRun(binary, Path(temporary), mode)
            try:
                probe.launch()
            except (OSError, ValueError, RuntimeError, TimeoutError, subprocess.SubprocessError) as error:
                # Exception bodies can contain upstream/provider content or credentials.
                probe.report["failure"] = type(error).__name__
            finally:
                probe.close()
                report["runs"].append(probe.report)
    if sha(binary) != pin["executable_sha256"]:
        raise ValueError("Executable changed during private run")
    if all(item["status"] == "SMOKE_PASS" and item["cleanup"] for item in report["runs"]):
        report["status"] = "SMOKE_PASS"
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "linux" or not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        parser.error("Linux /proc and pidfd signalling are required")
    def interrupted(_number, _frame):
        raise KeyboardInterrupt
    previous = signal.signal(signal.SIGTERM, interrupted)
    try:
        report = run(args.binary)
    finally:
        signal.signal(signal.SIGTERM, previous)
    # Validate even failed evidence for privacy before writing it anywhere.
    from verify_evidence import validate
    validate(report, require_pass=False)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "production_admission": False, "report": str(args.output)}))
    return 0 if report["status"] == "SMOKE_PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
