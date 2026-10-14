"""Private OpenCode server + controlled streaming provider, no credentials/model calls."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import socket
import threading
import time
import urllib.request


class Runtime:
    def __init__(self, experiment):
        self.experiment = experiment
        self.stop = threading.Event()
        self.requests = 0
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, format, *args):
                pass

            def do_POST(self):
                data = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                owner.requests += 1
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                deadline = time.monotonic() + experiment.args.seconds - .5
                def chunk(delta, reason=None):
                    self.wfile.write(b"data: " + json.dumps({"id": "chatcmpl_resource", "object": "chat.completion.chunk",
                        "created": int(time.time()), "model": data["model"],
                        "choices": [{"index": 0, "delta": delta, "finish_reason": reason}]}).encode() + b"\n\n")
                    self.wfile.flush()
                try:
                    chunk({"role": "assistant"})
                    while time.monotonic() < deadline and not owner.stop.is_set():
                        rate = json.loads((experiment.root / "workload.json").read_text())["events_per_second"]
                        if rate <= 0:
                            break
                        chunk({"content": "x "})
                        owner.stop.wait(1 / rate)
                    chunk({}, "stop")
                    self.wfile.write(b"data: [DONE]\n\n")
                    self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError):
                    pass

        self.provider = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.provider.daemon_threads = True
        self.thread = threading.Thread(target=self.provider.serve_forever, daemon=True)
        self.thread.start()
        root = experiment.root
        env = experiment.env.copy()
        env.update(HOME=str(root / "home"), OPENCODE_TEST_HOME=str(root / "home"),
            OPENCODE_CONFIG_DIR=str(root / "config/opencode"), OPENCODE_DISABLE_PROJECT_CONFIG="1",
            OPENCODE_DISABLE_AUTOUPDATE="1", OPENCODE_DISABLE_DEFAULT_PLUGINS="1", OPENCODE_DISABLE_EXTERNAL_SKILLS="1",
            OPENCODE_DISABLE_CLAUDE_CODE_SKILLS="1", VERIJ_RESOURCE_ROOT=str(root),
            VERIJ_RESOURCE_MONITORING=experiment.mode, VERIJ_RESOURCE_REPORTER=str(experiment.cli))
        self.env = env
        config = {"$schema": "https://opencode.ai/config.json", "model": "fixture/stream", "small_model": "fixture/stream",
            "enabled_providers": ["fixture"], "autoupdate": False, "share": "disabled", "snapshot": False,
            "lsp": False, "formatter": False, "permission": {"*": "deny"},
            "agent": {"title": {"disable": True}, "summary": {"disable": True}},
            "provider": {"fixture": {"npm": "@ai-sdk/openai-compatible", "name": "Controlled resource fixture",
                "options": {"apiKey": "probe-not-a-secret", "baseURL": f"http://127.0.0.1:{self.provider.server_port}/v1"},
                "models": {"stream": {"name": "stream", "limit": {"context": 32000, "output": 2000}}}}}}
        (root / "opencode.json").write_text(json.dumps(config))
        (root / "tui.json").write_text(json.dumps({"plugin": [str(Path(__file__).with_name("runtime-tui.mjs"))]}))
        env.update(OPENCODE_CONFIG=str(root / "opencode.json"), OPENCODE_TUI_CONFIG=str(root / "tui.json"))
        self.log = (root / "opencode-server.log").open("wb")
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        self.server = subprocess.Popen(["opencode", "serve", "--port", str(port), "--hostname", "127.0.0.1"],
            cwd=root, env=env, stdout=self.log, stderr=self.log, start_new_session=True)
        experiment.runtime = self
        experiment.report["opencode_runtime"] = {"provider": "controlled local SSE fixture; no remote provider"}
        experiment.remember(self.server.pid, "opencode_server")
        # Verify the reserved private listener belongs to this exact server PID.
        def listener():
            text = (root / "opencode-server.log").read_text()
            for line in text.splitlines():
                if "http://127.0.0.1:" in line:
                    return line[line.index("http://127.0.0.1:"):].split()[0]
            return None
        self.url = experiment.wait(listener)
        assert self.url == f"http://127.0.0.1:{port}"
        listeners = {line.split()[9] for line in Path(f"/proc/{self.server.pid}/net/tcp").read_text().splitlines()[1:]
                     if int(line.split()[1].split(":")[1], 16) == port and line.split()[3] == "0A"}
        targets = set()
        for fd in Path(f"/proc/{self.server.pid}/fd").iterdir():
            try:
                targets.add(os.readlink(fd))
            except FileNotFoundError:
                pass
        assert targets & {f"socket:[{inode}]" for inode in listeners}, "private listener not held by server PID"
        paths = self.http("/path")
        assert isinstance(paths, dict)
        assert all(str(root) in paths.get(name, "") for name in ("home", "config", "state", "directory")), paths
        created = self.http("/session", {})
        assert isinstance(created, dict)
        self.session = created["id"]
        experiment.report["opencode_runtime"] = {"version": subprocess.check_output(["opencode", "--version"], env=env, text=True).strip(),
            "private_listener": self.url, "paths": paths, "provider": "controlled local SSE fixture; no remote provider"}

    def http(self, path, data=None):
        request = urllib.request.Request(self.url + path, data=json.dumps(data).encode() if data is not None else None,
            headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(request, timeout=5) as response:
            content = response.read()
            return json.loads(content) if content else None

    def command(self):
        # Launch-time HOME isolation applies only to OpenCode, preserving stock Zellij layout loading.
        additions = {k: v for k, v in self.env.items() if self.experiment.env.get(k) != v}
        return ["env", *[f"{k}={v}" for k, v in additions.items()], "opencode", "attach", self.url, "--session", self.session]

    def prompt(self):
        self.http(f"/session/{self.session}/prompt_async", {"model": {"providerID": "fixture", "modelID": "stream"},
            "parts": [{"type": "text", "text": "Controlled resource probe; return fixture text."}]})

    def close(self):
        self.stop.set()
        self.provider.shutdown()
        self.provider.server_close()
        self.server.terminate()
        try:
            self.server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.server.kill()
            self.server.wait(timeout=5)
        self.thread.join(timeout=2)
        self.log.close()
        self.experiment.report["opencode_runtime"]["provider_requests"] = self.requests
