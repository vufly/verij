"""Deterministic loopback OpenAI-compatible fixture. Never stores request bodies."""
from collections import Counter
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import threading
import time

MODELS = (
    "success", "parallel", "nested", "tool_error", "provider_error", "retry_success",
    "retry_exhausted", "cancel", "queued", "late_queued", "continuation", "auto_compact",
    "ui", "ui_nested", "manual_compact", "compact_error", "postrun_error", "bad_compaction", "continuation_error",
)


class Provider:
    def __init__(self):
        self.counts = Counter()
        self.records = []
        self.lock = threading.Lock()
        self.stop = threading.Event()
        owner = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.0"

            def log_message(self, *_args):
                pass

            def setup(self):
                super().setup()
                self.connection.settimeout(5)

            def do_POST(self):
                length = int(self.headers.get("Content-Length", 0))
                if not 0 < length <= 1024 * 1024:
                    self.send_error(413)
                    return
                data = json.loads(self.rfile.read(length))
                model = data.get("model")
                if self.path != "/v1/chat/completions" or model not in MODELS:
                    self.send_error(400)
                    return
                summary = not data.get("tools")
                with owner.lock:
                    owner.counts[model] += 1
                    attempt = owner.counts[model]
                    status = 400 if model in ("provider_error", "compact_error") else (
                        503 if model == "retry_exhausted" or (model == "retry_success" and attempt == 1) else 200)
                    owner.records.append({"model": model, "attempt": attempt, "status": status, "summary": summary})
                if status != 200:
                    self.send_response(status)
                    self.send_header("Content-Type", "application/json")
                    self.end_headers()
                    self.wfile.write(json.dumps({"error": {"message": "PRIVATE_PROVIDER_ERROR_SENTINEL",
                        "type": "server_error" if status == 503 else "invalid_request_error"}}).encode())
                    return
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()

                def chunk(delta, finish=None, usage=None):
                    frame = {"id": "chatcmpl_p0_fixture", "object": "chat.completion.chunk", "created": 1,
                        "model": model, "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
                    if usage:
                        frame["usage"] = usage
                    self.wfile.write(b"data: " + json.dumps(frame).encode() + b"\n\n")
                    self.wfile.flush()

                try:
                    chunk({"role": "assistant"})
                    if model == "cancel":
                        # Cancel against a live open stream, not a pre-start timer.
                        owner.stop.wait(10)
                    tools = []
                    if attempt == 1 and not summary:
                        tools = {
                            "parallel": [("probe_leaf", {"delayMs": 80}), ("probe_leaf", {"delayMs": 40})],
                            "nested": [("probe_nested", {})],
                            "tool_error": [("probe_leaf", {"fail": True})],
                            "queued": [("probe_queue", {})],
                            "auto_compact": [("probe_leaf", {})],
                            "ui": [("probe_ui", {})],
                            "ui_nested": [("probe_ui_nested", {})],
                        }.get(model, [])
                    if tools:
                        chunk({"tool_calls": [{"index": i, "id": f"call_p0_{i}", "type": "function",
                            "function": {"name": name, "arguments": json.dumps(args)}} for i, (name, args) in enumerate(tools)]})
                    else:
                        chunk({"content": "PRIVATE_ASSISTANT_CONTENT_SENTINEL"})
                    high = model == "auto_compact" and attempt == 1
                    chunk({}, "tool_calls" if tools else "stop", {"prompt_tokens": 30000 if high else 100,
                        "completion_tokens": 5, "total_tokens": 30005 if high else 105})
                    self.wfile.write(b"data: [DONE]\n\n")
                    self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError):
                    pass  # Cancellation deliberately closes a live stream.

        self.http = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        # server_close joins request workers too; releasing stop first wakes
        # any cancellation fixture still holding its streaming response.
        self.http.daemon_threads = False
        self.thread = threading.Thread(target=self.http.serve_forever, daemon=True)
        self.thread.start()

    @property
    def url(self):
        return f"http://127.0.0.1:{self.http.server_port}/v1"

    def observations(self):
        with self.lock:
            return list(self.records)

    def close(self):
        self.stop.set()
        self.http.shutdown()
        self.http.server_close()
        self.thread.join(timeout=3)
        return not self.thread.is_alive()
