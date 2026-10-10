#!/usr/bin/env python3
"""Fixture metadata recorder wrapping the actual installed production callback."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import stat

event, config_file = sys.argv[1:3]
raw = sys.stdin.buffer.read(262145)
config = json.loads(Path(config_file).read_text())
callback_file = Path(config_file).with_name("callback.py")
spec = importlib.util.spec_from_file_location("production_callback", callback_file)
assert spec and spec.loader
callback = importlib.util.module_from_spec(spec)
spec.loader.exec_module(callback)
metadata = callback.snapshot(json.loads(raw), event, config["version"], time.monotonic_ns())
try:
    pid, birth = callback.agent_parent()
    metadata.update(pid=pid, birth=birth, pane=os.environ.get("ZELLIJ_PANE_ID"))
    descriptors = {}
    for file in Path(f"/proc/{pid}/fd").iterdir():
        try:
            info = file.stat()
            if int(file.name) <= 2 or stat.S_ISCHR(info.st_mode):
                descriptors[file.name] = {"terminal":stat.S_ISCHR(info.st_mode),"rdev":info.st_rdev}
        except OSError:
            pass
    metadata["descriptor_metadata"] = descriptors
except (OSError, ValueError):
    pass
result = subprocess.run([sys.executable, str(callback_file), event, config_file], input=raw, capture_output=True)
metadata["neutral_stdout"] = (not result.stdout if event == "ui" and not config.get("previous_command")
                             else result.stdout == b"OLD_STATUS_MARKER" if event == "ui"
                             else json.loads(result.stdout) == ({"decision":""} if event == "Stop" else {}))
metadata["callback_exit"] = result.returncode
with Path(os.environ["VJ_H4_EVENTS"]).open("a") as output:
    output.write(json.dumps(metadata) + "\n")
sys.stdout.buffer.write(result.stdout)
sys.stderr.buffer.write(result.stderr)
raise SystemExit(result.returncode)
