#!/usr/bin/env python3
"""Native cross-session, per-client plugin reload and resurrection lifetime probes.

Uses private sockets/config/XDG and a disposable plugin that records native
get_plugin_ids on every actual load. No stock CLI last-active routing is used
for cross-session navigation; a display sends its own configured keybinding.
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time

from verify import completion_control, process_identity


class Probe:
    def __init__(self, binary, plugin, root, sockets):
        self.binary, self.plugin, self.root = binary, plugin, root
        self.clients, self.sessions, self.sequences = [], set(), {}
        self.report = {"binary": str(binary), "plugin": str(plugin), "artifacts_dir": str(root),
                       "checks": [], "limitations": []}
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(("ZELLIJ", "VERIJ_ZELLIJ"))}
        for kind in ("CONFIG", "CACHE", "DATA", "STATE"):
            self.env[f"XDG_{kind}_HOME"] = str(root / kind.lower())
        self.env["ZELLIJ_SOCKET_DIR"] = sockets
        self.env["TERM"] = "xterm-256color"
        self.sockets = Path(sockets) / "contract_version_1"
        self.config = root / "config.kdl"
        self.config.write_text(
            'default_shell "/bin/sh"\nshow_startup_tips false\nshow_release_notes false\n'
            'mirror_session false\nsession_serialization true\nserialization_interval 1\n'
            'keybinds {\n normal {\n'
            '  bind "Ctrl y" { SwitchSession name="lifetime-b" pane_id=1; }\n'
            '  bind "Ctrl u" { SwitchSession name="lifetime-a" pane_id=0; }\n'
            ' }\n}\n')
        self.layout = root / "layout.kdl"
        self.layout.write_text('layout {\n pane\n}\n')
        self.receiver = root / "receiver.py"
        self.receiver.write_text('import sys\nfor line in sys.stdin:\n with open(sys.argv[1], "a") as f: f.write(line); f.flush()\n')
        (root / "plugin-loads").mkdir(mode=0o700)

    def stage(self, value):
        self.report["stage"] = value

    def drain(self):
        for child in self.clients:
            master = child["master"]
            if master is not None and select.select([master], [], [], 0)[0]:
                try:
                    data = os.read(master, 65536)
                    with (child["directory"] / "terminal.log").open("ab") as output:
                        output.write(data)
                except OSError:
                    pass

    def wait(self, predicate, timeout=30):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.drain()
            try:
                value = predicate()
                if value:
                    return value
            except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired):
                pass
            time.sleep(.1)
        raise TimeoutError(f"condition deadline at {self.report.get('stage')}")

    def action(self, session, *parts):
        result = subprocess.run([str(self.binary), "--config", str(self.config), "-s", session,
                                 "action", *parts], env=self.env, cwd=self.root,
                                capture_output=True, text=True, timeout=8)
        if result.returncode or result.stdout.startswith(("Session '", "Please specify")):
            raise RuntimeError((parts, result.returncode, result.stdout[:200], result.stderr[:200]))
        return result.stdout.strip()

    def focus_map(self, session):
        result = {}
        for line in self.action(session, "list-clients").splitlines()[1:]:
            words = line.split()
            if len(words) >= 2:
                result[int(words[0])] = words[1]
        return result

    def record(self, child, server_pid=None, excluding=None):
        candidates = []
        for path in child["directory"].glob("*.json"):
            value = json.loads(path.read_text())
            if (value.get("attached") and value["client_pid"] == child["process"].pid
                    and (server_pid is None or value["server_pid"] == server_pid)
                    and value["connection_id"] != excluding):
                candidates.append((path, value))
        assert len(candidates) <= 1, candidates
        if candidates:
            child["record_path"], child["record"] = candidates[0]
            child["server_birth"] = process_identity(child["record"]["server_pid"])
            return child["record"]

    def attach(self, label, session, create=False, resurrect=False):
        self.stage("attach " + label)
        self.sessions.add(session)
        directory = self.root / label
        directory.mkdir(mode=0o700)
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 150, 0, 0))
        command = [str(self.binary), "--config", str(self.config)]
        if create:
            command += ["--layout", str(self.layout)]
        command += ["attach", "--force-run-commands", session] if resurrect else ["attach", *( ["-c"] if create else []), session]
        if create:
            command += ["--", sys.executable, str(self.receiver), str(self.root / f"{session}-pane0-input")]
        process = subprocess.Popen(command, env=dict(self.env, VERIJ_ZELLIJ_IDENTITY_DIR=str(directory)),
                                   cwd=self.root, stdin=slave, stdout=slave, stderr=slave,
                                   start_new_session=True, preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0))
        os.close(slave)
        child = {"process": process, "master": master, "directory": directory, "session": session}
        self.clients.append(child)
        self.wait(lambda: self.record(child))
        child["client_birth"] = process_identity(process.pid)
        assert child["client_birth"] and child["server_birth"]
        self.wait(lambda: child["record"]["client_id"] in self.focus_map(session))
        return child

    def complete(self, child, pane, query=False, record=None, session=None):
        record = record or child["record"]
        session = session or child["session"]
        generation = record["connection_id"]
        sequence = 0 if query else self.sequences.get(generation, 0) + 1
        if not query:
            self.sequences[generation] = sequence
        reply = completion_control(self.sockets / session, record, pane, sequence, query)
        with (self.root / "completion.ndjson").open("a") as output:
            output.write(json.dumps(reply) + "\n")
        return reply

    def focus(self, child, pane):
        result = self.complete(child, pane)
        assert result["status"] == "focused" and result["focused_pane_id"] == pane, result
        return result

    def send(self, child, token, pane):
        os.write(child["master"], b"\x1b[I")
        for character in token + "\r":
            os.write(child["master"], character.encode())
            self.drain()
            time.sleep(.02)
        self.wait(lambda: token in (self.root / f"{child['session']}-pane{pane}-input").read_text())

    def plugin_loads(self, server_pid):
        return [value for path in (self.root / "plugin-loads").glob("*.json")
                if (value := json.loads(path.read_text()))["server_pid"] == server_pid]

    def run(self):
        a = self.attach("display-a", "lifetime-a", create=True)
        peer = self.attach("display-a-peer", "lifetime-a")
        b = self.attach("display-b", "lifetime-b", create=True)
        for session in ("lifetime-a", "lifetime-b"):
            created = self.action(session, "new-pane", "--", sys.executable, str(self.receiver),
                                  str(self.root / f"{session}-pane1-input"))
            assert created == "terminal_1", created
        self.focus(a, 0)
        self.focus(peer, 1)
        self.focus(b, 0)
        self.send(a, "a-before-switch", 0)
        self.send(peer, "peer-before-switch", 1)
        self.send(b, "b-before-switch", 0)

        self.stage("native display-specific cross-session switch")
        old = dict(a["record"])
        old_path = a["record_path"]
        original_birth = a["client_birth"]
        original_server_birth = a["server_birth"]
        before_peer, before_b = self.focus_map("lifetime-a")[peer["record"]["client_id"]], self.focus_map("lifetime-b")[b["record"]["client_id"]]
        os.write(a["master"], b"\x19")  # Display A's Ctrl+y binding, not last-active CLI.
        self.wait(lambda: not json.loads(old_path.read_text())["attached"])
        new = self.wait(lambda: self.record(a, b["record"]["server_pid"], old["connection_id"]))
        a["session"] = "lifetime-b"
        assert process_identity(a["process"].pid) == original_birth
        assert new["connection_id"] != old["connection_id"]
        query = self.complete(a, 1, query=True)
        assert query["status"] == "focused" and query["latest_sequence"] == 0, query
        stale = self.complete(a, 1, record=old, session="lifetime-a")
        assert stale["status"] == "stale_attachment", stale
        assert self.focus_map("lifetime-a")[peer["record"]["client_id"]] == before_peer
        assert self.focus_map("lifetime-b")[b["record"]["client_id"]] == before_b
        self.send(a, "a-switched-b-target-one", 1)
        self.send(peer, "a-peer-stayed", 1)
        self.send(b, "b-peer-stayed", 0)
        try:
            self.complete(a, 0, record=old, session="lifetime-b")
            raise RuntimeError("wrong-server correlation was accepted")
        except AssertionError as error:
            assert str(error) == "completion must identify the bound server", error
        wrong_server = self.complete(a, 0, record=dict(old, server_pid=new["server_pid"]), session="lifetime-b")
        assert wrong_server["status"] == "stale_attachment", wrong_server
        # Numeric pane/client collisions in different servers must not authorize old tokens.
        return_old, return_path = dict(new), a["record_path"]
        os.write(a["master"], b"\x15")
        self.wait(lambda: not json.loads(return_path.read_text())["attached"])
        returned = self.wait(lambda: self.record(a, old["server_pid"], return_old["connection_id"]))
        a["session"] = "lifetime-a"
        assert returned["client_id"] == old["client_id"] and returned["connection_id"] != old["connection_id"]
        assert a["server_birth"] == original_server_birth
        assert self.complete(a, 0, query=True)["status"] == "focused"
        assert self.complete(a, 0, record=old)["status"] == "stale_attachment"
        self.send(a, "a-returned-target-zero", 0)
        self.report["checks"].append({"native_display_specific_cross_session_and_return": True,
                                      "client_process_birth_preserved": True, "source_server_birth_preserved": True,
                                      "client_birth": original_birth, "source_server_birth": original_server_birth,
                                      "old_record_invalidated": True, "same_numeric_id_fresh_generation_on_return": True,
                                      "destination_sequence_starts_at_zero": True, "peers_keyboard_unchanged": True,
                                      "wrong_server_token_rejected": True})

        self.stage("actual per-client plugin reload")
        self.focus(a, 0)
        watermark_before = self.complete(a, 0, query=True)["latest_sequence"]
        assert watermark_before > 0
        bindings_before = [json.loads(child["record_path"].read_text()) for child in (a, peer)]
        birth_before = a["server_birth"]
        url = "file:" + str(self.plugin)
        self.action("lifetime-a", "launch-plugin", "--no-focus", url)
        def initial_loads():
            values = self.plugin_loads(a["record"]["server_pid"])
            required = {a["record"]["client_id"], peer["record"]["client_id"]}
            return values if {value["client_id"] for value in values} >= required else None
        initial = self.wait(initial_loads)
        initial_nonces = {value["load_nonce"] for value in initial}
        self.action("lifetime-a", "start-or-reload-plugin", url)
        def reloaded():
            fresh = [value for value in self.plugin_loads(a["record"]["server_pid"]) if value["load_nonce"] not in initial_nonces]
            return fresh if {value["client_id"] for value in fresh} >= {a["record"]["client_id"], peer["record"]["client_id"]} else None
        fresh = self.wait(reloaded)
        assert birth_before == process_identity(a["record"]["server_pid"])
        assert bindings_before == [json.loads(child["record_path"].read_text()) for child in (a, peer)]
        assert self.complete(a, 0, query=True)["latest_sequence"] == watermark_before
        self.focus(a, 0)
        self.focus(peer, 1)
        self.send(a, "after-plugin-reload-a", 0)
        self.send(peer, "after-plugin-reload-peer", 1)
        self.report["checks"].append({"actual_plugin_load_and_reload_for_both_clients": True,
                                      "initial_loads": initial, "reloads": fresh,
                                      "server_birth": birth_before,
                                      "attachment_and_server_birth_preserved": True,
                                      "navigation_watermark_preserved": watermark_before,
                                      "correlated_focus_and_keyboard_after_reload": True})

        self.stage("save-session and native resurrection")
        resurrect_name = "lifetime-resurrect"
        original = self.attach("resurrection-original", resurrect_name, create=True)
        self.focus(original, 0)
        self.send(original, "before-resurrection", 0)
        old = dict(original["record"])
        old_server_birth = original["server_birth"]
        self.action(resurrect_name, "save-session")
        cache_layout = self.root / "cache" / "zellij" / "contract_version_1" / "session_info" / resurrect_name / "session-layout.kdl"
        self.wait(lambda: cache_layout.exists() and str(self.receiver) in cache_layout.read_text())
        serialized = cache_layout.read_text()
        assert "verij_attachment" not in serialized and old["connection_id"] not in serialized
        subprocess.run([str(self.binary), "kill-session", resurrect_name], env=self.env,
                       cwd=self.root, capture_output=True, timeout=8, check=True)
        self.wait(lambda: original["process"].poll() is not None)
        self.wait(lambda: process_identity(old["server_pid"]) is None)
        assert not json.loads(original["record_path"].read_text())["attached"]
        replacement = self.attach("resurrection-replacement", resurrect_name, resurrect=True)
        assert replacement["server_birth"] != old_server_birth
        assert replacement["record"]["connection_id"] != old["connection_id"]
        before = self.complete(replacement, 0, query=True)
        assert before["status"] == "focused" and before["latest_sequence"] == 0, before
        try:
            self.complete(replacement, 0, record=old)
            raise RuntimeError("resurrected server accepted old-server correlation")
        except AssertionError as error:
            assert str(error) == "completion must identify the bound server", error
        stale = self.complete(replacement, 0, record=dict(old, server_pid=replacement["record"]["server_pid"]))
        assert stale["status"] == "stale_attachment", stale
        self.focus(replacement, 0)
        self.send(replacement, "after-native-resurrection", 0)
        self.report["checks"].append({"native_serialized_session_resurrection": True,
                                      "layout_cache": str(cache_layout), "new_server_birth": True,
                                      "old_server_birth": old_server_birth, "replacement_server_birth": replacement["server_birth"],
                                      "same_session_name_not_same_incarnation": True,
                                      "old_token_rejected": True, "fresh_generation_watermark_zero": True,
                                      "restored_receiver_keyboard_confirmed": True})
        self.report["limitations"].append("Cross-session uses a display-owned native keybinding, not a production correlated switch endpoint or whole-host acknowledgement.")
        self.report["limitations"].append("Plugin loads prove per-client native IDs and attachment stability; Verij production plugin reload/re-registration is not implemented.")

    def cleanup(self):
        for session in self.sessions:
            subprocess.run([str(self.binary), "kill-session", session], env=self.env,
                           cwd=self.root, capture_output=True, timeout=8)
        for child in self.clients:
            try:
                child["process"].wait(timeout=4)
            except subprocess.TimeoutExpired:
                child["process"].kill()
                child["process"].wait(timeout=4)
            os.close(child["master"])
            child["master"] = None
        self.report["all_clients_exited"] = all(child["process"].poll() is not None for child in self.clients)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--plugin", required=True, type=Path)
    parser.add_argument("--scratch-dir", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    args.scratch_dir.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="vj-lifetimes-", dir=args.scratch_dir)).resolve()
    with tempfile.TemporaryDirectory(prefix="vj-life-sock-") as sockets:
        probe = Probe(args.binary.resolve(strict=True), args.plugin.resolve(strict=True), root, sockets)
        try:
            probe.run()
            probe.report.update(status="PASS")
            probe.report.pop("stage", None)
        except Exception as error:
            probe.report.update(status="FAIL", error=str(error))
        finally:
            probe.cleanup()
    text = json.dumps(probe.report, indent=2) + "\n"
    (root / "results.json").write_text(text)
    if args.output:
        args.output.write_text(text)
    print(text)
    return 0 if probe.report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
