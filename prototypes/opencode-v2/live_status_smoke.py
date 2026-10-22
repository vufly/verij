#!/usr/bin/env python3
"""Private real-CLI loading smoke for production live-status assets.

Uses a fixture protocol sink, NOT native pane ownership/reporting. No prompts,
provider work, global configuration, or shared service changes. Reuses the P0
harness's private environment and birth-checked cleanup without changing its pin.
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import struct
import subprocess
import sys
import tempfile
import termios

from smoke import PrivateRun, identity, same_birth, sha
from marker_probe import Control

HERE = Path(__file__).resolve().parent
ASSETS = HERE.parents[1] / "integrations/opencode/v2"
SINK = '''#!/usr/bin/python3
import json,os,sys
path=os.environ["VJ_V2_PROBE_ROOT"]+"/live-protocol.ndjson"
def emit(value):
    with open(path,"a") as out: out.write(json.dumps(value)+"\\n")
emit({"kind":"ready"})
print(json.dumps({"ready":True}),flush=True)
revision=0
while True:
    line=sys.stdin.buffer.readline(65537)
    if not line: break
    if len(line)>65536 or not line.endswith(b"\\n"): sys.exit(2)
    value=json.loads(line)
    emit({"kind":"snapshot","schema":value.get("schema_version"),
          "activity":value.get("activity"),"has_conversation":value.get("conversation_id") is not None,
          "has_outcome":any(value.get(k) is not None for k in ("turn_id","completed_at_ms","error_name")),
          "pending_count":len(value.get("pending_requests",[])),
          "question_count":sum(r.get("kind")=="question" for r in value.get("pending_requests",[]))})
    revision+=1
    print(json.dumps({"record_revision":revision}),flush=True)
emit({"kind":"eof"})
'''


class LiveRun(PrivateRun):
    def __init__(self, binary, root, mode, expected_version):
        super().__init__(binary, root, mode)
        self.expected_version = expected_version
        for name in ("tui.js", "core.mjs", "reporter.mjs"):
            (root / "fixture" / name).write_bytes((ASSETS / name).read_bytes())
        # Deliberately no package.json: exercise the exact installed directory layout.
        (root / "fixture/package.json").unlink()
        self.sink = root / "sink"
        self.sink.write_text(SINK)
        self.sink.chmod(0o700)
        self.env.update(ZELLIJ_PANE_ID="0", ZELLIJ_SESSION_NAME="fixture-only")
        self.configure(False)

    def configure(self, disabled):
        entries = [{"package": str(self.root / "fixture"), "options": {
            "experimentalLiveStatus": True, "reporter": str(self.sink),
            "runtimeDir": str(self.root / "verij/runtime"), "statesDir": str(self.root / "verij/inventory")}}]
        if disabled:
            entries.append("-verij.monitoring.v2.live")
        path = self.root / "config/opencode/cli.json"
        temporary = path.with_suffix(".tmp")
        temporary.write_text(json.dumps({"plugins": entries, "animations": False,
            "attention": {"notifications": False, "sound": False}}))
        temporary.replace(path)

    def frames(self):
        path = self.root / "live-protocol.ndjson"
        if not path.exists(): return []
        raw = path.read_bytes()
        if len(raw) > 32768: raise ValueError("sink_bound")
        return [json.loads(line) for line in raw.splitlines(keepends=True) if line.endswith(b"\n")]

    def launch(self):
        version = subprocess.run([str(self.binary), "--version"], env=self.env,
            cwd=self.root / "work", stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=10, check=True)
        if version.stdout.strip() != f"opencode v{self.expected_version}".encode():
            raise ValueError("version_mismatch")
        args = [str(self.binary)]
        control = None
        selected = self.mode == "server-selected"
        if self.mode == "standalone":
            args += ["--standalone"]
        else:
            url = self.serve()
            args += ["--server", url]
            if selected:
                control = Control(url, self.env["OPENCODE_PASSWORD"])
                def ready():
                    try: return control.call("/api/info").get("version") == self.expected_version
                    except RuntimeError: return False
                self.wait(ready, timeout=15)
                root = control.call("/api/session", {"title": "Private selected fixture",
                    "location": {"directory": str(self.root / "work")}})["data"]["id"]
                foreign = control.call("/api/session", {"title": "Private foreign fixture",
                    "location": {"directory": str(self.root / "work")}})["data"]["id"]
                for session in (root, foreign):
                    control.call(f"/api/session/{session}/form", {"title": "Synthetic private form",
                        "fields": [{"type": "string", "key": "input", "required": True}]})
                args += ["--session", root]
        master, slave = pty.openpty()
        self.master = master
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 140, 0, 0))
        helper = "import fcntl,os,sys,termios;fcntl.ioctl(0,termios.TIOCSCTTY,0);os.execvpe(sys.argv[1],sys.argv[1:],os.environ)"
        try:
            client = self.remember(subprocess.Popen([sys.executable, "-c", helper, *args],
                cwd=self.root / "work", env=self.env, stdin=slave, stdout=slave, stderr=slave,
                start_new_session=True))
        finally:
            os.close(slave)
        if selected:
            self.wait(lambda: sum(f["kind"] == "snapshot" and f["has_conversation"] and f["pending_count"] == 1 for f in self.frames()) >= 2)
        else:
            self.wait(lambda: sum(f["kind"] == "snapshot" for f in self.frames()) >= 3)
        current = identity(client.pid)
        if not same_birth(self.births.get(client.pid), current) or current["tty"] == 0 or current["pgrp"] != current["tpgid"]:
            raise ValueError("client_tty")
        if Path(f"/proc/{client.pid}/exe").resolve() != self.binary: raise ValueError("client_executable")
        frames = self.frames()
        snapshots = [f for f in frames if f["kind"] == "snapshot"]
        if snapshots[0]["activity"] != "unknown": raise ValueError("initial_state")
        if selected:
            if any(f["has_outcome"] or f["activity"] != "unknown" or f["pending_count"] > 1 for f in snapshots):
                raise ValueError("selected_authority")
            if not any(f["question_count"] == 1 for f in snapshots): raise ValueError("selected_form")
        elif snapshots[1]["activity"] != "idle" or any(f["has_conversation"] or f["has_outcome"] for f in snapshots):
            raise ValueError("home_authority")
        if any(f["schema"] != 2 for f in snapshots): raise ValueError("schema")
        self.configure(True)
        self.wait(lambda: any(f["kind"] == "eof" for f in self.frames()))
        count = len(self.frames())
        self.configure(False)
        self.wait(lambda: len(self.frames()) >= count + 3)
        after = self.frames()[count:]
        if after[0]["kind"] != "ready" or after[1].get("activity") != "unknown":
            raise ValueError("reload_fence")
        self.configure(True)
        self.wait(lambda: sum(f["kind"] == "eof" for f in self.frames()) >= 2)
        if selected:
            for session in (root, foreign):
                if len(control.call(f"/api/session/{session}/form")["data"]) != 1:
                    raise ValueError("observation_settled_form")
        self.report.update(status="LIVE_LOADING_PASS", frames=self.frames(), client=current,
            checks=["production_assets_load", "real_foreground_cli", "unknown_first", "home_idle_without_authority",
                    "five_second_lease_renewal", "ordered_disable_cleanup", "reenable_unknown_fence"],
            native_ownership_verified=False, selected_status_qualified=False,
            selected_cache_hints_observed=selected)
        if selected:
            self.report["checks"] = ["production_assets_load", "real_foreground_cli", "unknown_first",
                "selected_quiescent_unknown", "selected_form_hint_only", "foreign_form_isolation",
                "forms_left_pending", "lease_renewal", "ordered_disable_cleanup", "reenable_unknown_fence"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--expected-version", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary
    if not binary.is_absolute() or binary.resolve() != binary or sha(binary) != args.sha256:
        parser.error("Select a canonical hash-pinned executable")
    if binary.open("rb").read(4) != b"\x7fELF": parser.error("Select the ELF, not the shell launcher")
    result = {"version": args.expected_version, "binary_sha256": args.sha256, "production_admission": False,
        "scope": "real CLI loading with fixture protocol sink; no native ownership/provider/resource qualification",
        "artifacts": {name: sha(ASSETS / name) for name in ("tui.js", "core.mjs", "reporter.mjs")},
        "harness_sha256": sha(Path(__file__)), "runs": []}
    for mode in ("standalone", "server", "server-selected"):
        with tempfile.TemporaryDirectory(prefix="verij-v2-live-") as tmp:
            probe = LiveRun(binary, Path(tmp), mode, args.expected_version)
            try:
                probe.launch()
            except (OSError, ValueError, RuntimeError, TimeoutError, subprocess.SubprocessError) as error:
                probe.report["failure"] = type(error).__name__
            finally:
                probe.close()
                result["runs"].append(probe.report)
    if sha(binary) != args.sha256: raise ValueError("binary_changed")
    passed = all(r["status"] == "LIVE_LOADING_PASS" and r["cleanup"] for r in result["runs"])
    result["status"] = "LIVE_LOADING_PASS" if passed else "INCOMPLETE"
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"status": result["status"], "production_admission": False}))
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
