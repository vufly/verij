#!/usr/bin/env python3
"""Restart only the generated fixture sidebar; retain each child's native birth."""
import importlib
import os
from pathlib import Path
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "stock-navigation"))
controller = importlib.import_module("controller")


def main():
    cli, root, host = sys.argv[1:]
    generation = 0
    while True:
        env=dict(os.environ,VERIJ_UI_READY_FILE=str(Path(root) / f"ui-ready-{host}.json"))
        child = subprocess.Popen([cli, "ui"],env=env)
        identity = controller.birth(child.pid)
        if identity is None:
            child.wait()
            raise RuntimeError("sidebar exited before its native birth could be recorded")
        generation += 1
        controller.atomic(Path(root) / f"sidebar-{host}.json", {
            "generation": generation, "process": identity,
        })
        child.wait()
        time.sleep(.2)


if __name__ == "__main__":
    main()
