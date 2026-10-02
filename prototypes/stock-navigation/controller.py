#!/usr/bin/env python3
"""Application-side stock control: explicit keybind registration, no private IPC.

Plugin epoch is Verij-owned. Native focus observations do not imply host visits.
This prototype never acknowledges Done. Stock actions already queued cannot be
cancelled by the local journal.
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import subprocess
import time
import uuid


def birth(pid):
    try:
        values = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        if values[0] in ("Z", "X"):
            return None
        return {"pid": pid, "start_jiffies": int(values[19]),
                "boot_id": Path("/proc/sys/kernel/random/boot_id").read_text().strip()}
    except (OSError, ValueError, IndexError):
        return None


def atomic(path, value):
    temporary = path.with_name(path.name + "." + uuid.uuid4().hex + ".tmp")
    with temporary.open("x") as output:
        os.chmod(temporary, 0o600)
        json.dump(value, output)
        output.write("\n")
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, path)
    descriptor = os.open(path.parent, os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def config(root):
    return json.loads((root / "manifest.json").read_text())


def environment(root, manifest):
    env = {key: value for key, value in os.environ.items() if not key.startswith(("ZELLIJ", "TMUX", "VERIJ_ZELLIJ"))}
    for kind in ("CONFIG", "DATA", "CACHE", "STATE"):
        env[f"XDG_{kind}_HOME"] = str(root / kind.lower())
    env.update(TERM="xterm-256color", ZELLIJ_SOCKET_DIR=manifest["socket_dir"])
    return env


def action(root, session, *parts):
    manifest = config(root)
    result = subprocess.run([manifest["binary"], "--config", str(root / "inner-base.kdl"), "-s", session,
                             "action", *parts], env=environment(root, manifest), cwd=root,
                            capture_output=True, text=True, timeout=8)
    if result.returncode or result.stdout.startswith(("Session '", "Please specify")):
        raise RuntimeError((parts, result.returncode, result.stdout[:180], result.stderr[:180]))
    return result.stdout.strip()


def binding(root, host):
    manifest = config(root)
    entry = manifest["hosts"][host]
    value = json.loads((root / "stock-state" / f"binding-{entry['nonce']}.json").read_text())
    observation = value["observation"]
    if value["source"] != "focused_plugin_keyboard" or value["host_nonce"] != entry["nonce"]:
        raise RuntimeError("no explicit focused-plugin keyboard registration")
    verified = json.loads((root / f"{host}-verified-binding.json").read_text())
    identity = {key: observation[key] for key in ("server_pid", "plugin_id", "client_id", "epoch")}
    if identity != verified["identity"] or birth(observation["server_pid"]) != verified["server_birth"]:
        raise RuntimeError("server/plugin registration changed; rebind required")
    if birth(entry["client_pid"]) != entry["client_birth"]:
        raise RuntimeError("host Workspace attachment process no longer live")
    snapshot = root / "stock-state" / f"snapshot-{observation['server_pid']}-{observation['plugin_id']}-{observation['client_id']}-{observation['epoch']}.json"
    live = json.loads(snapshot.read_text())
    if time.time_ns() - int(live["time_ns"]) > 5_000_000_000 or not live["ready"]:
        raise RuntimeError("plugin observation is not fresh; rebind/query unavailable")
    if not isinstance(live.get("session"), str) or not live["session"]:
        raise RuntimeError("plugin observation missing current session name")
    return live


def register(root, host):
    manifest = config(root)
    entry = manifest["hosts"][host]
    value = json.loads((root / "stock-state" / f"binding-{entry['nonce']}.json").read_text())
    observation = value["observation"]
    if value["source"] != "focused_plugin_keyboard" or not observation["ready"]:
        raise RuntimeError("plugin not ready or registration is not focused-keyboard-sourced")
    if time.time_ns() - int(value["time_ns"]) > 10_000_000_000:
        raise RuntimeError("registration too old")
    server_birth = birth(observation["server_pid"])
    if not server_birth or birth(entry["client_pid"]) != entry["client_birth"]:
        raise RuntimeError("registration processes are not live")
    identity = {key: observation[key] for key in ("server_pid", "plugin_id", "client_id", "epoch")}
    atomic(root / f"{host}-verified-binding.json", {"identity": identity, "server_birth": server_birth})
    return value


def request(root, host, terminal, operation="focus", target_session=None, override=None):
    manifest = config(root)
    observed = binding(root, host)
    session = observed["session"]
    journal_path = root / f"{host}-sequence.json"
    with (root / f"{host}.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        identity = {key: observed[key] for key in ("server_pid", "plugin_id", "client_id", "epoch")}
        try:
            journal = json.loads(journal_path.read_text())
        except FileNotFoundError:
            journal = {"identity": identity, "sequence": 0}
        if journal["identity"] != identity:
            journal = {"identity": identity, "sequence": 0}
        sequence = 0 if operation == "query" else max(journal["sequence"], observed["sequence"]) + 1
        if operation != "query":
            atomic(journal_path, {"identity": identity, "sequence": sequence})
        payload = {**identity, "request_id": uuid.uuid4().hex, "sequence": sequence, "operation": operation,
                   "terminal_id": terminal, "target_session": target_session}
        if override:
            payload.update(override)
        action(root, session, "pipe", "--name", "verij_stock_navigation", "--", json.dumps(payload))
        path = root / "stock-state" / f"result-{payload['request_id']}.json"
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            try:
                result = json.loads(path.read_text())
                if result["request"] != payload:
                    raise RuntimeError("control result correlation mismatch")
                if birth(observed["server_pid"]) != json.loads((root / f"{host}-verified-binding.json").read_text())["server_birth"]:
                    raise RuntimeError("server birth changed during request")
                if result.get("status") == "inner_focus_observed":
                    actual = result.get("observation", {})
                    if any(actual.get(key) != payload[key] for key in ("server_pid", "plugin_id", "client_id", "epoch")) or actual.get("focused_pane") != f"terminal_{terminal}":
                        raise RuntimeError("focus observation identity/target mismatch")
                # Preserve verified evidence in one journal, prune consumed
                # one-shot files instead of accumulating them indefinitely.
                with (root / "control-results.ndjson").open("a") as output:
                    output.write(json.dumps(result) + "\n")
                path.unlink(missing_ok=True)
                return result
            except FileNotFoundError:
                time.sleep(.05)
            except (json.JSONDecodeError, UnicodeDecodeError) as error:
                raise RuntimeError("malformed control result; focus is unverified") from error
        return {"request": payload, "status": "unavailable_timeout", "acknowledge_done": False,
                "whole_host_verified": False}


def host_focus(root, host, workspace=True):
    manifest = config(root)
    entry = manifest["hosts"][host]
    pane = entry["workspace_pane"] if workspace else entry["sidebar_pane"]
    rows = action(root, entry["session"], "list-clients").splitlines()[1:]
    if len(rows) == 1 and rows[0].split()[1] == f"terminal_{pane}":
        return True
    try:
        action(root, entry["session"], "focus-pane-id", f"terminal_{pane}")
    except RuntimeError as error:
        if "already focused" not in str(error):
            raise
    # Only a single-display outer server is validated in this demo.
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline:
        rows = action(root, entry["session"], "list-clients").splitlines()[1:]
        if len(rows) == 1 and rows[0].split()[1] == f"terminal_{pane}":
            return True
        time.sleep(.05)
    return False


def activate(root, host, terminal, is_current=None):
    result = request(root, host, terminal)
    if is_current is not None and not is_current():
        return {"status": "superseded_locally", "acknowledge_done": False, "whole_host_verified": False}
    result["demo_outer_focus_observed"] = host_focus(root, host) if result["status"] == "inner_focus_observed" else False
    # Never promote a demo focus sample into a production completion acknowledgement.
    result["acknowledge_done"] = False
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--host", choices=("a", "b"), default="a")
    parser.add_argument("--pane", type=int, default=0)
    parser.add_argument("--operation", choices=("focus", "query", "register", "sidebar"), default="focus")
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    if args.operation == "register":
        result = register(root, args.host)
    elif args.operation == "sidebar":
        result = {"sidebar_focus_observed": host_focus(root, args.host, workspace=False)}
    elif args.operation == "query":
        result = request(root, args.host, args.pane, "query")
    else:
        result = activate(root, args.host, args.pane)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
