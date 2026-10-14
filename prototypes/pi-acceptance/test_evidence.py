"""Evidence gate negative tests. No Zellij, provider or Pi process launches."""
import copy
import json
from pathlib import Path
import unittest

from verify_evidence import qualify

HERE = Path(__file__).resolve().parent


class EvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.evidence = json.loads((HERE / "results.json").read_text())

    def changed(self):
        return copy.deepcopy(self.evidence)

    def test_retained_matrix(self):
        self.assertTrue(qualify(self.evidence)["qualified"])

    def test_incomplete_runtime_rejected(self):
        report = self.changed()
        report["runs"].pop()
        with self.assertRaises(AssertionError):
            qualify(report)

    def test_missing_case_rejected(self):
        report = self.changed()
        report["runs"][0]["cases"] = [c for c in report["runs"][0]["cases"] if c["case"] != "cancel"]
        with self.assertRaises(AssertionError):
            qualify(report)

    def test_abort_cannot_be_done(self):
        report = self.changed()
        case = next(c for c in report["runs"][0]["cases"] if c["case"] == "cancel")
        case["facts"]["expected_status"] = "done"
        case["facts"]["completion_after"] += 1
        with self.assertRaises(AssertionError):
            qualify(report)

    def test_premature_native_completion_rejected(self):
        report = self.changed()
        case = next(c for c in report["runs"][0]["cases"] if c["case"] == "parallel")
        sample = next(s for s in case["samples"] if s["completion_revision"] > case["facts"]["completion_before"])
        sample["observed_settlements"] = case["facts"]["settlements_before"]
        with self.assertRaises(AssertionError):
            qualify(report)

    def test_unknown_request_provenance_rejected(self):
        report = self.changed()
        case = next(c for c in report["runs"][0]["cases"] if c["case"] == "ui")
        sample = next(s for s in case["samples"] if s["pending_count"])
        sample["request_kinds"] = ["permission"]
        with self.assertRaises(AssertionError):
            qualify(report)

    def test_raw_content_rejected(self):
        report = self.changed()
        report["runs"][0]["terminal_on_failure"] = "PRIVATE_PROVIDER_ERROR_SENTINEL"
        with self.assertRaises(AssertionError):
            qualify(report)

    def test_unclean_processes_rejected(self):
        report = self.changed()
        report["runs"][0]["cleanup"]["private_processes_exited"] = False
        with self.assertRaises(AssertionError):
            qualify(report)

    def test_forced_cleanup_rejected(self):
        report = self.changed()
        report["runs"][0]["cleanup"]["forced_kills"] = 1
        with self.assertRaises(AssertionError):
            qualify(report)

    def test_missing_reporter_counters_rejected(self):
        report = self.changed()
        idle = next(m for m in report["runs"][0]["measurements"] if m["phase"] == "two_panes_two_hosts_idle")
        idle["counters"]["native_record_writes"] = 0
        with self.assertRaises(AssertionError):
            qualify(report)

    def test_idle_proofs_rejected(self):
        report = self.changed()
        idle = next(m for m in report["runs"][0]["measurements"] if m["phase"] == "idle")
        idle["counters"]["proof_attempts"] = 1
        with self.assertRaises(AssertionError):
            qualify(report)

    def test_burst_budget_rejected(self):
        report = self.changed()
        burst = next(m for m in report["runs"][0]["measurements"] if m["phase"] == "two_panes_tool_burst")
        burst["counters"]["native_record_writes"] = 10000
        with self.assertRaises(AssertionError):
            qualify(report)

    def test_insufficient_actual_tools_rejected(self):
        report = self.changed()
        case = next(c for c in report["runs"][0]["cases"] if c["case"] == "burst_budget")
        next(iter(case["facts"]["event_counts_by_pane"].values()))["tool_execution_start"] = 1
        with self.assertRaises(AssertionError):
            qualify(report)


if __name__ == "__main__":
    unittest.main()
