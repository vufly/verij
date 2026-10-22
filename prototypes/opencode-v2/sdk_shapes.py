#!/usr/bin/env python3
"""Hash-pinned Promise SDK on a private headless fixture, not CLI provenance."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import tempfile
import time

from marker_probe import Experiment, key
from provider import Provider
from smoke import HERE, sha
from verify_evidence import bounded_json, read_report

SDK_SOURCES = {"client.ts": "9e4da23f8bcd598b45c6fe589ecaf434e3a8561bca9913cf1e4ec332d80aff36",
               "client-error.ts": "1c02bcfc01072693c43e71cc53c8dcca5d8d75327e09099cac9b37a0317ace2d"}
ARTIFACTS = ("sdk_shapes.py", "sdk_shapes_evidence.py", "sdk-runtime.mjs", "sdk-observation.mjs", "sdk-probe.mjs",
             "bounded-transport.mjs", "../../integrations/opencode/core.mjs", "marker_probe.py", "marker_evidence.py", "provider.py",
             "smoke.py", "verify_evidence.py", "pin.json")
CHECKS = ("session_get_unwrapped", "active_unwrapped", "parent_pagination", "message_cursor_order",
          "permission_location_scoping", "permission_session_pending", "form_location_scoping",
          "form_session_unwrapped", "pending_inbox_unwrapped", "preaborted_cancellation",
          "physical_request_accounting", "global_fetch_unchanged")


def copy_sources(source, destination):
    if not source.is_absolute() or source.resolve() != source:
        raise ValueError("canonical_source_directory_required")
    contents = {}
    for name, digest in SDK_SOURCES.items():
        path = source / name
        if path.is_symlink() or not path.is_file():
            raise ValueError("tagged_source_pin")
        with path.open("rb") as stream:
            content = stream.read(128 * 1024 + 1)
        if len(content) > 128 * 1024 or hashlib.sha256(content).hexdigest() != digest:
            raise ValueError("tagged_source_pin")
        contents[name] = content
    # Copy only the pinned runtime imports into the disposable root. No ambient
    # package.json, tsconfig, Bun plugins, node_modules, or shadow .js siblings.
    destination.mkdir(mode=0o700)
    for name, content in contents.items():
        (destination / name).write_bytes(content)
        if sha(destination / name) != SDK_SOURCES[name]:
            raise ValueError("source_changed_during_copy")


class ShapesExperiment(Experiment):
    def __init__(self, binary, bun, source, root, provider):
        super().__init__(binary, None, root, provider)
        self.bun, self.source = bun, source
        self.report = {"status": "INCOMPLETE", "stage": "start", "failure": None,
                       "fixture": None, "observation": None, "inputs_unchanged": False, "provider_requests": {}, "cleanup": False}

    def execute_observer(self, config):
        payload = json.dumps(config).encode()
        if len(payload) > 4096:
            raise ValueError("control_bound")
        child = self.private.remember(subprocess.Popen(
            [str(self.bun), str(HERE / "sdk-runtime.mjs"), "--source-dir", str(self.root / "sdk")],
            cwd=self.root / "work", env=self.private.env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, start_new_session=True))
        try:
            child.stdin.write(payload)
            child.stdin.close()
            output = bytearray()
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                self.private.descendants()
                if select.select([child.stdout], [], [], .05)[0]:
                    chunk = os.read(child.stdout.fileno(), 8193)
                    output.extend(chunk)
                    if len(output) > 8192:
                        raise ValueError("observer_output_bound")
                    if not chunk:
                        break
            else:
                raise TimeoutError("observer_deadline")
            child.wait(timeout=1)
            if child.returncode:
                raise RuntimeError("observer_failed")
            return bounded_json(output, maximum=8192)
        finally:
            child.stdout.close()
            if not child.stdin.closed:
                child.stdin.close()

    def run(self):
        self.private.version()
        copy_sources(self.source, self.root / "sdk")
        self.start()
        self.report["stage"] = "seed"
        root = self.create()
        self.turn(root)
        children = [self.create(parent=root) for _ in range(3)]
        location_a = str(self.root / "work")
        location_b = str(self.root / "work-b")
        Path(location_b).mkdir(mode=0o700)
        foreign = self.control.call("/api/session", {"title": "Foreign controlled fixture",
            "location": {"directory": location_b}})["data"]["id"]
        for session in (root, foreign):
            self.control.call(f"/api/session/{session}", {"permissions": [
                {"action": "fixture.observe", "resource": "*", "effect": "ask"}]}, method="PATCH")
        def permission(session):
            result = self.control.call(f"/api/session/{session}/permission", {
                "action": "fixture.observe", "resources": ["private synthetic resource sentinel"]})["data"]
            if result["effect"] != "ask":
                raise ValueError("permission_not_pending")
            return result["id"]
        def form(session):
            return self.control.call(f"/api/session/{session}/form", {"title": "Private synthetic form sentinel",
                "fields": [{"type": "string", "key": "input", "required": True}]})["data"]["id"]
        permission_a, permission_b = permission(root), permission(foreign)
        form_a, form_b = form(root), form(foreign)
        pending = self.control.call(f"/api/session/{root}/prompt", {"text": "Private queued prompt sentinel", "resume": False})["data"]["id"]
        self.provider.entered.clear()
        self.provider.gate.clear()
        active = self.create("blocked")
        self.control.call(f"/api/session/{active}/prompt", {"text": "Return OK"})
        self.private.wait(self.provider.entered.is_set, timeout=8)
        viewed = self.control.call(f"/api/session/{root}")["data"]["time"].get("viewed")
        config = {"baseUrl": self.control.url, "headers": self.control.headers, "root": root,
                  "active": active, "children": children, "locationA": location_a, "locationB": location_b,
                  "permissionA": permission_a, "permissionB": permission_b, "formA": form_a, "formB": form_b, "pending": pending}
        self.report["fixture"] = {"root": key(root), "active": key(active), "children": [key(child) for child in children],
            "permission_a": key(permission_a), "permission_b": key(permission_b), "form_a": key(form_a), "form_b": key(form_b),
            "pending": key(pending), "location_a": key(location_a), "location_b": key(location_b)}
        self.report["stage"] = "observe"
        observation = self.execute_observer(config)
        # Validate before retaining child output; it must never contain bodies,
        # credentials, raw locations or raw IDs, including on partial failure.
        from sdk_shapes_evidence import validate_observation
        validate_observation(observation)
        self.report["observation"] = observation
        self.report["stage"] = "verify_read_only"
        for session, permission_id, form_id in ((root, permission_a, form_a), (foreign, permission_b, form_b)):
            if ([item["id"] for item in self.control.call(f"/api/session/{session}/permission")["data"]] != [permission_id]
                or [item["id"] for item in self.control.call(f"/api/session/{session}/form")["data"]] != [form_id]):
                raise ValueError("observation_settled_input")
        current = self.control.call(f"/api/session/{root}")["data"]
        if ([item["id"] for item in self.control.call(f"/api/session/{root}/inbox")["data"]] != [pending]
            or current["time"].get("viewed") != viewed or current.get("outcome") != "succeeded"
            or observation["root"] != key(root)):
            raise ValueError("observation_changed_authority")
        self.report.update(status="SHAPES_PASS", stage="complete", inputs_unchanged=True)


def run(binary, bun, source):
    pin = read_report(HERE / "pin.json")
    if not binary.is_absolute() or binary.resolve() != binary or sha(binary) != pin["executable_sha256"]:
        raise ValueError("pinned_absolute_binary_required")
    if not bun.is_absolute() or bun.resolve() != bun or not bun.is_file():
        raise ValueError("canonical_bun_required")
    if Path("/etc/opencode").exists() or Path("/etc/opencode").is_symlink():
        raise ValueError("ambient_system_config")
    bun_hash = sha(bun)
    report = {"schema": 1, "target": pin, "artifact_hashes": {name: sha(HERE / name) for name in ARTIFACTS},
              "sdk_sources": SDK_SOURCES, "bun_sha256": bun_hash, "evidence_kind": "private-tagged-sdk-runtime",
              "production_admission": False, "connected_transport_qualified": False, "run": None}
    provider = Provider()
    try:
        with tempfile.TemporaryDirectory(prefix="verij-v2-sdk-shapes-") as temporary:
            experiment = ShapesExperiment(binary, bun, source, Path(temporary), provider)
            try:
                experiment.run()
            except (OSError, RuntimeError, ValueError, KeyError, TypeError, TimeoutError, subprocess.SubprocessError) as error:
                experiment.report["failure"] = ("TimeoutError" if isinstance(error, TimeoutError) else
                    "OSError" if isinstance(error, OSError) else "SubprocessError" if isinstance(error, subprocess.SubprocessError)
                    else type(error).__name__)
            finally:
                experiment.private.close()
                experiment.report["cleanup"] = experiment.private.report["cleanup"]
                experiment.report["provider_requests"] = provider.snapshot()
                report["run"] = experiment.report
    finally:
        closed = provider.close()
    report["run"]["cleanup"] = report["run"]["cleanup"] and closed
    if sha(binary) != pin["executable_sha256"] or sha(bun) != bun_hash:
        raise ValueError("binary_changed")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--bun", type=Path, required=True)
    parser.add_argument("--source-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    def interrupted(_number, _frame):
        raise KeyboardInterrupt
    previous = signal.signal(signal.SIGTERM, interrupted)
    try:
        report = run(args.binary, args.bun, args.source_dir)
    finally:
        signal.signal(signal.SIGTERM, previous)
    from sdk_shapes_evidence import validate
    validate(report, require_pass=False)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["run"]["status"], "production_admission": False}))
    return 0 if report["run"]["status"] == "SHAPES_PASS" and report["run"]["cleanup"] else 1


if __name__ == "__main__":
    sys.exit(main())
