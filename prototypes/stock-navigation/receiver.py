#!/usr/bin/env python3
"""Visible disposable receiver: keyboard tokens, not model or production agent data."""
from pathlib import Path
import sys


root, label = Path(sys.argv[1]), sys.argv[2]
print(f"STOCK DEMO TARGET {label}\nType a unique word and Enter. It is retained only in this private demo.\n", flush=True)
for line in sys.stdin:
    with (root / f"input-{label}").open("a") as output:
        output.write(line)
    print(f"{label} received: {line.strip()}", flush=True)
