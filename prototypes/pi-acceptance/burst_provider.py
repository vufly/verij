"""Loopback burst workload: one parent tool with 160 real nested Pi calls."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import threading


class BurstProvider:
    def __init__(self):
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_POST(self):
                length = int(self.headers.get("Content-Length", 0))
                if not 0 < length <= 1024 * 1024:
                    self.send_error(413)
                    return
                data = json.loads(self.rfile.read(length))
                if self.path != "/v1/chat/completions" or data.get("model") != "burst":
                    self.send_error(400)
                    return
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                tool = data.get("messages", [])[-1].get("role") != "tool"
                delta = {"tool_calls": [{"index": 0, "id": "call_p4_burst", "type": "function",
                         "function": {"name": "p4_burst", "arguments": "{}"}}]} if tool else {"content": "PRIVATE_BURST_SENTINEL"}
                frames = [{"role": "assistant"}, delta, {}]
                try:
                    for index, value in enumerate(frames):
                        finish = ("tool_calls" if tool else "stop") if index == 2 else None
                        frame = {"id": "p4-burst", "object": "chat.completion.chunk", "created": 1,
                                 "model": "burst", "choices": [{"index": 0, "delta": value, "finish_reason": finish}]}
                        if index == 2:
                            frame["usage"] = {"prompt_tokens": 100, "completion_tokens": 5, "total_tokens": 105}
                        self.wfile.write(b"data: " + json.dumps(frame).encode() + b"\n\n")
                        self.wfile.flush()
                    self.wfile.write(b"data: [DONE]\n\n")
                    self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError):
                    pass

        self.http = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.http.daemon_threads = False
        self.thread = threading.Thread(target=self.http.serve_forever, daemon=True)
        self.thread.start()

    @property
    def url(self):
        return f"http://127.0.0.1:{self.http.server_port}/v1"

    def close(self):
        self.http.shutdown()
        self.http.server_close()
        self.thread.join(timeout=3)
        return not self.thread.is_alive()
