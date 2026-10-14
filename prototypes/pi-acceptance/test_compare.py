"""Reject incomplete or misattributed short matched resource evidence."""
from copy import deepcopy
import unittest

from compare import qualify


def evidence():
    runs = []
    for cell in ("master_control", "rebased_control", "rebased_monitored"):
        monitored = cell == "rebased_monitored"
        roles = ["pi", "sidebar", "sidebar"] + (["reporter"] if monitored else [])
        runs.append({"cell": cell, "monitoring": monitored, "passed": True, "version": "1.2.0",
                     "cleanup": {"private_processes_exited": True, "provider_stopped": True,
                                 "burst_provider_stopped": True, "private_directory_removed": True, "forced_kills": 0},
                     "nested_tools": 160, "event_counts": {"tool_execution_start": 161,
                         "tool_execution_end": 161, "agent_settled": 1}, "completion_revision": 1,
                     "measurements": [{"phase": phase, "seconds": 10,
                         "processes": [{"role": role, "peak_rss_mib": 128} for role in roles],
                         "counters": {"native_record_writes": 2 if monitored else 0}}
                         for phase in ("idle", "tool_burst", "recovery")]})
    return {"complete": True, "runs": runs}


class CompareEvidenceTests(unittest.TestCase):
    def test_complete_matrix(self):
        self.assertTrue(qualify(evidence()))

    def test_rejects_bad_evidence(self):
        changes = (
            lambda r: r["runs"].pop(),
            lambda r: r["runs"][0].update(monitoring=True),
            lambda r: r["runs"][2].update(version="1.2.1"),
            lambda r: r["runs"][2].update(completion_revision=2),
            lambda r: r["runs"][2]["event_counts"].update(tool_execution_start=160),
            lambda r: r["runs"][2]["cleanup"].update(forced_kills=1),
            lambda r: r["runs"][2]["measurements"].pop(),
            lambda r: r["runs"][2]["measurements"][0]["counters"].update(proof_attempts=1),
            lambda r: r["runs"][2]["measurements"][2]["counters"].update(native_record_writes=0),
            lambda r: r["runs"][0]["measurements"][0]["counters"].update(native_record_writes=1),
            lambda r: r["runs"][2]["measurements"][0]["processes"].pop(),
            lambda r: r["runs"][2]["measurements"][0]["processes"][0].update(peak_rss_mib=800),
        )
        original = evidence()
        for change in changes:
            report = deepcopy(original)
            change(report)
            with self.subTest(change=changes.index(change)):
                with self.assertRaises(AssertionError):
                    qualify(report)


if __name__ == "__main__":
    unittest.main()
