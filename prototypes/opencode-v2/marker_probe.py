#!/usr/bin/env python3
"""Private headless 2.0.32 marker/pagination experiment, NOT CLI transport qualification.

Fixture control may create prompts, forms, children, forks, and restart only its
own private server. No monitor is installed. All retained samples are projected
identities/outcomes; HTTP/message/provider bodies and auth are never persisted.
"""
import argparse
import base64
import json
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

from provider import MODELS, Provider
from smoke import HERE, PrivateRun, sha, signal_birth
from verify_evidence import bounded_json

ARTIFACTS = ("marker_probe.py", "provider.py", "marker_evidence.py", "smoke.py", "verify_evidence.py", "pin.json",
             "transport-check.mjs", "bounded-transport.mjs", "../../integrations/opencode/core.mjs")
CHECKS = ("successful_marker", "repeated_recovery", "message_cursor_order", "new_turn_marker",
          "failure_not_success", "form_read_only", "parent_pagination", "growth_invalidates_scan", "restart_identity", "fork_identity",
          "explicit_private_transport", "queued_input", "steered_turn", "user_abort")


def key(value):
    import hashlib
    if type(value) is not str or not 0 < len(value) <= 256:
        raise ValueError("identity_bound")
    return hashlib.sha256(value.encode()).hexdigest()[:20]


class Control:
    """Fixture control connection, explicitly owned. Not a production adapter.

    Byte/depth/node limits precede JSON allocation. HTTP timeout is a socket
    timeout, not an independently qualified aggregate streaming deadline.
    """
    def __init__(self, url, password):
        self.url = url
        self.headers = {"Authorization": "Basic " + base64.b64encode(("opencode:" + password).encode()).decode(),
                        "Content-Type": "application/json"}
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
        self.next_call = 0
        self.calls = 0

    def call(self, path, data=None, method=None):
        if not path.startswith("/api/") or path.startswith("//"):
            raise ValueError("private_api_path")
        delay = self.next_call - time.monotonic()
        if delay > 0:
            time.sleep(delay)
        self.next_call = time.monotonic() + 1 / 32
        self.calls += 1
        request = urllib.request.Request(self.url + path, data=json.dumps(data).encode() if data is not None else None,
            headers=self.headers, method=method or ("POST" if data is not None else "GET"))
        try:
            with self.opener.open(request, timeout=3) as response:
                raw = response.read(65537)
                if len(raw) > 65536:
                    raise ValueError("control_byte_bound")
                return bounded_json(raw, maximum=65536) if raw else None
        except urllib.error.HTTPError as error:
            error.close()
            raise RuntimeError("control_http") from None


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        return None


def fixture_config(provider):
    return {"update": "disable", "model": "fixture/success", "snapshots": False, "share": "disabled",
            "permissions": [{"action": "*", "resource": "*", "effect": "deny"}],
            "policies": [{"action": "provider.use", "resource": "*", "effect": "deny"},
                         {"action": "provider.use", "resource": "fixture", "effect": "allow"}],
            "providers": {"fixture": {"package": "@opencode/ai/providers/openai-compatible",
                "settings": {"apiKey": "fixture-only-not-a-credential", "baseURL": provider.url},
                "models": {model: {"name": model, "limit": {"context": 32000, "output": 128}} for model in MODELS}}}}


