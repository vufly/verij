#!/usr/bin/env python3
"""Host-owned stock attachment wrapper; publishes OS identity, never client ID guesses."""
import json
import os
from pathlib import Path
import sys


root, host = Path(sys.argv[1]), sys.argv[2]
manifest = json.loads((root / "manifest.json").read_text())
marker = {"host_session": os.environ["ZELLIJ_SESSION_NAME"], "host_pane": os.environ["ZELLIJ_PANE_ID"],
          "client_pid": os.getpid()}
(root / f"{host}-workspace.json").write_text(json.dumps(marker))
env = {key: value for key, value in os.environ.items() if not key.startswith("ZELLIJ")}
env["ZELLIJ_SOCKET_DIR"] = manifest["socket_dir"]
command = [manifest["binary"], "--config", manifest["hosts"][host]["client_config"],
           "--layout", str(root / "inner-layout.kdl"), "attach", "-c", "shared", "--",
           sys.executable, str(Path(__file__).with_name("receiver.py")), str(root), "shared-0"]
os.execvpe(command[0], command, env)
