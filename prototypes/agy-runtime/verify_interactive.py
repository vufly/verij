#!/usr/bin/env python3
"""Test isolated HOME configuration and real interactive Agy callbacks.

Copies one authenticated token into a private mode-0700 scratch home; never
prints credentials or points mutable scratch files back at the user profile.
No model invocation is needed for the startup positive control.
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import shlex
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time


def records(path):
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines(keepends=True) if line.endswith("\n")]


def process_birth(pid):
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        return None if fields[0] in ("Z", "X") else (pid, int(fields[19]))
    except (OSError, IndexError, ValueError):
        return None


def cleanup_private(root):
    assert root.name.startswith("vj-agy-") and root.is_dir() and not root.is_symlink(), root
    for case in root.iterdir():
        if case.is_dir():
            for name in ("home", "config", "cache", "data", "state"):
                shutil.rmtree(case / name, ignore_errors=True)
    return all(not (case / name).exists() for case in root.iterdir() if case.is_dir()
               for name in ("home", "config", "cache", "data", "state"))


def startup_hints(plain):
    # Only pre-turn UI diagnostics in this private no-tool probe. Redact account,
    # path, link and opaque identifier values; do not retain terminal transcript.
    text = plain.decode("utf8", errors="replace")
    text = re.sub(r"https?://\S+", "[URL]", text)
    text = re.sub(r"[^\s<>]+@[^\s<>]+", "[EMAIL]", text)
    text = re.sub(r"/[A-Za-z0-9_.~/-]+", "[PATH]", text)
    text = re.sub(r"\b[0-9a-fA-F-]{36}\b|\b[A-Za-z0-9_-]{40,}\b", "[OPAQUE]", text)
    hints = []
    for match in re.finditer(r"(?i).{0,60}(?:press enter|allow|confirm|trust|no,|do not|decline|previous|next|toggle|proceed).{0,120}", text):
        hint = match.group().strip()
        if hint not in hints:
            hints.append(hint)
    return hints[-12:]


def private_flags(home):
    result = {}
    def collect(value, prefix=""):
        if not isinstance(value, dict):
            return
        for name, item in value.items():
            field = prefix + name
            if isinstance(item, bool) and any(word in field.lower() for word in ("telemetry", "share", "feedback", "training", "consent")):
                result[field] = item
            if isinstance(item, dict):
                collect(item, field + ".")
    for path in (home / ".gemini").rglob("settings.json"):
        collect(json.loads(path.read_text()), str(path.relative_to(home)) + ":")
    return result


def prompt_diagnostics(plain):
    words = ("invalid decision", "empty decision", "hook error", "hook failed", "hook failure", "pretooluse", "invalid json")
    text = plain.decode("utf8", errors="replace").lower()
    return {word: word in text for word in words}


def exact_native_once_modal(screen, expected):
    lines = screen.decode("utf8", errors="replace").splitlines()
    header = any(re.fullmatch(r"[\s●]*bash\(" + re.escape(expected) + r"\)(?: \(ctrl\+o to expand\))?\s*", line) for line in lines)
    commands = [i for i, line in enumerate(lines) if line.strip(" \t│┃>$") == expected]
    once = [i for i, line in enumerate(lines) if re.fullmatch(r"[\s>❯]*1[.)]\s*yes, run command\s*", line)]
    labels = {"", "run this command?"}
    return header and any(command < option and all(row.strip(" \t│┃─") in labels for row in lines[command + 1:option])
                          for command in commands for option in once)


def modal_shape(screen, expected):
    rows = screen.decode("utf8", errors="replace").splitlines()
    command_indices = [i for i, row in enumerate(rows) if row.strip(" \t│┃>$") == expected]
    option_indices = [i for i, row in enumerate(rows) if re.fullmatch(r"[\s>❯]*1[.)]\s*yes, run command\s*", row)]
    return {"exact_command_row": any(row.strip(" \t│┃>$") == expected for row in rows),
            "exact_once_row": any(re.fullmatch(r"[\s>❯]*1[.)]\s*yes, run command\s*", row) for row in rows),
            "exact_summary": any(re.fullmatch(r"[\s●]*bash\(" + re.escape(expected) + r"\)(?: \(ctrl\+o to expand\))?\s*", row) for row in rows),
            "intermediate_row_types": [[("blank" if not row.strip() else "native_confirmation_label" if row.strip() == "run this command?" else "cwd" if "cwd" in row.lower() else "other")
                                        for row in rows[command + 1:option]] for command in command_indices for option in option_indices if command < option],
            "short_static_question_labels": [row.strip() for command in command_indices for option in option_indices if command < option
                                             for row in rows[command + 1:option] if re.fullmatch(r"[a-z ]{1,60}\?", row.strip())],
            "command_row_wrappers": [{"prefix": row[:row.index(expected)].strip(),
                                      "suffix": row[row.index(expected) + len(expected):].strip()}
                                     for row in rows if expected in row and len(row) <= len(expected) + 35],
            "once_row_wrappers": [row.strip() for row in rows if "1. yes" in row and len(row) < 35]}


def run_case(root, label, token, wrapped, scenario="startup", timeout=25, dismiss_welcome=False, decline_telemetry=False, native_tmux=False, pretool_observer=False):
    directory = root / label
    directory.mkdir(mode=0o700)
    home = directory / "home"
    app = home / ".gemini" / "antigravity-cli"
    app.mkdir(mode=0o700, parents=True)
    # Private copy, not a writable symlink into the live user's OAuth store.
    destination = app / "antigravity-oauth-token"
    shutil.copyfile(token, destination)
    destination.chmod(0o600)
    events = directory / "statusline.ndjson"
    lifecycle = directory / "lifecycle.ndjson"
    command = shlex.join([sys.executable, str(Path(__file__).with_name("statusline.py").resolve())])
    settings = {"statusLine": {"type": "command", "command": command, "enabled": True, "padding": 0, "stack_with_default": True},
                "notifications": False, "showFeedbackSurvey": False,
                "trustedWorkspaces": [str(directory)]}
    (app / "settings.json").write_text(json.dumps(settings))
    (app / "settings.json").chmod(0o600)
    (directory / "statusline-config.json").write_text(json.dumps(settings))
    prompts = {
        "no-tool": "Reply exactly OK. Do not use tools, skills, files or browser.",
        "permission": "Run exactly this harmless shell command using run_command: printf VJ_PERMISSION. Do not use any other tool.",
        "permission-reply": "Run exactly this harmless shell command using run_command: printf VJ_PERMISSION. Do not use any other tool. After it completes, reply OK.",
        "background": "Use run_command once with CommandLine exactly 'sleep 8; printf VJ_BACKGROUND' and WaitMsBeforeAsync=1. After the tool returns a background task handle, reply OK without waiting. Do not use manage_task, wait, other tools or commands.",
        "question": "Use ask_question to ask me to choose Alpha or Beta. Wait for my answer. Do not use other tools.",
        "cancel": "Without using tools, write a detailed explanation of sorting algorithms in about 600 words.",
        "deadline": "Without using any tools, write an extensive detailed tutorial about sorting algorithms in at least 6000 words.",
    }
    if scenario != "startup":
        custom = directory / ".agents"
        custom.mkdir(mode=0o700)
        hooks = {phase: [{"type": "command", "command": shlex.join([
            sys.executable, str(Path(__file__).with_name("lifecycle.py").resolve()), phase]), "timeout": 5}]
                 for phase in ("PreInvocation", "PostInvocation", "Stop")}
        hooks["PostToolUse"] = [{"matcher": "*", "hooks": [{"type": "command", "command": shlex.join([
            sys.executable, str(Path(__file__).with_name("lifecycle.py").resolve()), "PostToolUse"]), "timeout": 5}]}]
        if pretool_observer:
            hooks["PreToolUse"] = [{"matcher": "run_command|ask_question", "hooks": [{"type": "command", "command": shlex.join([
                sys.executable, str(Path(__file__).with_name("lifecycle.py").resolve()), "PreToolUse"]), "timeout": 5}]}]
        (custom / "hooks.json").write_text(json.dumps({"verij-neutral-probe": hooks}))
    env = {k: v for k, v in os.environ.items() if not k.startswith(("ZELLIJ", "TMUX", "MAGY", "ANTIGRAVITY", "AGY"))}
    env.update(HOME=str(home), TERM="xterm-256color", AGY_CLI_DISABLE_AUTO_UPDATE="true",
               VJ_STATUSLINE_EVENTS=str(events), VJ_STATUSLINE_WRAP=str(int(wrapped)),
               VJ_LIFECYCLE_EVENTS=str(lifecycle))
    for kind in ("CONFIG", "CACHE", "DATA", "STATE"):
        env[f"XDG_{kind}_HOME"] = str(directory / kind.lower())
    command = ["agy"]
    if scenario == "deadline":
        command += ["--model", "Gemini 3.8 Flash (High)", "--print", prompts[scenario],
                    "--output-format", "stream-json", "--print-timeout", "2s"]
        process = subprocess.Popen(command, cwd=directory, env=env, stdout=subprocess.PIPE,
                                   stderr=subprocess.DEVNULL, start_new_session=True)
        birth = process_birth(process.pid)
        try:
            output, _ = process.communicate(timeout=timeout)
            stream = []
            for line in output.splitlines():
                try:
                    stream.append(json.loads(line))
                except json.JSONDecodeError:
                    pass
            final = next((item.get("result", {}) for item in reversed(stream) if item.get("event") == "result"), {})
            hooks = records(lifecycle)
            executed = any(item["phase"] == "PreInvocation" for item in hooks)
            passed = executed and final.get("status") == "ERROR" and process.returncode != 0
            outcome = "PASS" if passed else "BLOCKED"
            if executed and final.get("status") == "SUCCESS" and not any(item["phase"] == "Stop" for item in hooks):
                outcome = "LIMITATION"
            return {"case": label, "scenario": scenario, "terminal_transport": "headless stream-json",
                    "cli_pid": process.pid, "cli_start_jiffies": birth[1] if birth else None,
                    "callbacks": records(events), "lifecycle": hooks,
                    "stream_event_types": [item.get("event") for item in stream],
                    "final_result_status": final.get("status"), "final_error_present": bool(final.get("error")),
                    "final_response_characters": len(final.get("response", "")), "final_num_turns": final.get("num_turns"),
                    "cli_exit_code": process.returncode, "permission_autoapproval": False,
                    "empty_pretool_observer_installed": pretool_observer,
                    "trusted_workspace_is_generated_probe_only": True,
                    "status": outcome}
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=3)
    if scenario != "startup":
        command += ["--model", "Gemini 3.8 Flash (High)", "--prompt-interactive", prompts[scenario]]
    process = None
    master = None
    tmux_directory = None
    native_pid, native_birth = None, None
    def mux(*parts):
        assert tmux_directory
        return subprocess.run(["tmux", "-f", "/dev/null", "-S", str(Path(tmux_directory.name) / "socket"), *parts],
                              env=env, cwd=directory, capture_output=True, timeout=5)
    if native_tmux:
        tmux_directory = tempfile.TemporaryDirectory(prefix="vj-agy-term-")
        result = mux("new-session", "-d", "-s", "probe", "-x", "150", "-y", "45", "-c", str(directory),
                     "exec " + shlex.join(command))
        result.check_returncode()
        native_pid = int(mux("display-message", "-p", "-t", "probe", "#{pane_pid}").stdout)
        native_birth = process_birth(native_pid)
        assert native_birth and os.getpgid(native_pid) == native_pid
        mux("set-option", "-w", "-t", "probe", "remain-on-exit", "on").check_returncode()
    else:
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 150, 0, 0))
        process = subprocess.Popen(command, cwd=directory, env=env, stdin=slave, stdout=slave, stderr=slave,
                                   start_new_session=True, preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0))
        os.close(slave)
    def send(data):
        if native_tmux:
            if data == b"\r":
                mux("send-keys", "-t", "probe", "Enter").check_returncode()
            elif data == b" ":
                mux("send-keys", "-t", "probe", "Space").check_returncode()
            elif data == b"\t":
                mux("send-keys", "-t", "probe", "Tab").check_returncode()
            elif data == b"\x1b[C":
                mux("send-keys", "-t", "probe", "Right").check_returncode()
            elif data == b"\x03":
                mux("send-keys", "-t", "probe", "C-c").check_returncode()
            else:
                mux("send-keys", "-t", "probe", "-H", *[f"{byte:02x}" for byte in data]).check_returncode()
        else:
            assert master is not None
            os.write(master, data)
    output = bytearray()
    start = time.monotonic()
    answered_queries = {}
    stopped_at = None
    welcome_dismissed = False
    telemetry_decline_attempted = False
    unchecked_confirmed = False
    cancellation_sent = False
    question_ui_seen = False
    question_seen_at = None
    pending_seen_at = None
    idle_after_cancel_at = None
    approval_sent = False
    approval_at = None
    approval_choices = set()
    permission_modal_rows = []
    last_modal_shape = None
    try:
        while time.monotonic() - start < timeout and (process is None or process.poll() is None):
            current_screen = None
            if native_tmux:
                screen = mux("capture-pane", "-p", "-t", "probe")
                screen.check_returncode()
                current_screen = screen.stdout.lower()
                if screen.stdout != output[-len(screen.stdout):]:
                    output.extend(screen.stdout)
                time.sleep(.1)
            elif master is not None and select.select([master], [], [], .1)[0]:
                try:
                    output.extend(os.read(master, 65536))
                except OSError:
                    break
            # Answer terminal introspection, not agent prompts or permissions.
            for query, response in (() if native_tmux else ((b"\x1b[6n", b"\x1b[1;1R"), (b"\x1b[c", b"\x1b[?1;2c"),
                                    (b"\x1b]11;?", b"\x1b]11;rgb:0000/0000/0000\x1b\\"))):
                pending = output.count(query) - answered_queries.get(query, 0)
                for _ in range(pending):
                    send(response)
                answered_queries[query] = output.count(query)
            callbacks = records(events)
            if native_tmux and current_screen is not None:
                for option in (b"allow once", b"always allow", b"run once", b"run command", b"reject", b"deny", b"yes", b"no"):
                    if option in current_screen:
                        approval_choices.add(option.decode())
            tool_events = [e for e in records(lifecycle) if e["phase"] == "PreToolUse"]
            permission_pending = any(e.get("tool_confirmation_pending") is True for e in callbacks)
            if native_tmux and current_screen is not None and permission_pending:
                for line in current_screen.decode("utf8", errors="replace").splitlines():
                    if any(word in line for word in ("run]", "reject", "always", "allow once", "ctrl+", "run once", "proceed")):
                        projected = startup_hints(line.encode())
                        permission_modal_rows.extend(row for row in projected if row not in permission_modal_rows)
            if scenario in ("permission-reply", "background") and not approval_sent and native_tmux:
                expected = "printf vj_permission" if scenario == "permission-reply" else "sleep 8; printf vj_background"
                if current_screen is not None and permission_pending:
                    last_modal_shape = modal_shape(current_screen, expected)
                exact = current_screen is not None and exact_native_once_modal(current_screen, expected)
                pending = any(e.get("tool_confirmation_pending") is True for e in callbacks)
                if exact and pending:
                    send(b"1")
                    time.sleep(.2)
                    still = mux("capture-pane", "-p", "-t", "probe").stdout.lower()
                    if exact_native_once_modal(still, expected):
                        send(b"\r")
                    approval_sent = True
                    approval_at = time.monotonic()
            if native_tmux and current_screen is not None:
                lines = current_screen.decode("utf8", errors="replace").splitlines()
                alpha = any(re.fullmatch(r"[\s>❯•\[\]0-9.()|─]*alpha\s*", line) for line in lines)
                beta = any(re.fullmatch(r"[\s>❯•\[\]0-9.()|─]*beta\s*", line) for line in lines)
                question_ui_seen |= alpha and beta and (b"enter" in current_screen or b"submit" in current_screen)
            if (scenario == "cancel" and not cancellation_sent
                    and any(item.get("agent_state") in ("working", "thinking") for item in callbacks)):
                send(b"\x03")
                cancellation_sent = True
            plain_now = current_screen if current_screen is not None else re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", bytes(output)).lower()
            if (dismiss_welcome and not welcome_dismissed and scenario != "startup"
                    and b"welcome" in plain_now and b"press enter" in plain_now
                    and time.monotonic() - start >= 1
                    and not any(word in plain_now for word in (b"permission", b"allow", b"trust", b"terms", b"privacy"))):
                # Explicitly selected disposable welcome acknowledgement only;
                # no tool/input permission prompt has been observed or accepted.
                send(b"\r")
                welcome_dismissed = True
            if (decline_telemetry and not telemetry_decline_attempted and welcome_dismissed
                    and b"help improve antigravity" in plain_now
                    and b"yes, i agree" in plain_now and b"no" in plain_now):
                # Uncheck the known first-run data-sharing form, not a tool
                # permission. Confirmation occurs only from a native screen.
                send(b" ")  # Uncheck the visible opt-in checkbox.
                time.sleep(.2)
                if not native_tmux:
                    send(b"\r")
                telemetry_decline_attempted = True
            if (native_tmux and telemetry_decline_attempted and not unchecked_confirmed
                    and b"> [ ] yes, i agree to help improve antigravity" in plain_now):
                # Native screen proves opt-in is unchecked before moving focus
                # to the form's Confirm control; no permission prompt accepted.
                send(b"\t")
                time.sleep(.2)
                send(b"\x1b[C")
                time.sleep(.2)
                confirmation = mux("capture-pane", "-p", "-t", "probe").stdout.lower()
                if b"[ ] yes, i agree" not in confirmation or not re.search(rb">\s+done\b", confirmation):
                    raise RuntimeError("data-sharing checkbox is not visibly unchecked")
                send(b"\r")
                unchecked_confirmed = True
            stops = [event for event in records(lifecycle) if event["phase"] == "Stop"]
            eligible_stops = stops if scenario != "background" else [e for e in stops if e.get("fullyIdle") is True]
            if eligible_stops and stopped_at is None:
                stopped_at = time.monotonic()
            background_elapsed = scenario != "background" or (approval_at is not None and time.monotonic() - approval_at >= 11)
            if scenario != "startup" and stopped_at is not None and time.monotonic() - stopped_at >= 2 and background_elapsed:
                break
            if scenario == "permission" and any(item.get("tool_confirmation_pending") is True for item in callbacks):
                pending_seen_at = pending_seen_at or time.monotonic()
                if time.monotonic() - pending_seen_at >= 2:
                    break
            if scenario == "question" and question_ui_seen:
                question_seen_at = question_seen_at or time.monotonic()
                if time.monotonic() - question_seen_at >= 3:
                    break
            if scenario == "cancel" and cancellation_sent and any(item.get("agent_state") == "idle" for item in callbacks):
                idle_after_cancel_at = idle_after_cancel_at or time.monotonic()
                if time.monotonic() - idle_after_cancel_at >= 5:
                    break
            if scenario == "startup" and callbacks and any(item.get("agent_state") == "idle" for item in callbacks) and time.monotonic() - start >= 5:
                time.sleep(.5)
                if master is not None and select.select([master], [], [], .2)[0]:
                    output.extend(os.read(master, 65536))
                break
        callbacks = records(events)
        hooks = records(lifecycle)
        distinct = {}
        for item in callbacks:
            shape = {k: v for k, v in item.items() if k not in ("time_ns", "callback_pid")}
            distinct.setdefault(json.dumps(shape, sort_keys=True), item)
        plain = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", bytes(output))
        # Terminal transcript can include identity/UI text; save classification only.
        flags = {name: phrase.lower() in plain.lower() for name, phrase in {
            "authentication_prompt": b"sign in", "trust_prompt": b"trust", "welcome": b"welcome",
            "folder_selection": b"select a", "press_enter": b"press enter", "confirm_permission": b"allow",
            "configured_marker_rendered": b"OLD_STATUS_MARKER" if wrapped else b"PROBE_STATUS_MARKER"}.items()}
        hints = startup_hints(plain) if scenario == "no-tool" and not hooks else []
        passed = bool(callbacks and flags["configured_marker_rendered"])
        if scenario == "no-tool":
            passed = passed and any(e["phase"] == "PreInvocation" for e in hooks) and any(
                e["phase"] == "Stop" and e.get("fullyIdle") is True and not e["error_present"] for e in hooks)
        elif scenario == "cancel":
            passed = passed and cancellation_sent and any(e["phase"] == "Stop" and e.get("terminationReason") in (
                "USER_CANCELLED", "CANCELLED", "INTERRUPTED", "STOP_REQUESTED") for e in hooks)
        elif scenario == "permission-reply":
            passed = passed and approval_sent and any(e["phase"] == "PostToolUse" and not e["error_present"] for e in hooks) and any(
                e["phase"] == "Stop" and e.get("fullyIdle") is True and not e["error_present"] for e in hooks)
        elif scenario == "background":
            partials = [e for e in hooks if e["phase"] == "Stop" and e.get("fullyIdle") is False]
            terminals = [e for e in hooks if e["phase"] == "Stop" and e.get("fullyIdle") is True]
            passed = passed and approval_sent and bool(partials and terminals and partials[0]["time_ns"] < terminals[-1]["time_ns"])
        elif scenario != "startup":
            passed = passed and any(e.get("tool_confirmation_pending") is True or e.get("pending_input_count", 0) > 0 for e in callbacks)
        outcome = "PASS" if passed else "BLOCKED"
        if pretool_observer and scenario in ("permission", "question") and not passed and any(e["phase"] == "PreToolUse" for e in hooks):
            outcome = "LIMITATION"
        if scenario == "question" and question_ui_seen and not passed:
            outcome = "LIMITATION"
        if scenario == "cancel" and cancellation_sent and any(e.get("agent_state") == "idle" for e in callbacks) and not passed:
            outcome = "LIMITATION"
        return {"case": label, "scenario": scenario, "terminal_transport": "native private tmux" if native_tmux else "synthetic PTY",
                "wrapped": wrapped, "callbacks": list(distinct.values()), "callback_count": len(callbacks),
                "cli_pid": native_pid if native_tmux else process.pid if process else None,
                "cli_start_jiffies": native_birth[1] if native_birth else None,
                "trusted_workspace_is_generated_probe_only": True,
                "lifecycle": hooks, "permission_autoapproval": False,
                "empty_pretool_observer_installed": pretool_observer,
                "exact_probe_command_native_once_reply": approval_sent,
                "test_only_exact_modal_shape": last_modal_shape,
                "native_permission_option_words": sorted(approval_choices),
                "test_only_redacted_permission_control_rows": permission_modal_rows[-10:],
                "fixed_hook_error_diagnostic_phrases": prompt_diagnostics(plain),
                "disposable_welcome_dismissed": welcome_dismissed,
                "disposable_data_sharing_decline_attempted": telemetry_decline_attempted,
                "unchecked_native_consent_before_confirmation": unchecked_confirmed,
                "test_only_native_question_choices_visible": question_ui_seen,
                "cancellation_key_sent_after_working_callback": cancellation_sent,
                "private_settings_flags": private_flags(home),
                "pre_turn_redacted_ui_hints": hints,
                "observed_states": sorted({item.get("agent_state", "unknown") for item in callbacks}),
                "idle_observed": any(item.get("agent_state") == "idle" for item in callbacks), "terminal_flags": flags,
                "terminal_bytes": len(output), "config_path": str(app / "settings.json"),
                "retained_config": str(directory / "statusline-config.json"),
                "status": outcome}
    finally:
        if native_pid is not None and process_birth(native_pid) == native_birth:
            os.killpg(native_pid, signal.SIGTERM)
            deadline = time.monotonic() + 4
            while process_birth(native_pid) == native_birth and time.monotonic() < deadline:
                time.sleep(.05)
            if process_birth(native_pid) == native_birth:
                os.killpg(native_pid, signal.SIGKILL)
                deadline = time.monotonic() + 2
                while process_birth(native_pid) == native_birth and time.monotonic() < deadline:
                    time.sleep(.05)
        if process is not None and process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=4)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=4)
        if master is not None:
            os.close(master)
        if tmux_directory:
            mux("kill-server")
            tmux_directory.cleanup()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--token-file", required=True, type=Path)
    parser.add_argument("--scratch-dir", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--scenario", choices=("startup", "no-tool", "permission", "permission-reply", "background", "question", "cancel", "deadline"), default="startup")
    parser.add_argument("--timeout", type=int, default=75)
    parser.add_argument("--dismiss-welcome", action="store_true", help="Acknowledge only the identified disposable pre-turn welcome screen")
    parser.add_argument("--decline-telemetry", action="store_true", help="Uncheck and confirm the known first-run data-sharing form")
    parser.add_argument("--tmux", action="store_true", help="Use a private native terminal instead of a synthetic PTY")
    parser.add_argument("--pretool-observer", action="store_true", help="Experiment with metadata-only PreToolUse returning empty JSON, not a permission decision")
    args = parser.parse_args()
    if (args.dismiss_welcome or args.decline_telemetry) and not args.tmux:
        parser.error("onboarding key automation requires a native --tmux screen")
    if args.scenario in ("permission-reply", "background") and (args.pretool_observer or not args.tmux):
        parser.error("command replies require native --tmux without the unverified PreToolUse observer")
    args.scratch_dir.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="vj-agy-", dir=args.scratch_dir)).resolve()
    report = {"agy_version": subprocess.check_output(["agy", "--version"], text=True).strip(),
              "artifacts_dir": str(root), "cases": [], "no_model_invocation": args.scenario == "startup",
              "scope": "startup callback/composition" if args.scenario == "startup" else (
                  "bounded headless deadline" if args.scenario == "deadline" else "bounded native interactive " + args.scenario)}
    try:
        cases = (("baseline", False), ("wrapped", True)) if args.scenario == "startup" else ((args.scenario, True),)
        for name, wrapped in cases:
            result = run_case(root, name, args.token_file.resolve(strict=True), wrapped,
                              args.scenario, 25 if args.scenario == "startup" else args.timeout,
                              args.dismiss_welcome, args.decline_telemetry, args.tmux, args.pretool_observer)
            report["cases"].append(result)
            if result["status"] != "PASS":
                break
        report["status"] = "PASS" if len(report["cases"]) == len(cases) and all(c["status"] == "PASS" for c in report["cases"]) else (
            "LIMITATION" if report["cases"] and all(c["status"] == "LIMITATION" for c in report["cases"]) else "BLOCKED")
    except Exception as error:
        report.update(status="FAIL", error_type=type(error).__name__)
    finally:
        report["private_auth_and_app_data_removed"] = cleanup_private(root)
        if not report["private_auth_and_app_data_removed"]:
            report.update(status="FAIL", cleanup_incomplete=True)
    text = json.dumps(report, indent=2) + "\n"
    (root / "results.json").write_text(text)
    if args.output:
        args.output.write_text(text)
    print(text)
    return 0 if report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