class Experiment:
    def __init__(self, binary, node, root, provider):
        self.private = PrivateRun(binary, root, "server")
        self.root, self.provider, self.node = root, provider, node
        (root / "config/opencode/opencode.json").write_text(json.dumps(fixture_config(provider)))
        self.control = None
        self.report = {"status": "INCOMPLETE", "stage": "start", "failure": None, "checks": [],
                       "samples": [], "provider_requests": {}, "cleanup": False}

    def start(self):
        url = self.private.serve()
        self.control = Control(url, self.private.env["OPENCODE_PASSWORD"])
        def ready():
            try:
                return self.control.call("/api/info").get("version") == "2.0.32"
            except RuntimeError:
                return False
        self.private.wait(ready, timeout=15)

    def create(self, model="success", parent=None):
        data = {"title": "Controlled contract fixture", "model": {"providerID": "fixture", "id": model}}
        data.update({"parentID": parent} if parent else {"location": {"directory": str(self.root / "work")}})
        return self.control.call("/api/session", data)["data"]["id"]

    def history(self, session, order="desc", limit=2):
        path = f"/api/session/{session}/message"
        cursor, seen, messages = None, set(), []
        for _ in range(16):
            query = {"limit": limit, "cursor": cursor} if cursor else {"order": order, "limit": limit}
            page = self.control.call(path + "?" + urllib.parse.urlencode(query))
            if type(page.get("data")) is not list or len(page["data"]) > limit:
                raise ValueError("message_page")
            if not page["data"]:
                return messages
            for message in page["data"]:
                if message["id"] in seen:
                    raise ValueError("message_duplicate")
                seen.add(message["id"])
                messages.append(message)
            following = page.get("cursor", {}).get("next")
            if type(following) is not str or not 0 < len(following) <= 4096 or following == cursor:
                raise ValueError("message_cursor")
            cursor = following
        raise ValueError("message_page_bound")

    def sample(self, session, label):
        info = self.control.call(f"/api/session/{session}")["data"]
        messages = self.history(session)
        markers = [message for message in messages if message["type"] == "idle"]
        marker = markers[0] if markers else None
        sample = {"label": label, "session": key(info["id"]), "outcome": info.get("outcome"),
                  "marker": key(marker["id"]) if marker else None,
                  "marker_outcome": marker.get("outcome") if marker else None,
                  "marker_count": len(markers), "message_ids": [key(message["id"]) for message in messages],
                  "message_types": [message["type"] for message in messages]}
        if sample["outcome"] not in (None, "succeeded", "failed", "interrupted") or sample["marker_outcome"] not in (None, "succeeded", "failed", "interrupted"):
            raise ValueError("outcome_schema")
        self.report["samples"].append(sample)
        return sample

    def turn(self, session):
        path = f"/api/session/{session}/message?limit=1&order=desc"
        before = self.control.call(path)["data"]
        previous = before[0]["id"] if before else None
        self.control.call(f"/api/session/{session}/prompt", {"text": "Return OK"})
        def settled():
            info = self.control.call(f"/api/session/{session}")["data"]
            active = self.control.call("/api/session/active")["data"]
            head = self.control.call(path)["data"]
            return (info.get("outcome") and session not in active and head and head[0]["type"] == "idle"
                    and head[0]["id"] != previous and head[0]["outcome"] == info["outcome"])
        self.private.wait(settled, timeout=20)

    def check(self, name, condition):
        if not condition:
            raise ValueError("contract_assertion")
        self.report["checks"].append(name)

    def run(self):
        self.private.version()
        self.start()
        self.report["stage"] = "markers"
        root = self.create()
        self.turn(root)
        first = self.sample(root, "success")
        self.check("successful_marker", first["outcome"] == first["marker_outcome"] == "succeeded" and first["marker_count"] == 1)
        repeated = self.sample(root, "recovered")
        self.check("repeated_recovery", first["marker"] == repeated["marker"] and first["message_ids"] == repeated["message_ids"])
        ascending = self.history(root, order="asc", limit=1)
        self.check("message_cursor_order", [key(message["id"]) for message in ascending] == first["message_ids"][::-1])
        self.turn(root)
        second = self.sample(root, "next_turn")
        self.check("new_turn_marker", second["marker"] != first["marker"] and second["marker_count"] == 2)
        failed = self.create("failure")
        self.turn(failed)
        error = self.sample(failed, "failure")
        self.check("failure_not_success", error["outcome"] == error["marker_outcome"] == "failed")

        self.report["stage"] = "pending_and_family"
        form = self.control.call(f"/api/session/{root}/form", {"title": "Private synthetic form sentinel",
            "fields": [{"type": "string", "key": "input", "required": True}]})["data"]["id"]
        viewed = self.control.call(f"/api/session/{root}")["data"]["time"].get("viewed")
        a = self.control.call(f"/api/session/{root}/form")["data"]
        b = self.control.call(f"/api/session/{root}/form")["data"]
        after = self.control.call(f"/api/session/{root}")["data"]
        self.check("form_read_only", any(item["id"] == form for item in a) and a == b
                   and after["time"].get("viewed") == viewed and after["outcome"] == "succeeded")
        children = [self.create(parent=root) for _ in range(3)]
        query = {"parentID": root, "limit": 1, "order": "desc"}
        first_page = self.control.call("/api/session?" + urllib.parse.urlencode(query))
        cursor, found = first_page["cursor"]["next"], [item["id"] for item in first_page["data"]]
        # A new newest child lands behind the existing descending cursor.
        # This is a real concurrent-topology gap, not snapshot isolation.
        new = self.create(parent=root)
        for _ in range(5):
            page = self.control.call("/api/session?" + urllib.parse.urlencode({"cursor": cursor, "limit": 1}))
            if not page["data"]:
                break
            if any(item.get("parentID") != root for item in page["data"]):
                raise ValueError("parent_filter")
            found.extend(item["id"] for item in page["data"])
            cursor = page["cursor"]["next"]
        else:
            raise ValueError("family_page_bound")
        self.check("parent_pagination", set(found) == set(children) and len(found) == len(children))
        fresh_head = self.control.call("/api/session?" + urllib.parse.urlencode(query))["data"][0]["id"]
        self.check("growth_invalidates_scan", new not in found and fresh_head == new
                   and fresh_head != first_page["data"][0]["id"])

        self.report["stage"] = "restart_and_fork"
        server = self.private.processes[-1]
        signal_birth(self.private.births[server.pid], signal.SIGTERM)
        self.private.wait(lambda: server.poll() is not None, timeout=8)
        self.start()
        resumed = self.sample(root, "after_restart")
        self.check("restart_identity", resumed["marker"] == second["marker"] and resumed["message_ids"] == second["message_ids"])
        fork = self.control.call(f"/api/session/{root}/fork", {})["data"]["id"]
        forked = self.sample(fork, "fork")
        # Session namespace must differ; marker copying/reminting is recorded,
        # not assumed to authorize a terminal outcome for the new family.
        self.check("fork_identity", forked["session"] != second["session"] and forked["outcome"] is None
                   and forked["marker_outcome"] == "succeeded" and forked["marker_count"] == 2
                   and not set(forked["message_ids"]) & set(second["message_ids"]))
        child = self.private.remember(subprocess.Popen([str(self.node), str(HERE / "transport-check.mjs")],
            cwd=self.root / "work", env=self.private.env, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, start_new_session=True))
        payload = {"baseUrl": self.control.url, "headers": self.control.headers,
                   "sessionID": root, "marker": resumed["marker"]}
        output, _ = child.communicate(json.dumps(payload).encode(), timeout=8)
        transport = bounded_json(output, maximum=4096) if output else None
        self.check("explicit_private_transport", child.returncode == 0 and transport == {
            "explicit_private_transport_pass": True, "calls": 2, "connected_cli_transport_qualified": False})
        self.report["stage"] = "inbox_and_abort"
        for delivery in ("queue", "steer"):
            self.provider.entered.clear()
            self.provider.gate.clear()
            session = self.create("blocked")
            self.control.call(f"/api/session/{session}/prompt", {"text": "Return OK"})
            self.private.wait(self.provider.entered.is_set, timeout=8)
            self.control.call(f"/api/session/{session}/prompt", {"text": "Return OK", "delivery": delivery})
            inbox = self.control.call(f"/api/session/{session}/inbox")["data"]
            if not any(item["delivery"] == delivery for item in inbox):
                raise ValueError("missing_pending_input")
            if self.control.call(f"/api/session/{session}/inbox")["data"] != inbox:
                raise ValueError("observation_settled_input")
            self.provider.gate.set()
            def finished():
                active = self.control.call("/api/session/active")["data"]
                head = self.control.call(f"/api/session/{session}/message?limit=1&order=desc")["data"]
                pending = self.control.call(f"/api/session/{session}/inbox")["data"]
                return session not in active and not pending and head and head[0]["type"] == "idle"
            self.private.wait(finished, timeout=15)
            sample = self.sample(session, delivery)
            users = [message for message in self.history(session) if message["type"] == "user"]
            self.check("queued_input" if delivery == "queue" else "steered_turn",
                sample["outcome"] == sample["marker_outcome"] == "succeeded" and len(users) == 2
                and sample["marker_count"] == 1)
        self.provider.entered.clear()
        self.provider.gate.clear()
        aborted = self.create("blocked")
        self.control.call(f"/api/session/{aborted}/prompt", {"text": "Return OK"})
        self.private.wait(self.provider.entered.is_set, timeout=8)
        interrupted = self.control.call(f"/api/session/{aborted}/interrupt?resume=false", {}, method="POST")
        self.private.wait(lambda: aborted not in self.control.call("/api/session/active")["data"], timeout=8)
        sample = self.sample(aborted, "abort")
        self.provider.gate.set()
        self.check("user_abort", interrupted["interrupted"] is True and sample["outcome"] == sample["marker_outcome"] == "interrupted")
        self.report["status"] = "CONTRACT_PASS"
        self.report["stage"] = "complete"


