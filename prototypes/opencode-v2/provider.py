"""Controlled loopback provider; never stores request bodies or credentials."""
from collections import Counter
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import threading

from verify_evidence import bounded_json

MODELS = ("success", "failure", "blocked")


class Provider:
    def __init__(self):
        self.counts = Counter()
        self.lock = threading.Lock()
        self.entered = threading.Event()
        self.gate = threading.Event()
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_POST(self):
                self.connection.settimeout(3)
                try:
                    size = int(self.headers.get("Content-Length", "0"))
                    if not 0 < size <= 1024 * 1024:
                        self.send_error(413)
                        return
                    body = bounded_json(self.rfile.read(size), maximum=1024 * 1024)
                    model = body.get("model")
                    if model not in MODELS or self.path != "/v1/chat/completions":
                        self.send_error(400)
                        return
                    with fixture.lock:
                        fixture.counts[model] += 1
                    if model == "blocked":
                        fixture.entered.set()
                        if not fixture.gate.wait(20):
                            self.send_error(503)
                            return
                    if model == "failure":
                        self.send_response(400)
                        self.send_header("Content-Type", "application/json")
                        self.end_headers()
                        self.wfile.write(b'{"error":{"message":"private synthetic error sentinel","type":"invalid_request"}}')
                        return
                    streaming = body.get("stream", False)
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream" if streaming else "application/json")
                    self.end_headers()
                    if streaming:
                        for delta, finish in (({"role": "assistant", "content": "OK"}, None), ({}, "stop")):
                            payload = {"id": "chatcmpl_fixture", "object": "chat.completion.chunk", "created": 1,
                                       "model": model, "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
                            self.wfile.write(b"data: " + json.dumps(payload).encode() + b"\n\n")
                            self.wfile.flush()
                        self.wfile.write(b"data: [DONE]\n\n")
                    else:
                        self.wfile.write(json.dumps({"id": "chatcmpl_fixture", "object": "chat.completion", "created": 1,
                            "model": model, "choices": [{"index": 0, "message": {"role": "assistant", "content": "OK"},
                                                         "finish_reason": "stop"}],
                            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}).encode())
                except (BrokenPipeError, ConnectionResetError, TimeoutError, ValueError):
                    pass

        self.http = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        # Join accepted-request workers on server_close, not only the listener.
        self.http.daemon_threads = False
        self.thread = threading.Thread(target=self.http.serve_forever, daemon=True)
        self.thread.start()

    @property
    def url(self):
        return f"http://127.0.0.1:{self.http.server_port}/v1"

    def snapshot(self):
        with self.lock:
            return {name: self.counts[name] for name in MODELS}

    def close(self):
        self.gate.set()
        self.http.shutdown()
        self.http.server_close()
        self.thread.join(timeout=3)
        return not self.thread.is_alive()
