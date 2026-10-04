#!/usr/bin/env python3
"""Operate or clean only a manifest-qualified private H1 review instance."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import time

from verify import environment


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("focus", "query", "inventory", "inspect", "clean"))
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--host", choices=("a", "b"), default="a")
    parser.add_argument("--pane", type=int, default=0)
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    manifest = json.loads((root / "review-manifest.json").read_text())
    assert root.name.startswith("vj-h1-") and manifest["root"] == str(root)
    sockets = Path(manifest["socket_dir"])
    env = environment(root, sockets)
    env["ZELLIJ_CONFIG_FILE"] = str(root / "inner.kdl")
    cli = manifest["cli"]
    if args.operation == "clean":
        for session in manifest["sessions"]:
            subprocess.run([manifest["binary"], "kill-session", session], env=env, cwd=root, capture_output=True, timeout=8)
        subprocess.run(["tmux", "-f", "/dev/null", "-S", manifest["tmux_socket"], "kill-server"], capture_output=True, timeout=8)
        shutil.rmtree(sockets, ignore_errors=True)
        manifest["cleaned_at_ms"] = time.time_ns() // 1_000_000
        (root / "review-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        print(json.dumps({"cleaned": str(root), "private_socket_removed": not sockets.exists()}))
        return
    if not sockets.exists():
        raise RuntimeError("this review instance was cleaned")
    if args.operation == "inventory":
        command = [cli, "inventory", "--session", manifest["inner_session"]]
    elif args.operation == "inspect":
        command = [cli, "agent", "inspect", "--instance", manifest["synthetic_instance"]]
    elif args.operation == "query":
        command = [cli, "navigate", "query", "--host", args.host]
    else:
        command = [cli, "navigate", "focus", "--host", args.host, "--pane", str(args.pane)]
    result = subprocess.run(command, cwd=root, env=env, text=True, timeout=20)
    raise SystemExit(result.returncode)


if __name__ == "__main__":
    main()
