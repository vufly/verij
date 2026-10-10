#!/usr/bin/env python3
"""Installed Magy APIs, invoked inside a real Zellij launcher pane."""
import json
import os
from pathlib import Path
import sys

from magy.headful import start_headful_run
from magy.runs import start_run
from magy.watches import start_watch_run, cancel_watch_run

action, root_string = sys.argv[1:3]
root = Path(root_string)
os.environ.update(json.loads((root / "magy-env.json").read_text()))
workspace = str(root / "workspace")
options = {"workspace":workspace,"profile":"h4-fixture","model":"gemini-3.8-flash-low","auto_approval":False}
no_tool = "Reply exactly OK. Do not use tools, skills, files or browser."
try:
    if action == "pane":
        result = start_headful_run(no_tool, **options).__dict__
    elif action == "watch":
        result = start_watch_run("Without using any tools, explain binary search in about 400 words.", **options).to_dict()
    elif action == "cancel-watch":
        result = start_watch_run("Without using any tools, write a detailed 6000 word tutorial on sorting algorithms.", **options).to_dict()
    elif action == "cancel":
        data = json.loads((root / "driver-cancel-watch.json").read_text())
        result = cancel_watch_run(data["watch_id"]).to_dict()
    elif action == "run":
        result = start_run(no_tool, **options, timeout=45).to_dict()
    else:
        raise ValueError(action)
except Exception as error:
    result = {"error_type":type(error).__name__,"error":str(error)[:400]}
(root / f"driver-{action}.json").write_text(json.dumps(result))
