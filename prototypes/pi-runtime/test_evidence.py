"""Regression tests for evidence integrity, privacy, and cleanup failure paths."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from provider import Provider
from verify import Probe
from verify_evidence import validate_report, validate_trace

HERE = Path(__file__).resolve().parent


class EvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.retained = json.loads((HERE / "results.json").read_text())

    def evidence(self):
        return copy.deepcopy(self.retained)

    def case(self, report, name):
        return next(case for case in report["runs"][0]["cases"] if case["case"] == name)

    def test_retained_matrix_passes_with_current_fixtures(self):
        summary = validate_report(self.evidence())
        self.assertEqual(summary["scenario_runs"], 112)
        self.assertFalse(summary["production_adapter_qualified"])

    def test_incomplete_matrix_rejected(self):
        report = self.evidence()
        report["runs"].pop()
        with self.assertRaisesRegex(ValueError, "matrix"):
            validate_report(report)

    def test_unapproved_trace_fields_rejected(self):
        trace = copy.deepcopy(self.retained["runs"][0]["cases"][1]["trace"])
        for field in ("prompt", "message", "title", "arguments", "errorMessage", "transcript_path"):
            poisoned = copy.deepcopy(trace)
            poisoned[0][field] = "PRIVATE_CONTENT_SENTINEL"
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "unapproved"):
                validate_trace(poisoned)

    def test_arbitrary_enum_value_rejected(self):
        trace = copy.deepcopy(self.retained["runs"][0]["cases"][1]["trace"])
        trace[0]["case"] = "PRIVATE_CONTENT_SENTINEL"
        with self.assertRaisesRegex(ValueError, "enum"):
            validate_trace(trace)

    def test_raw_session_identity_rejected(self):
        trace = copy.deepcopy(self.retained["runs"][0]["cases"][1]["trace"])
        trace[0]["session"] = "actual-session-name"
        with self.assertRaisesRegex(ValueError, "unhashed"):
            validate_trace(trace)

    def test_forged_event_counts_rejected(self):
        report = self.evidence()
        self.case(report, "success")["event_counts"]["agent_settled"] = 2
        with self.assertRaisesRegex(ValueError, "counts"):
            validate_report(report)

    def test_failed_cleanup_rejected(self):
        report = self.evidence()
        report["runs"][0]["cleanup"]["pi_process_group_empty"] = False
        with self.assertRaisesRegex(ValueError, "cleanup"):
            validate_report(report)

    def test_stale_fixture_hash_rejected(self):
        report = self.evidence()
        report["fixture_hashes"]["fixture.ts"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "stale fixture"):
            validate_report(report)

    def test_wrong_runtime_engine_rejected(self):
        report = self.evidence()
        for case in report["runs"][0]["cases"]:
            for event in case["trace"]:
                event["engine"] = "node"
        with self.assertRaisesRegex(ValueError, "engine"):
            validate_report(report)

    def test_unknown_outcome_cannot_be_declared_success(self):
        report = self.evidence()
        for event in self.case(report, "provider_error")["trace"]:
            if event["event"] == "agent_before_settle":
                event["outcome"] = "completed"
        with self.assertRaisesRegex(ValueError, "outcome"):
            validate_report(report)

    def test_provider_body_not_allowed_as_metadata(self):
        report = self.evidence()
        self.case(report, "success")["provider_requests"][0]["response"] = "PRIVATE_SECRET_SENTINEL"
        with self.assertRaisesRegex(ValueError, "provider metadata"):
            validate_report(report)

    def test_constructor_failure_closes_provider_and_removes_private_root(self):
        providers = []
        def create_provider():
            instance = Provider()
            providers.append(instance)
            return instance
        with tempfile.TemporaryDirectory(prefix="vj-pi-p0-test-") as temporary:
            root = Path(temporary)
            with patch("verify.Provider", side_effect=create_provider), patch.object(Probe, "launch", side_effect=RuntimeError("controlled setup failure")):
                with self.assertRaises(RuntimeError):
                    Probe(["unused"], root, "observer-first")
        self.assertFalse(root.exists())
        self.assertEqual(len(providers), 1)
        self.assertFalse(providers[0].thread.is_alive())


if __name__ == "__main__":
    unittest.main()
