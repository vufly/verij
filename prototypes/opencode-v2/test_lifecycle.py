"""Deterministic lifecycle evidence/privacy regressions; no OpenCode launch."""
import copy
from pathlib import Path
import unittest
from unittest.mock import Mock, patch

from lifecycle_evidence import validate
from lifecycle_probe import ARTIFACTS, CHECKS, LifecycleExperiment, run
from marker_probe import key
from smoke import sha
from verify_evidence import bounded_json, read_report

HERE = Path(__file__).resolve().parent


def fixture():
    # Mutation tests are synthetic; only the retained report is runtime evidence.
    report = copy.deepcopy(read_report(HERE / "lifecycle-results.json"))
    report["artifact_hashes"] = {name: sha(HERE / name) for name in ARTIFACTS}
    return report


class LifecycleEvidenceTests(unittest.TestCase):
    def reject(self, mutate, pattern=None):
        report = fixture()
        mutate(report)
        with self.assertRaisesRegex(ValueError, pattern or ".*"):
            validate(report)

    def test_retained_run_matches_current_artifacts_and_never_admits_support(self):
        result = validate(read_report(HERE / "lifecycle-results.json"))
        self.assertEqual(result["checks"], 8)
        for flag in ("production_admission", "connected_transport_qualified", "automatic_resume_qualified", "p0_gate_passed"):
            self.assertIs(result[flag], False)

    def test_unknown_fields_rejected_at_every_layer(self):
        for path in ((), ("target",), ("artifact_hashes",), ("run",), ("run", "samples", 0), ("run", "provider_requests")):
            def mutate(report):
                target = report
                for part in path:
                    target = target[part]
                target["private_body"] = "PRIVATE_SENTINEL"
            with self.subTest(path=path):
                self.reject(mutate)

    def test_raw_identity_private_error_and_unknown_outcome_rejected(self):
        for field in ("session", "marker", "revert", "outcome", "marker_outcome"):
            with self.subTest(field=field):
                self.reject(lambda r: r["run"]["samples"][1].update({field: "PRIVATE_SENTINEL"}))
        self.reject(lambda r: r["run"].update(failure="PRIVATE_ERROR_BODY"))

    def test_no_production_transport_or_resume_promotion(self):
        for flag in ("production_admission", "connected_transport_qualified", "automatic_resume_qualified"):
            with self.subTest(flag=flag):
                self.reject(lambda r: r.update({flag: True}), "cannot qualify")

    def test_stage_cannot_change_history_or_pick_non_user_boundary(self):
        self.reject(lambda r: r["run"]["samples"][1]["message_ids"].__setitem__(1, key("changed")), "stage")
        self.reject(lambda r: r["run"]["samples"][1].update(revert=r["run"]["samples"][0]["marker"]), "boundary")

    def test_clear_marker_requires_no_new_model_execution(self):
        self.reject(lambda r: r["run"].update(clear_provider_requests=1), "clear wake")
        self.reject(lambda r: r["run"]["samples"][2].update(outcome="failed"), "clear wake")
        self.reject(lambda r: r["run"]["samples"][2]["message_ids"].__setitem__(2, key("rewritten")), "clear wake")

    def test_commit_cannot_remint_older_history_or_keep_removed_marker(self):
        def remint(report):
            sample = report["run"]["samples"][3]
            sample["marker"] = sample["message_ids"][0] = key("invented")
        self.reject(remint, "commit")
        def restore(report):
            sample = report["run"]["samples"][3]
            sample["marker"] = sample["message_ids"][0] = report["run"]["samples"][0]["marker"]
        self.reject(restore, "commit")
        def drop_old_message(report):
            sample = report["run"]["samples"][3]
            sample["message_ids"].pop()
            sample["message_types"].pop()
        self.reject(drop_old_message, "commit rewrote")

    def test_replacement_cannot_reuse_removed_success(self):
        def reuse(report):
            sample = report["run"]["samples"][4]
            sample["marker"] = sample["message_ids"][0] = report["run"]["samples"][0]["marker"]
        self.reject(reuse, "replacement")

    def test_shutdown_historical_success_is_not_current_terminal(self):
        self.reject(lambda r: r["run"]["samples"][5].update(active=False), "open execution")
        for index in (5, 6, 7):
            with self.subTest(index=index):
                self.reject(lambda r: r["run"]["samples"][index].update(outcome="interrupted"), "historical success")
        def fabricate(report):
            sample = report["run"]["samples"][6]
            sample["marker"] = key("fabricated-terminal")
            sample["message_ids"].insert(0, sample["marker"])
            sample["message_types"].insert(0, "idle")
            sample["marker_count"] += 1
        self.reject(fabricate, "shutdown")

    def test_restart_cannot_remint_history_or_claim_managed_resume(self):
        self.reject(lambda r: r["run"]["samples"][7].update(active=True), "unexpectedly resumed")
        self.reject(lambda r: r["run"]["samples"][7]["message_ids"].__setitem__(0, key("new-assistant")), "restart replay")

    def test_order_bounds_types_and_duplicate_ids_fail_closed(self):
        mutations = (
            lambda r: r["run"]["checks"].append(CHECKS[0]),
            lambda r: r["run"]["checks"].reverse(),
            lambda r: r["run"]["samples"].pop(),
            lambda r: r["run"]["samples"][0].update(marker_count=True),
            lambda r: r["run"]["samples"][0].update(marker_count=33),
            lambda r: r["run"]["samples"][0].update(active=1),
            lambda r: r["run"]["samples"][0]["message_ids"].__setitem__(1, r["run"]["samples"][0]["message_ids"][0]),
            lambda r: r["run"]["samples"][0]["message_types"].__setitem__(1, "PRIVATE_TOOL_BODY"),
            lambda r: r["run"]["provider_requests"].update(blocked=True),
            lambda r: r["run"].update(clear_provider_requests=True),
            lambda r: r["artifact_hashes"].update({ARTIFACTS[0]: "0" * 64}),
        )
        for index, mutate in enumerate(mutations):
            with self.subTest(index=index):
                self.reject(mutate)

    def test_partial_runs_and_cleanup_failures_can_be_retained_but_never_pass(self):
        report = fixture()
        report["run"]["cleanup"] = False
        validate(report, require_pass=False)
        with self.assertRaises(ValueError):
            validate(report)
        report["run"].update(status="INCOMPLETE", stage="revert", checks=[], samples=[], failure="TimeoutError")
        validate(report, require_pass=False)
        with self.assertRaises(ValueError):
            validate(report)

    def test_observation_failure_does_not_retain_partial_projection(self):
        experiment = LifecycleExperiment.__new__(LifecycleExperiment)
        experiment.report = {"samples": []}
        experiment.control = Mock()
        experiment.control.call.side_effect = RuntimeError("PRIVATE_HTTP_BODY")
        def sample(_session, label):
            result = {"label": label}
            experiment.report["samples"].append(result)
            return result
        experiment.sample = sample
        with self.assertRaises(RuntimeError):
            experiment.observe("fixture-session", "active")
        self.assertEqual(experiment.report["samples"], [])

    def test_unpinned_binary_refused_before_process_or_provider_launch(self):
        with patch("lifecycle_probe.Provider") as provider, patch("lifecycle_probe.LifecycleExperiment") as experiment:
            with self.assertRaisesRegex(ValueError, "pinned_absolute_binary"):
                run(Path("relative-opencode"))
            provider.assert_not_called()
            experiment.assert_not_called()

    def test_preparse_evidence_limits_and_duplicate_fields(self):
        for raw in (b" " * (128 * 1024 + 1), b"[" * 33 + b"0" + b"]" * 33,
                    b'{"private":1,"private":2}', b"[" + b"0," * 16385 + b"0]"):
            with self.assertRaises(ValueError):
                bounded_json(raw)


if __name__ == "__main__":
    unittest.main()
