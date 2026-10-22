"""Deterministic marker/privacy regressions. No OpenCode launch or global changes."""
import copy
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import threading
import unittest
import urllib.request

from marker_evidence import validate
from marker_probe import ARTIFACTS, CHECKS, Control, key, sha
from provider import Provider
from verify_evidence import read_report

HERE = Path(__file__).resolve().parent


def synthetic_report():
    # Use the retained projection as a shape fixture, not another runtime run.
    report = copy.deepcopy(read_report(HERE / "marker-results.json"))
    report["artifact_hashes"] = {name: sha(HERE / name) for name in ARTIFACTS}
    return report


class MarkerEvidenceTests(unittest.TestCase):
    def test_retained_evidence_matches_current_artifacts(self):
        result = validate(read_report(HERE / "marker-results.json"))
        self.assertEqual(result["checks"], 14)
        self.assertFalse(result["production_admission"])
        self.assertFalse(result["connected_transport_qualified"])
        self.assertFalse(result["p0_gate_passed"])

    def test_unknown_private_fields_rejected_at_every_layer(self):
        for path in ((), ("target",), ("run",), ("run", "samples", 0), ("run", "provider_requests")):
            report = synthetic_report()
            target = report
            for component in path:
                target = target[component]
            target["prompt"] = "PRIVATE_SENTINEL"
            with self.subTest(path=path), self.assertRaises(ValueError):
                validate(report)

    def test_raw_ids_and_arbitrary_outcomes_or_errors_rejected(self):
        for field, value in (("session", "raw-session-id"), ("marker", "raw-marker-id"),
                             ("outcome", "PRIVATE_PROVIDER_BODY"), ("marker_outcome", "PRIVATE_CONTENT")):
            report = synthetic_report()
            report["run"]["samples"][0][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                validate(report)
        report = synthetic_report()
        report["run"]["failure"] = "PRIVATE_ERROR_BODY"
        with self.assertRaises(ValueError):
            validate(report)

    def test_recovery_and_restart_cannot_mint_new_identity(self):
        for index in (1, 4):
            report = synthetic_report()
            sample = report["run"]["samples"][index]
            sample["marker"] = key("invented-marker")
            sample["message_ids"][0] = sample["marker"]
            with self.subTest(index=index), self.assertRaises(ValueError):
                validate(report)

    def test_new_turn_cannot_reuse_old_marker(self):
        report = synthetic_report()
        first, _, second = report["run"]["samples"][:3]
        second["marker"] = first["marker"]
        with self.assertRaises(ValueError):
            validate(report)

    def test_fork_historical_success_cannot_create_root_outcome(self):
        report = synthetic_report()
        report["run"]["samples"][5]["outcome"] = "succeeded"
        with self.assertRaisesRegex(ValueError, "fork"):
            validate(report)
        report = synthetic_report()
        report["run"]["samples"][5]["message_ids"] = report["run"]["samples"][2]["message_ids"].copy()
        report["run"]["samples"][5]["marker"] = report["run"]["samples"][2]["marker"]
        with self.assertRaisesRegex(ValueError, "fork"):
            validate(report)

    def test_queue_and_steer_each_share_one_marker_in_this_fixture(self):
        for index in (6, 7):
            report = synthetic_report()
            report["run"]["samples"][index]["marker_count"] = 2
            with self.subTest(index=index), self.assertRaises(ValueError):
                validate(report)

    def test_message_projection_counts_and_types_cannot_be_forged(self):
        for mutate in (lambda s: s["message_types"].pop(),
                       lambda s: s["message_types"].__setitem__(0, "PRIVATE_TOOL_OUTPUT"),
                       lambda s: s["message_types"].__setitem__(1, "idle"),
                       lambda s: s["message_types"].__setitem__(2, "assistant")):
            report = synthetic_report()
            mutate(report["run"]["samples"][6])
            with self.assertRaises(ValueError):
                validate(report)

    def test_failure_and_abort_never_become_done(self):
        for index in (3, 8):
            report = synthetic_report()
            report["run"]["samples"][index].update(outcome="succeeded", marker_outcome="succeeded")
            with self.subTest(index=index), self.assertRaises(ValueError):
                validate(report)

    def test_missing_checks_wrong_order_or_samples_cannot_pass(self):
        for mutate in (lambda r: r["run"]["checks"].pop(), lambda r: r["run"]["checks"].reverse(),
                       lambda r: r["run"]["samples"].pop()):
            report = synthetic_report()
            mutate(report)
            with self.assertRaises(ValueError):
                validate(report)

    def test_cleanup_failure_and_incomplete_run_can_be_retained_but_never_pass(self):
        report = synthetic_report()
        report["run"]["cleanup"] = False
        validate(report, require_pass=False)
        with self.assertRaisesRegex(ValueError, "cleanup"):
            validate(report)
        report["run"].update(status="INCOMPLETE", failure="TimeoutError", stage="markers", checks=[], samples=[])
        validate(report, require_pass=False)
        with self.assertRaises(ValueError):
            validate(report)

    def test_forged_admission_stale_sources_and_unbounded_counts_rejected(self):
        mutations = [lambda r: r.update(production_admission=True),
                     lambda r: r.update(connected_transport_qualified=True),
                     lambda r: r["artifact_hashes"].update({ARTIFACTS[0]: "0" * 64}),
                     lambda r: r["run"]["provider_requests"].update(success=101),
                     lambda r: r["run"]["provider_requests"].update(success=True),
                     lambda r: r["run"]["samples"][0].update(marker_count=33),
                     lambda r: r["run"]["samples"][0].update(marker=None),
                     lambda r: r["run"]["checks"].append(CHECKS[0])]
        for mutate in mutations:
            report = synthetic_report()
            mutate(report)
            with self.assertRaises(ValueError):
                validate(report)


class FixtureSafetyTests(unittest.TestCase):
    def test_provider_counts_only_fixed_models_and_joins_on_close(self):
        provider = Provider()
        try:
            payload = json.dumps({"model": "success", "messages": [{"role": "user", "content": "PRIVATE_PROMPT_SENTINEL"}]}).encode()
            request = urllib.request.Request(provider.url + "/chat/completions", data=payload,
                                             headers={"Content-Type": "application/json"})
            with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=3) as response:
                response.read(4096)
            self.assertEqual(provider.snapshot(), {"success": 1, "failure": 0, "blocked": 0})
            self.assertNotIn("PRIVATE_PROMPT_SENTINEL", repr(vars(provider)))
        finally:
            self.assertTrue(provider.close())
        self.assertEqual(provider.http.socket.fileno(), -1)
        self.assertFalse(provider.thread.is_alive())

    def test_control_refuses_redirects_without_forwarding_credentials(self):
        hits = []
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass
            def do_GET(self):
                hits.append(self.path)
                self.send_response(302)
                self.send_header("Location", "/private-target")
                self.end_headers()
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        try:
            control = Control(f"http://127.0.0.1:{server.server_port}", "fixture-only")
            with self.assertRaisesRegex(RuntimeError, "control_http"):
                control.call("/api/info")
            self.assertEqual(hits, ["/api/info"])
        finally:
            server.shutdown(); server.server_close(); thread.join(timeout=3)
        self.assertFalse(thread.is_alive())


if __name__ == "__main__":
    unittest.main()
