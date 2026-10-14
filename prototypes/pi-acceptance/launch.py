#!/usr/bin/env python3
"""Exec real Pi in its stock Zellij pane; private test metadata only."""
import json
import os
from pathlib import Path
import sys

config = json.loads(Path(sys.argv[1]).read_text())
root, repo = Path(config["root"]), Path(config["repo"])
pane = os.environ["ZELLIJ_PANE_ID"]
os.environ["VJ_PI_TRACE"] = str(root / f"trace-{pane}.ndjson")
os.environ["VJ_PI_SESSION_CONTROL"] = str(root / f"session-{pane}")
os.environ["VJ_P4_ROOT"] = str(root)
os.environ["VJ_P4_PYTHON"] = sys.executable
os.environ["VJ_P4_DETACHED"] = str(repo / "prototypes/pi-acceptance/detached.py")
args = [*config["command"], "--offline", "--no-builtin-tools", "--no-skills", "--no-prompt-templates",
        "--no-context-files", "--no-mcp", "--no-approve", "--thinking", "off",
        "--extension", str(repo / "prototypes/pi-runtime/observer.ts"),
        "--extension", str(repo / "prototypes/pi-runtime/fixture.ts"),
        "--extension", str(repo / "prototypes/pi-acceptance/fixture.ts")]
if len(sys.argv) > 2 and sys.argv[2] == "disabled":
    args = [*config["command"], "--offline", "--no-extensions", "--no-builtin-tools", "--no-skills",
            "--no-context-files", "--no-mcp", "--no-approve", "--extension", str(repo / "prototypes/pi-runtime/observer.ts")]
    os.environ["VERIJ_RESOURCE_METRICS_DIR"] = str(root / "mode-discovery-metrics")
os.execvpe(args[0], args, os.environ)
