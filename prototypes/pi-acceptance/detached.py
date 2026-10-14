#!/usr/bin/env python3
"""Launch real Pi with inherited pane hints but an unrelated controlling PTY."""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import sys
import termios
import time

root = Path(sys.argv[1])
config = json.loads((root / "launch.json").read_text())
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 140, 0, 0))
env = dict(os.environ, VJ_PI_TRACE=str(root / "detached-trace.ndjson"))
args = [*config["command"], "--offline", "--no-builtin-tools", "--no-skills", "--no-prompt-templates",
        "--no-context-files", "--no-mcp", "--no-approve", "--thinking", "off",
        "--extension", str(Path(config["repo"]) / "prototypes/pi-runtime/observer.ts")]
helper = "import fcntl,os,sys,termios;fcntl.ioctl(0,termios.TIOCSCTTY,0);os.execvpe(sys.argv[1],sys.argv[1:],os.environ)"
child = subprocess.Popen([sys.executable, "-c", helper, *args], cwd=root, env=env,
                         stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
os.close(slave)
try:
    (root / "detached.json").write_text(json.dumps({"pid": child.pid, "candidate_pane": int(env["ZELLIJ_PANE_ID"])}))
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline and child.poll() is None:
        if select.select([master], [], [], .1)[0]:
            try:
                data = os.read(master, 65536)
                if b"\x1b[6n" in data:
                    os.write(master, b"\x1b[1;1R")
            except OSError:
                break
finally:
    if child.poll() is None:
        os.killpg(child.pid, signal.SIGTERM)
        try:
            child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait(timeout=3)
    os.close(master)
    (root / "detached-done").write_text("done")
