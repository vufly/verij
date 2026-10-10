#!/usr/bin/env python3
"""H4 production reporting in disposable stock Zellij, with real Agy turns."""
import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import re
import signal
import subprocess
import sys
import tempfile
import time


class Probe:
    def __init__(self, root, token, verij, plugin, magy_python):
        self.root, self.verij = root, verij
        self.magy_python = str(magy_python)
        self.name = "h4-inner"
        (root / "bin").mkdir(mode=0o700)
        agy = shutil.which("agy")
        assert agy
        shutil.copyfile(agy,root / "bin/agy")
        (root / "bin/agy").chmod(0o700)
        self.env = {k:v for k,v in os.environ.items() if not k.startswith(("VERIJ", "ZELLIJ", "MAGY", "AGY", "ANTIGRAVITY", "TMUX"))}
        for kind in ("CONFIG", "CACHE", "DATA", "STATE"):
            self.env[f"XDG_{kind}_HOME"] = str(root / kind.lower())
        self.env.update(HOME=str(root / "home"), TERM="xterm-256color", AGY_CLI_DISABLE_AUTO_UPDATE="true",
                        VERIJ_STATES_DIR=str(root / "states"), VERIJ_AGENT_STATE_DIR=str(root / "agents"),
                        VERIJ_CONTROL_DIR=str(root / "control"),
                        MAGY_STATE_DIR=str(root / "magy-state"), VJ_H4_EVENTS=str(root / "callbacks.ndjson"))
        self.env.update(MAGY_DATA_DIR=str(root / "magy-data"),MAGY_CONFIG_DIR=str(root / "magy-config"))
        self.env["PATH"] = str(root / "bin") + os.pathsep + self.env["PATH"]
        self.env["MAGY_AGY_CMD"] = str(root / "bin/agy")
        for name in ("home/.gemini/antigravity-cli", "states", "agents", "control", "cache/zellij", "workspace"):
            (root / name).mkdir(mode=0o700, parents=True, exist_ok=True)
        self.app = root / "home/.gemini/antigravity-cli"
        shutil.copyfile(token, self.app / "antigravity-oauth-token")
        (self.app / "antigravity-oauth-token").chmod(0o600)
        self.settings = self.app / "settings.json"
        previous = {"statusLine":{"type":"command","command":"printf OLD_STATUS_MARKER","enabled":True,"padding":0,"stack_with_default":True},
                    "notifications":False,"showFeedbackSurvey":False,"trustedWorkspaces":[str(root / "workspace")]}
        self.settings.write_text(json.dumps(previous))
        self.original = previous
        self.report = {"root":str(root),"checks":[],"agy_version":self.call(["agy","--version"]).strip(),
                       "zellij_version":self.call(["zellij","--version"]).strip(),"semantic_inputs":"real installed Agy callbacks and authenticated provider",
                       "limitations":[]}
        self.cli("agent","setup","agy")
        installed = self.settings.read_bytes()
        self.cli("agent","setup","agy")
        assert installed == self.settings.read_bytes()
        self.check("idempotent_setup_and_previous_status_composition", self.cli("agent","doctor","agy"))
        self.assets = root / "home/.gemini/verij-monitoring-agy"
        self.adapter = self.assets / "adapter.json"
        # Metadata-only instrumentation around the installed production wrapper.
        recorder = Path(__file__).with_name("record_callback.py").resolve()
        config = json.loads(self.settings.read_text())
        config["statusLine"]["command"] = shlex.join([sys.executable,str(recorder),"ui",str(self.adapter)])
        self.settings.write_text(json.dumps(config))
        hook_file = root / "home/.gemini/config/hooks.json"
        hooks = json.loads(hook_file.read_text())
        for event in ("PreInvocation","PostInvocation","Stop"):
            hooks["verij-monitoring-v1"][event][0]["command"] = shlex.join([sys.executable,str(recorder),event,str(self.adapter)])
        hook_file.write_text(json.dumps(hooks))
        self.hook_file = hook_file
        url = "file:" + str(plugin)
        (root / "cache/zellij/permissions.kdl").write_text("\n".join(json.dumps(p) + " {\nReadApplicationState\nChangeApplicationState\nReadCliPipes\n}\n" for p in (url,str(plugin))))
        # Native Zellij permission name is ReadApplicationState.
        self.config = root / "zellij.kdl"
        self.config.write_text('default_shell "/bin/bash"\nshow_startup_tips false\nshow_release_notes false\nsession_serialization false\nmirror_session false\n'
                               + f'load_plugins {{\n {json.dumps(url)} {{ state_dir "/host/states"; control_dir "/host/control"; }}\n}}\n')
        self.env["ZELLIJ_CONFIG_FILE"] = str(self.config)
        self.socket_directory = tempfile.TemporaryDirectory(prefix="vj-h4-sockets-", dir="/tmp")
        self.env["ZELLIJ_SOCKET_DIR"] = self.socket_directory.name
        binary = shutil.which("zellij")
        assert binary
        self.env["VERIJ_ZELLIJ_BIN"] = binary
        self.terminal_directory = tempfile.TemporaryDirectory(prefix="vj-h4-terminal-", dir="/tmp")
        self.terminal = str(Path(self.terminal_directory.name) / "socket")
        self.tmux("new-session","-d","-s","h4","-x","160","-y","48","-c",str(root),
                  "exec " + shlex.join(["zellij","--config",str(self.config),"attach","-c",self.name]))
        self.observer = None

    def call(self, command, timeout=15):
        result = subprocess.run(command, env=self.env, cwd=self.root, capture_output=True, text=True, timeout=timeout)
        if result.returncode:
            raise RuntimeError(f"command failed ({result.returncode}): {command[0]}: {result.stderr[:400]}")
        return result.stdout

    def cli(self,*args):
        return json.loads(self.call([str(self.verij),*args]))

    def tmux(self,*args):
        return self.call(["tmux","-f","/dev/null","-S",self.terminal,*args])

    def action(self,*args):
        return self.call(["zellij","--config",str(self.config),"-s",self.name,"action",*args])

    def wait(self,predicate,timeout=50):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                value = predicate()
                if value: return value
            except (OSError,ValueError): pass
            time.sleep(.1)
        if hasattr(self,"terminal"):
            screen = self.screen()
            rows = [line.strip() for line in screen.splitlines() if any(word in line.lower() for word in ("error", "failed", "traceback", "unavailable", "not found", "profile", "agy", "trust", "quota"))]
            self.report["failure_ui_diagnostics"] = [re.sub(r"/[\w./-]+|[^\s]+@[^\s]+|[a-f0-9-]{36}","[REDACTED]",row)[:250] for row in rows[-8:]]
        raise TimeoutError(self.report.get("stage","wait"))

    def events(self):
        path = self.root / "callbacks.ndjson"
        return [json.loads(line) for line in path.read_text().splitlines() if line.strip()] if path.exists() else []

    def records(self):
        result = []
        for identity in (self.root / "agents/agents/v1").glob("*/identity.json"):
            value = json.loads(identity.read_text())
            try:
                stat = Path(f"/proc/{value['process']['pid']}/stat").read_text().rsplit(")",1)[1].split()
                if stat[0] not in ("Z","X") and int(stat[19]) == value["process"]["start_jiffies"]:
                    result.append({"identity":value,"state":json.loads(identity.with_name("state.json").read_text())})
            except OSError: pass
        return result

    def record(self,pid,status=None):
        return next((r for r in self.records() if r["identity"]["process"]["pid"] == pid and
                     (status is None or r["state"]["reduced"]["status"] == status)),None)

    def check(self,name,evidence=None):
        file = self.root / f"evidence-{len(self.report['checks']):02}.json"
        file.write_text(json.dumps(evidence,indent=2))
        self.report["checks"].append({"name":name,"status":"PASS","evidence":evidence,"evidence_file":str(file)})
        print(name, flush=True)

    def screen(self):
        return self.tmux("capture-pane","-p","-t","h4")

    def keys(self,*keys):
        self.tmux("send-keys","-t","h4",*keys)

    def text(self,text):
        self.tmux("send-keys","-t","h4","-l",text)
        self.keys("Enter")

    def onboarding(self):
        # Recognized native onboarding only; never blanket tool approval.
        deadline = time.monotonic() + 60
        welcomed = False
        unchecked = False
        while time.monotonic() < deadline:
            screen = self.screen().lower()
            self.report["onboarding_flags"] = {p:p in screen for p in ("welcome","press enter","help improve antigravity","yes, i agree","old_status_marker","sign in","trust","done")}
            if not welcomed and "welcome" in screen and "press enter" in screen:
                self.keys("Enter")
                welcomed = True
            elif welcomed and "help improve antigravity" in screen and "yes, i agree" in screen:
                if not unchecked:
                    self.keys("Space")
                    unchecked = True
                    time.sleep(.3)
                else:
                    if "[ ] yes, i agree" in screen:
                        self.keys("Tab","Right")
                        time.sleep(.3)
                        confirmation = self.screen().lower()
                        assert "[ ] yes, i agree" in confirmation and "done" in confirmation
                        self.keys("Enter")
                        time.sleep(1)
                        return
            elif any(e.get("agent_state") == "idle" for e in self.events()):
                return
            time.sleep(.4)
        raise TimeoutError("native onboarding/idle")

    def run(self):
        self.report["stage"] = "verified exporter"
        self.wait(lambda: list((self.root / "states").glob("*.json")))
        self.report["stage"] = "interactive Agy initial presence"
        pane = self.action("new-pane","--cwd",str(self.root / "workspace"),"--","agy","--model","gemini-3.8-flash-low").strip()
        self.wait(lambda: any(e.get("pid") for e in self.events()))
        self.onboarding()
        self.wait(lambda: any(e.get("agent_state") == "idle" for e in self.events()), timeout=30)
        time.sleep(.5)
        pid = next(e["pid"] for e in self.events() if e.get("pid"))
        self.wait(lambda: self.record(pid))
        self.check("interactive_presence_actual_pane_and_process_birth", self.record(pid))
        record = self.record(pid)
        assert record and record["identity"]["pane_key"]["terminal"] == int(pane.removeprefix("terminal_"))
        gate = subprocess.run([str(self.verij),"navigate","agent","--host","unbound-h4", "--instance",record["identity"]["agent_instance_id"]],
                              cwd=self.root,env=self.env,capture_output=True,text=True,timeout=10)
        assert gate.returncode != 0 and "no owned Workspace attachment" in gate.stderr, gate.stderr
        self.check("real_agy_navigation_owner_gate_reaches_missing_host_without_dispatch",{"ownership_passed":True,"host_binding_missing":True,"focus_dispatched":False})
        self.report["stage"] = "successful authenticated no-tool turn"
        self.text("Reply exactly OK. Do not use any tools, skills, files or browser.")
        self.wait(lambda: self.record(pid,"working"))
        self.check("interactive_working",self.record(pid))
        self.wait(lambda: self.record(pid,"done"),timeout=80)
        self.check("qualified_stop_success",self.record(pid))
        record = self.record(pid)
        assert record
        revision = record["state"]["completion_revision"]
        time.sleep(2)
        record = self.record(pid)
        assert record and record["state"]["completion_revision"] == revision
        assert all(e["neutral_stdout"] and e["callback_exit"] == 0 for e in self.events())
        self.check("neutral_lifecycle_and_visible_prior_stdout",{"event_count":len(self.events()),"completion_revision":revision})
        self.report["stage"] = "permission remains native"
        self.text("Use run_command once to run exactly: printf VJ_H4_PERMISSION. Do not use any other tool. After it completes reply OK.")
        self.wait(lambda: self.record(pid,"needs_input"),timeout=80)
        self.check("native_permission_no_pretool_gate",self.record(pid))
        screen = self.screen()
        assert "printf VJ_H4_PERMISSION" in screen and "1. yes, run command" in screen.lower()
        self.keys("Enter")
        self.wait(lambda: (r := self.record(pid,"done")) and r["state"]["completion_revision"] > revision,timeout=80)
        self.check("permission_reply_and_next_completion",self.record(pid))
        record = self.record(pid)
        assert record
        revision = record["state"]["completion_revision"]
        self.report["stage"] = "background work delays aggregate completion"
        self.text("Use run_command once with CommandLine exactly 'sleep 8; printf VJ_H4_BACKGROUND' and WaitMsBeforeAsync=1. After the tool returns a background task handle reply OK without waiting. Do not use manage_task, wait, other tools or commands.")
        self.wait(lambda: self.record(pid,"needs_input"),timeout=80)
        screen = self.screen()
        assert "sleep 8; printf VJ_H4_BACKGROUND" in screen and "1. yes, run command" in screen.lower()
        self.keys("Enter")
        def partial_stop():
            record = self.record(pid)
            return record if record and record["state"]["reduced"]["status"] == "working" and record["state"]["reduced"]["background_task_count"] > 0 else None
        partial = self.wait(partial_stop,timeout=30)
        assert partial["state"]["completion_revision"] == revision
        self.check("background_tasks_delay_done",partial)
        self.wait(lambda: (r := self.record(pid,"done")) and r["state"]["completion_revision"] > revision,timeout=80)
        self.check("aggregate_background_stop_completes_once",self.record(pid))
        self.report["stage"] = "agent exit to surviving pane shell"
        self.action("write-chars","/exit")
        self.keys("Enter")
        self.wait(lambda: self.record(pid) is None)
        self.check("agent_exit_removes_live_row")
        self.action("close-pane","--pane-id",pane)
        self.magy()
        self.report["callback_metadata"] = self.events()

    def magy_launch(self, action):
        driver = Path(__file__).with_name("magy_driver.py").resolve()
        python = self.magy_python
        try:
            self.action("focus-pane-id","terminal_0")
        except RuntimeError as error:
            if "already focused" not in str(error): raise
        self.action("write-chars",shlex.join([python,str(driver),action,str(self.root)]))
        self.keys("Enter")
        path = self.root / f"driver-{action}.json"
        result = self.wait(lambda: json.loads(path.read_text()) if path.exists() else None,timeout=40)
        assert "error_type" not in result, result
        return result

    def magy(self):
        self.report["stage"] = "Magy private profile setup"
        home = self.root / "profile-home"
        shutil.copytree(self.root / "home",home)
        self.env.update(MAGY_DATA_DIR=str(self.root / "magy-data"),MAGY_CONFIG_DIR=str(self.root / "magy-config"))
        (self.root / "magy-env.json").write_text(json.dumps({"MAGY_DATA_DIR":self.env["MAGY_DATA_DIR"],"MAGY_CONFIG_DIR":self.env["MAGY_CONFIG_DIR"],
                                       "MAGY_STATE_DIR":self.env["MAGY_STATE_DIR"],"ZELLIJ_SESSION_NAME":self.name}))
        python = self.magy_python
        self.call([python,"-c","from magy.profiles import add_profile; import sys; add_profile('h4-fixture',kind='external',home_dir=sys.argv[1])",str(home)])
        self.call(["git","init",str(self.root / "workspace")])
        self.call(["git","-C",str(self.root / "workspace"),"-c","user.name=Verij fixture","-c","user.email=fixture@example.invalid",
                   "commit","--allow-empty","-m","Fixture baseline"])
        self.observer = subprocess.Popen([str(self.verij),"agent","magy"],env=self.env,cwd=self.root,
                                         stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        existing = {r["identity"]["process"]["pid"] for r in self.records()}
        pane = self.magy_launch("pane")
        self.report["stage"] = "Magy profile-backed interactive callback"
        def profile_done():
            screen = self.action("dump-screen","--pane-id",pane["pane_id"])
            self.report["profile_screen_nonempty"] = bool(screen.strip())
            panes = json.loads(self.action("list-panes","--all","--json"))
            self.report["profile_native_pane"] = [{k:p.get(k) for k in ("id","is_plugin","exited","pane_rows","pane_columns")} for p in panes if not p["is_plugin"] and p["id"] == int(pane["pane_id"].removeprefix("terminal_"))]
            logs = self.root / "magy-state/logs/h4-fixture"
            log_diagnostics = []
            for log in logs.glob("*.log"):
                for row in log.read_text(errors="replace").splitlines():
                    if any(s in row.lower() for s in ("error", "failed", "not found", "traceback")):
                        log_diagnostics.append(re.sub(r"/[\w./-]+|[^\s]+@[^\s]+|[a-f0-9-]{36}","[REDACTED]",row)[:250])
            self.report["profile_log_diagnostics"] = log_diagnostics[-8:]
            rows = [line.strip() for line in screen.splitlines() if any(word in line.lower() for word in ("error","failed","profile","not found","traceback","trust","quota"))]
            self.report["profile_ui_diagnostics"] = [re.sub(r"/[\w./-]+|[^\s]+@[^\s]+|[a-f0-9-]{36}","[REDACTED]",row)[:250] for row in rows[-8:]]
            if any(not p["is_plugin"] and p["id"] == int(pane["pane_id"].removeprefix("terminal_")) and p.get("exited") for p in panes):
                raise RuntimeError("Magy profile pane exited; see profile_ui_diagnostics")
            return next((r for r in self.records() if r["identity"]["process"]["pid"] not in existing
                         and not r["identity"].get("runner_id") and r["state"]["reduced"]["status"] == "done"),None)
        record = self.wait(profile_done,timeout=90)
        self.check("magy_pane_start_profile_home_shared_reporter",{"pane":pane,"record":record})
        self.action("close-pane","--pane-id",pane["pane_id"])
        self.wait(lambda: self.record(record["identity"]["process"]["pid"]) is None)
        for action in ("watch","cancel-watch"):
            self.report["stage"] = f"Magy {action} runner observation"
            start = self.magy_launch(action)
            watch_id = start["watch_id"]
            def working_watch():
                state_path = self.root / "magy-state/watches" / watch_id / "state.json"
                if state_path.exists():
                    state = json.loads(state_path.read_text())
                    self.report["last_watch_state"] = {key:state.get(key) for key in ("watch_id","status","pane_id","runner_pid","runner_create_time","exit_code","conversation_id")}
                    log_path = state_path.with_name("pty.log")
                    if log_path.exists():
                        events = []
                        for line in log_path.read_text().splitlines():
                            try:
                                event = json.loads(line)
                                events.append({"event":event.get("event"),"result_status":event.get("result",{}).get("status")})
                            except ValueError: pass
                        self.report["last_watch_events"] = events
                self.report["last_watch_diagnostic"] = self.cli("agent","magy","--once")
                history = self.report.setdefault("watch_diagnostics",[])
                diagnostic = self.report["last_watch_diagnostic"]
                if diagnostic not in history and len(history) < 20: history.append(diagnostic)
                return next((r for r in self.records() if r["identity"].get("runner_id") == watch_id
                             and r["state"]["reduced"]["status"] == "working"),None)
            watched = self.wait(working_watch,timeout=80)
            assert watched["identity"]["pane_key"]["terminal"] == int(start["pane_id"].removeprefix("terminal_"))
            self.check(f"{action}_bound_native_renderer_stream_working",watched)
            self.observer.terminate(); self.observer.wait(timeout=5)
            self.observer = subprocess.Popen([str(self.verij),"agent","magy"],env=self.env,cwd=self.root,
                                             stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            if action == "cancel-watch": self.magy_launch("cancel")
            state_file = self.root / "magy-state/watches" / watch_id / "state.json"
            def finalized():
                state = json.loads(state_file.read_text())
                return state if state["status"] != "running" else None
            state = self.wait(finalized,timeout=90)
            assert state["status"] == ("completed" if action == "watch" else "cancelled"), state["status"]
            metadata = {"state":{key:state.get(key) for key in ("watch_id","status","session","pane_id","runner_pid","runner_create_time","conversation_id","exit_code")},"events":[]}
            offset = 0
            for line in state_file.with_name("pty.log").read_bytes().splitlines(keepends=True):
                offset += len(line)
                try:
                    event = json.loads(line)
                    kind = event.get("event")
                    data = event.get("step_update",{}) if kind == "step_update" else event.get("result",{}) if kind == "result" else event
                    metadata["events"].append({"event":kind,"end_offset":offset,"conversation_id":data.get("conversation_id"),
                                               "state":data.get("state"),"step_type":data.get("step_type"),"status":data.get("status")})
                except ValueError: pass
            (self.root / f"watch-metadata-{watch_id}.json").write_text(json.dumps(metadata,indent=2))
            self.wait(lambda: not any(r["identity"].get("runner_id") == watch_id for r in self.records()))
            self.wait(lambda: not any(p["id"] == int(start["pane_id"].removeprefix("terminal_")) and not p["is_plugin"] for p in json.loads(self.action("list-panes","--all","--json"))))
            self.check(f"{action}_final_state_and_auto_close_remove_row",{"watch_id":watch_id,"status":state["status"],"pane_absent":True,"metadata":metadata})
        self.report["stage"] = "pane-less detached run"
        run = self.magy_launch("run")
        time.sleep(3)
        assert not self.records(), "Detached run created a false interactive/runner row"
        self.check("pane_less_detached_run_never_registers_launcher",{"run_id":run["run_id"],"live_records":len(self.records())})
        state_file = self.root / "magy-state/runs" / run["run_id"] / "state.json"
        self.wait(lambda: json.loads(state_file.read_text())["status"] not in ("queued","running"),timeout=55)
        self.report["stage"] = "session rename preserves callback process binding"
        renamed_pane = self.action("new-pane","--cwd",str(self.root / "workspace"),"--","agy","--model","gemini-3.8-flash-low").strip()
        before = self.wait(lambda: next((r for r in self.records() if r["identity"]["pane_key"]["terminal"] == int(renamed_pane.removeprefix("terminal_"))),None))
        pid = before["identity"]["process"]["pid"]
        self.wait(lambda: any(e.get("pid") == pid and e.get("agent_state") == "idle" for e in self.events()))
        self.action("rename-session","h4-renamed")
        self.name = "h4-renamed"
        self.wait(lambda: any(json.loads(p.read_text()).get("name") == self.name for p in (self.root / "states").glob("*.json")))
        self.text("Reply exactly OK. Do not use tools, skills, files or browser.")
        self.wait(lambda: self.record(pid,"done"),timeout=80)
        after = self.record(pid)
        assert after and after["identity"] == before["identity"]
        self.check("renamed_session_same_process_instance",after)

    def close(self):
        identities = {(e["pid"],e["birth"]) for e in self.events() if e.get("pid") and e.get("birth")}
        identities.update((r["identity"]["process"]["pid"],r["identity"]["process"]["start_jiffies"]) for r in self.records())
        for file in (self.root / "states").glob("*.json"):
            snapshot = json.loads(file.read_text())
            pid = snapshot.get("inventory",{}).get("producer",{}).get("server_pid")
            if pid:
                try:
                    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")",1)[1].split()
                    identities.add((pid,int(fields[19])))
                except OSError: pass
        self.report.setdefault("callback_metadata",self.events())
        if self.observer and self.observer.poll() is None:
            self.observer.terminate(); self.observer.wait(timeout=5)
        for name in {self.name,"h4-inner","h4-renamed"}:
            try: self.call(["zellij","kill-session",name])
            except (RuntimeError,subprocess.SubprocessError): pass
        try: self.tmux("kill-server")
        except (RuntimeError,subprocess.SubprocessError): pass
        def alive(pid,birth):
            try:
                fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")",1)[1].split()
                return fields[0] not in ("Z","X") and int(fields[19]) == birth
            except OSError: return False
        deadline = time.monotonic() + 4
        while any(alive(pid,birth) for pid,birth in identities) and time.monotonic() < deadline:
            time.sleep(.1)
        for pid,birth in identities:
            if alive(pid,birth):
                os.kill(pid,signal.SIGTERM)
        time.sleep(.2)
        for pid,birth in identities:
            if alive(pid,birth): os.kill(pid,signal.SIGKILL)
        assert not any(alive(pid,birth) for pid,birth in identities)
        self.socket_directory.cleanup()
        self.terminal_directory.cleanup()
        # Restore instrumentation to installed entries before uninstall check.
        manifest = json.loads((self.assets / "installed.json").read_text())
        settings = json.loads(self.settings.read_text())
        settings["statusLine"] = manifest["installed_status"]
        settings["user_added_after_setup"] = True
        self.settings.write_text(json.dumps(settings))
        hooks = json.loads(self.hook_file.read_text())
        hooks["verij-monitoring-v1"] = manifest["installed_hooks"]
        hooks["unrelated"] = {"Stop":[]}
        self.hook_file.write_text(json.dumps(hooks))
        self.cli("agent","uninstall","agy")
        restored = json.loads(self.settings.read_text())
        assert restored["statusLine"] == self.original["statusLine"] and restored["user_added_after_setup"]
        assert "unrelated" in json.loads(self.hook_file.read_text())
        self.check("uninstall_restores_previous_status_and_preserves_unrelated_entries")
        shutil.rmtree(self.root / "home")
        shutil.rmtree(self.root / "profile-home",ignore_errors=True)
        shutil.rmtree(self.root / "magy-state",ignore_errors=True)
        self.report["cleanup"] = {"private_auth_removed":not (self.root / "home").exists(),"socket_context_removed":True,
                                  "recorded_agent_lifetimes_exited":len(identities)}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--verij",type=Path,default=Path("target/release/verij"))
    parser.add_argument("--plugin",type=Path,default=Path("dist/verij_plugin.wasm"))
    parser.add_argument("--magy-python",type=Path,required=True,help="Python interpreter in installed Magy environment")
    parser.add_argument("--token-file",type=Path,required=True)
    parser.add_argument("--scratch-dir",type=Path,required=True)
    parser.add_argument("--output",type=Path,required=True)
    args = parser.parse_args()
    root = Path(tempfile.mkdtemp(prefix="vj-h4-",dir=args.scratch_dir.resolve()))
    probe = None
    report = {"root":str(root),"status":"FAIL"}
    try:
        probe = Probe(root,args.token_file.resolve(),args.verij.resolve(),args.plugin.resolve(),args.magy_python.absolute())
        probe.run()
        probe.report["status"] = "PASS"
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
        if probe: probe.report.update(status="FAIL",error=report["error"])
    finally:
        if probe:
            try: probe.close()
            except Exception as error: probe.report.update(status="FAIL",cleanup_error=f"{type(error).__name__}: {error}")
            report = probe.report
        else:
            shutil.rmtree(root / "home",ignore_errors=True)
        args.output.write_text(json.dumps(report,indent=2))
        print(json.dumps({"status":report["status"],"root":str(root),"checks":len(report.get("checks",[])),"error":report.get("error")}))
    return 0 if report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