def run(binary, node):
    pin = bounded_json((HERE / "pin.json").read_bytes())
    if not binary.is_absolute() or binary.resolve() != binary or sha(binary) != pin["executable_sha256"]:
        raise ValueError("pinned_absolute_binary_required")
    if not node.is_absolute() or node.resolve() != node or not node.is_file():
        raise ValueError("absolute_node_required")
    node_hash = sha(node)
    if Path("/etc/opencode").exists() or Path("/etc/opencode").is_symlink():
        raise ValueError("ambient_system_config")
    report = {"schema": 1, "target": pin, "artifact_hashes": {name: sha(HERE / name) for name in ARTIFACTS},
              "evidence_kind": "private-headless-runtime", "production_admission": False,
              "connected_transport_qualified": False, "node_sha256": node_hash, "run": None}
    provider = Provider()
    try:
        with tempfile.TemporaryDirectory(prefix="verij-v2-marker-") as temporary:
            experiment = Experiment(binary, node, Path(temporary), provider)
            try:
                experiment.run()
            except (OSError, RuntimeError, ValueError, KeyError, TypeError, TimeoutError, subprocess.SubprocessError) as error:
                experiment.report["failure"] = ("TimeoutError" if isinstance(error, TimeoutError) else
                    "ValueError" if isinstance(error, ValueError) else "OSError" if isinstance(error, OSError) else type(error).__name__)
            finally:
                experiment.private.close()
                experiment.report["cleanup"] = experiment.private.report["cleanup"]
                experiment.report["provider_requests"] = provider.snapshot()
                report["run"] = experiment.report
    finally:
        closed = provider.close()
    report["run"]["cleanup"] = report["run"]["cleanup"] and closed
    if sha(binary) != pin["executable_sha256"] or sha(node) != node_hash:
        raise ValueError("binary_changed")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--node", type=Path, required=True)
    args = parser.parse_args()
    def interrupted(_number, _frame):
        raise KeyboardInterrupt
    previous = signal.signal(signal.SIGTERM, interrupted)
    try:
        report = run(args.binary, args.node)
    finally:
        signal.signal(signal.SIGTERM, previous)
    from marker_evidence import validate
    validate(report, require_pass=False)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    passed = report["run"]["status"] == "CONTRACT_PASS" and report["run"]["cleanup"]
    print(json.dumps({"status": report["run"]["status"], "production_admission": False}))
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
