#!/usr/bin/env python3
"""Bounded metadata reporting; non-gating Codex lifecycle hook callback adapter."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

LIMIT = 262144

SUPPORTED_EVENTS = (
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "Stop",
    "Interrupt",
    "SessionEnd",
)

SESSION_START_SOURCES = ("startup", "resume", "clear", "compact")


def is_clean_string(s, max_len=256):
    if not isinstance(s, str) or len(s) > max_len or not s:
        return False
    return not any(ord(c) < 32 or ord(c) == 127 for c in s)


def _read_stat(pid):
    with open(f"/proc/{pid}/stat", "r", encoding="utf-8") as f:
        return f.read()


def _read_cmdline(pid):
    with open(f"/proc/{pid}/cmdline", "rb") as f:
        data = f.read(65537)
        if len(data) > 65536:
            raise ValueError(f"cmdline too large for pid {pid}")
        return data


def is_interactive_codex(pid, stat):
    try:
        # Check controlling tty nr from /proc/{pid}/stat (index 4 after comm split)
        tty_nr = int(stat[4])
        if tty_nr == 0:
            return False

        # Verify stdin/stdout point to terminal device
        stdin_target = os.readlink(f"/proc/{pid}/fd/0")
        stdout_target = os.readlink(f"/proc/{pid}/fd/1")
        if not (stdin_target.startswith("/dev/pts/") or stdin_target.startswith("/dev/tty")):
            return False
        if not (stdout_target.startswith("/dev/pts/") or stdout_target.startswith("/dev/tty")):
            return False
    except OSError:
        return False

    try:
        cmdline = _read_cmdline(pid)
        args = [arg.decode("utf-8", "replace") for arg in cmdline.split(b"\x00") if arg]
        # Exclude headless / server subcommands
        excluded = {"app-server", "exec", "review", "serve", "daemon"}
        if any(arg in excluded for arg in args):
            return False
    except OSError:
        return False

    return True


def agent_parent():
    pid = os.getppid()
    for _ in range(128):
        stat = _read_stat(pid).rsplit(")", 1)[1].split()
        executable = Path(os.readlink(f"/proc/{pid}/exe")).name.removesuffix(" (deleted)")
        if (executable == "codex" or executable.startswith("codex.")) and is_interactive_codex(pid, stat):
            return pid, int(stat[19])
        pid = int(stat[1])
        if pid <= 1:
            break
    raise ValueError("no pane-local interactive Codex ancestor")


def snapshot(value, event, version, sample):
    if value.get("hook_event_name") != event:
        raise ValueError(f"mismatched hook event: expected {event}, got {value.get('hook_event_name')}")
    result = {
        "schema_version": 1,
        "version": version,
        "event": event,
        "sample": sample,
    }
    # Whitelist clean string identifiers only
    for key in ("session_id", "turn_id", "tool_name", "tool_use_id"):
        if key in value and value[key] is not None:
            if not is_clean_string(value[key]):
                raise ValueError(f"invalid identifier for {key}")
            result[key] = value[key]

    if event == "SessionStart":
        source = value.get("source")
        if source not in SESSION_START_SOURCES:
            raise ValueError(f"invalid or missing SessionStart source: {source}")
        result["source"] = source

    if "stop_hook_active" in value and isinstance(value["stop_hook_active"], bool):
        result["stop_hook_active"] = value["stop_hook_active"]

    if "error_present" in value and isinstance(value["error_present"], bool):
        result["error_present"] = value["error_present"]

    return result


def report(config, value, event, sample):
    if config.get("enabled") is False:
        return
    # Pane-less runs and watches use separate runner observer, never inherited UI ownership.
    if os.environ.get("MAGY_WATCH_ID"):
        return
    pane = os.environ.get("ZELLIJ_PANE_ID", "").removeprefix("terminal_")
    if not pane.isdigit():
        return
    if value.get("hook_event_name") != event:
        return
    pid, birth = agent_parent()
    # Executable ancestry and reporter-side live foreground ownership establish
    # identity. Installed version selects the hook contract, not a binary inode.
    version = config["version"]
    payload = snapshot(value, event, version, sample)
    env = dict(os.environ)
    env.setdefault("VERIJ_AGENT_STATE_DIR", config["runtime"])
    env.setdefault("VERIJ_STATES_DIR", config["states"])
    try:
        subprocess.run(
            [
                config["reporter"],
                "agent",
                "codex",
                "--pane",
                pane,
                "--pid",
                str(pid),
                "--birth",
                str(birth),
            ],
            input=json.dumps(payload).encode(),
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            env=env,
            timeout=1.5,
            check=False,
        )
    except (subprocess.SubprocessError, OSError):
        pass


def main():
    try:
        # Validate arguments and event BEFORE touching stdin
        if len(sys.argv) != 3:
            print("{}")
            return 0
        event = sys.argv[1]
        file = sys.argv[2]
        if event not in SUPPORTED_EVENTS:
            print("{}")
            return 0

        # Validate config file before reading stdin
        config_path = Path(file)
        if not config_path.is_file():
            print("{}")
            return 0
        config = json.loads(config_path.read_text())
        if not isinstance(config, dict) or "version" not in config:
            print("{}")
            return 0

        sample = time.monotonic_ns()
        raw = sys.stdin.buffer.read(LIMIT + 1)
        if len(raw) <= LIMIT:
            value = json.loads(raw)
            if isinstance(value, dict):
                report(config, value, event, sample)
    except Exception:
        pass  # Observation must never gate execution.

    print("{}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
