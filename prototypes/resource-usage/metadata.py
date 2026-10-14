#!/usr/bin/env python3
"""Exercise bounded metadata recovery against a private, real OpenCode server.

Conversation messages are genuine controlled-provider/no-reply turns. The large
stored tool part is explicitly synthetic fixture data, inserted through the real
REST API. Metrics count bytes delivered by the adapter's transport, not server-side
serialization or OpenCode's ordinary TUI cache allocation.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import time

SOURCE = Path(__file__).resolve().parents[1] / "h3-opencode/verify.py"
spec = importlib.util.spec_from_file_location("h3_metadata", SOURCE)
assert spec and spec.loader
h3 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(h3)
REPO = Path(__file__).resolve().parents[2]


class Probe(h3.Probe):
    def __init__(self, *args):
        super().__init__(*args)
        (self.root / "metrics").mkdir()
        self.env.update(VERIJ_RESOURCE_MONITORING="enabled", VERIJ_RESOURCE_REPORTER=str(self.verij),
                        VERIJ_RESOURCE_ROOT=str(self.root), VERIJ_RESOURCE_PER_PROCESS="1",
                        VERIJ_RESOURCE_TRANSPORT_METRICS="1")
        config_path = self.root / "config/opencode/tui.json"
        config = json.loads(config_path.read_text())
        config["plugin"] = [config["plugin"][0], str(Path(__file__).with_name("runtime-tui.mjs"))]
        config_path.write_text(json.dumps(config))
        self.report["measurement_kind"] = "actual OpenCode HTTP/TUI/native reporter; synthetic stored tool part"
        self.report["assets"] = {name: {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
            for name, path in {"bridge": REPO / "integrations/opencode/bridge.mjs",
                               "core": REPO / "integrations/opencode/core.mjs", "cli": self.verij,
                               "transport_probe": Path(__file__).with_name("runtime-tui.mjs")}.items()}
        self.peak = {}

    def resources(self):
        server = getattr(self, "server", None)
        inventory = self.root / f"states/{self.name}.json"
        zellij_server = None
        if inventory.exists():
            try:
                zellij_server = (json.loads(inventory.read_text()).get("inventory") or {}).get("producer", {}).get("server_pid")
            except (OSError, ValueError):
                pass
        for pid in [*self.children, *([server.pid] if server is not None else []), *([zellij_server] if zellij_server else [])]:
            identity = h3.runtime.process_identity(pid)
            if identity is None:
                continue
            rss = int(Path(f"/proc/{pid}/statm").read_text().split()[1]) * os.sysconf("SC_PAGE_SIZE")
            self.peak[str(pid)] = max(self.peak.get(str(pid), 0), rss)
            limit = 512 if pid == zellij_server else 1536
            if rss > limit * 2**20:
                raise RuntimeError(f"Private process {pid} exceeded {limit} MiB RSS guard")

    def wait(self, predicate, timeout=45):
        def guarded():
            self.resources()
            return predicate()
        return super().wait(guarded, timeout=timeout)

    def metrics(self, pid):
        return json.loads((self.root / f"metrics/api-{pid}.json").read_text())

    def run(self, history, output_bytes):
        a, b, aa, bb = self.start()
        apid = aa["pid"]
        self.wait(lambda: self.record(apid, "idle"))
        selected = self.http("/session", {})["id"]
        self.stage("seed genuine long conversation")
        for index in range(history):
            self.http(f"/session/{selected}/message", {"noReply": True,
                "model": {"providerID": "fixture", "modelID": "success"},
                "parts": [{"type": "text", "text": f"Controlled historical turn {index}"}]})
            self.resources()
        self.select(apid, selected)
        self.prompt(selected, "success")
        self.wait(lambda: self.record(apid, "done"))
        revision = self.record(apid)["state"]["completion_revision"]
        latest = self.http(f"/session/{selected}/message?limit=1")[0]
        part_id = latest["parts"][0]["id"]
        self.stage("insert synthetic large tool part into actual terminal message")
        now = int(time.time() * 1000)
        self.http(f"/session/{selected}/message/{latest['info']['id']}/part/{part_id}", {
            "id": part_id, "sessionID": selected, "messageID": latest["info"]["id"],
            "type": "tool", "tool": "fixture-large-output", "callID": "fixture-large-output",
            "state": {"status": "completed", "input": {}, "output": "x" * output_bytes,
                      "title": "Synthetic stored output", "metadata": {}, "time": {"start": now, "end": now}}}, method="PATCH")
        first = self.metrics(apid)
        self.stage("repeat quiescent metadata recovery with large output")
        def recovered_twice():
            metrics = self.metrics(apid)
            return metrics if metrics["endpoints"]["session.messages"] >= first["endpoints"]["session.messages"] + 6 else None
        last = self.wait(recovered_twice, timeout=35)
        # Two complete root suffix proofs must not transfer either whole tool body.
        transferred = last["transport"]["session.messages"]["bytes"] - first["transport"]["session.messages"]["bytes"]
        assert transferred < output_bytes // 2, (transferred, output_bytes)
        assert last["max_message_limit"] == 1
        assert self.record(apid, "done")["state"]["completion_revision"] == revision
        self.report["checks"].append({"large_output_repeated_recovery": True, "history_messages": history,
            "synthetic_tool_output_bytes": output_bytes, "transport_bytes": transferred,
            "message_queries": last["endpoints"]["session.messages"] - first["endpoints"]["session.messages"],
            "max_message_limit": last["max_message_limit"], "completion_revision": revision,
            "metrics_before": first, "metrics_after": last})
        self.stage("reload on long conversation without replay")
        source_epoch = self.record(apid)["state"]["sources"]["opencode"]["source_epoch"]
        self.control(apid, "disable")
        self.control(apid, "enable")
        self.wait(lambda: (record := self.record(apid, "done")) and
                  record["state"]["sources"]["opencode"]["source_epoch"] != source_epoch)
        assert self.record(apid)["state"]["completion_revision"] == revision
        self.report["checks"].append({"long_history_reload_same_completion": True,
            "completion_revision": revision, "source_epoch_before": source_epoch,
            "source_epoch_after": self.record(apid)["state"]["sources"]["opencode"]["source_epoch"],
            "provider_requests": dict(self.fixture.counts)})
        self.stage("standalone worker metadata without message-body RPC")
        pane = self.action("new-pane", "--", "opencode", "--session", selected)
        standalone = self.ready(int(pane.split("_")[-1]))
        spid = standalone["pid"]
        self.children.append(spid)
        self.wait(lambda: self.record(spid, "done"))
        def worker_ready():
            metrics = self.metrics(spid)
            return metrics if metrics["endpoints"]["session.messages"] >= 3 else None
        first_worker = self.wait(worker_ready)
        def worker_recovered_twice():
            metrics = self.metrics(spid)
            return metrics if metrics["endpoints"]["session.messages"] >= first_worker["endpoints"]["session.messages"] + 6 else None
        last_worker = self.wait(worker_recovered_twice, timeout=35)
        assert last_worker["transport"]["session.messages"]["bytes"] == 0
        assert last_worker["max_message_limit"] == 1
        assert self.record(spid, "done")["state"]["completion_revision"] == 1
        self.report["checks"].append({"standalone_worker_no_message_body_rpc": True,
            "history_messages": history, "synthetic_tool_output_bytes": output_bytes,
            "metrics_before": first_worker, "metrics_after": last_worker,
            "completion_revision": self.record(spid)["state"]["completion_revision"]})
        self.report["limitations"] = ["Attach transport byte counts include read-ahead chunks, not just metadata bytes.",
            "Standalone metadata requires verified local storage/indexes; unavailable or in-memory DBs stay conservative.",
            "Other stock worker SDK endpoints still own their upstream buffers; adapter guards bound decoding and parsing.",
            "Stored tool output is synthetic REST seed data, not a real tool execution.",
            "Short runtime checks do not establish long-run memory stability or server-side serialization bounds."]
        self.report.update(status="PASS", rss_guard_mib={"opencode": 1536, "zellij": 512},
                           peak_rss_mib={pid: value / 2**20 for pid, value in self.peak.items()})
        self.report.pop("stage", None)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--zellij", type=Path, required=True)
    parser.add_argument("--verij", type=Path, required=True)
    parser.add_argument("--plugin", type=Path, required=True)
    parser.add_argument("--scratch-dir", type=Path, default=Path("/tmp/opencode"))
    parser.add_argument("--history", type=int, default=128)
    parser.add_argument("--output-bytes", type=int, default=8 * 2**20)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not 101 <= args.history <= 256 or not 2**20 <= args.output_bytes <= 16 * 2**20:
        parser.error("history must be 101–256 and output size 1–16 MiB")
    root = Path(tempfile.mkdtemp(prefix="verij-metadata-", dir=args.scratch_dir)).resolve()
    with tempfile.TemporaryDirectory(prefix="verij-metadata-sockets-") as sockets:
        probe = Probe(args.zellij.resolve(), args.verij.resolve(), args.plugin.resolve(), root, sockets)
        try:
            probe.run(args.history, args.output_bytes)
        except Exception as error:
            probe.report.update(status="FAIL", error=repr(error))
        finally:
            probe.close()
    text = json.dumps(probe.report, indent=2) + "\n"
    (root / "results.json").write_text(text)
    args.output.write_text(text)
    print(json.dumps({"status": probe.report["status"], "error": probe.report.get("error"), "root": str(root)}))
    return 0 if probe.report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
