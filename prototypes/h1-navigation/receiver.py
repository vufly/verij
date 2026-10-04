#!/usr/bin/env python3
"""Synthetic keyboard target and redirected-child ownership negative control."""
import json
import os
from pathlib import Path
import subprocess
import sys

root, label = Path(sys.argv[1]), sys.argv[2]
print(f"STOCK DEMO TARGET {label}\nSynthetic H1 receiver; enter a unique test word.\n", flush=True)
for line in sys.stdin:
    with (root / f"input-{label}").open("a") as output:
        output.write(line)
    if line.strip() == "h1-spawn-redirected":
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"],
                                 stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        (root / "redirected-child.json").write_text(json.dumps({"pid": child.pid, "parent": os.getpid()}))
    print(f"{label} received: {line.strip()}", flush=True)
