"""Deterministic reconciliation evidence/privacy/cleanup tests; no CLI launch."""
import copy
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from marker_probe import key
from reconciliation_contract import ARTIFACTS, CASES, ReconciliationExperiment, run
from reconciliation_evidence import validate
from smoke import sha
from verify_evidence import read_report

HERE = Path(__file__).resolve().parent


def fixture():
    report = copy.deepcopy(read_report(HERE / "reconciliation-results.json"))
    report["artifact_hashes"] = {name: sha(HERE / name) for name in ARTIFACTS}
    return report


def terminals(report):
    return [f for f in report["runs"][0]["frames"] if f["kind"] == "event" and f["event"] != "session.execution.started"]


class ReconciliationEvidenceTests(unittest.TestCase):
    def reject(self, mutate, reason=".*"):
        report = fixture()
        mutate(report)
        with self.assertRaisesRegex(ValueError, reason):
            validate(report)

    def test_retained_runtime_checks_current_artifacts_without_admission(self):
        result = validate(read_report(HERE / "reconciliation-results.json"))
        self.assertEqual((result["runs"], result["checks_per_run"]), (2, 11))
        for name in ("production_admission", "connected_transport_qualified", "p0_gate_passed"):
            self.assertIs(result[name], False)

    def test_unknown_fields_and_private_bodies_rejected_at_every_layer(self):
        for path in ((), ("target",), ("artifact_hashes",), ("runs", 0), ("runs", 0, "samples", 0),
                     ("runs", 0, "frames", 0), ("runs", 0, "frames", 3), ("runs", 0, "provider_requests"),
                     ("runs", 0, "client"), ("runs", 0, "layout", 0), ("runs", 0, "layout", 0, "birth")):
            def mutate(report):
                value = report
                for part in path:
                    value = value[part]
                value["private_body"] = "PRIVATE_SENTINEL"
            with self.subTest(path=path):
                self.reject(mutate)

    def test_mismatched_live_event_and_recovered_marker_rejected(self):
        def mutate(report):
            for event in terminals(report):
                event["marker_from_event"] = key("foreign-marker")
        self.reject(mutate, "event/marker mismatch")
        self.reject(lambda r: terminals(r)[0].update(marker_from_event=key("foreign")), "typed/envelope")

    def test_event_id_is_not_a_recovered_marker_fallback(self):
        def mutate(report):
            original = report["runs"][0]["samples"][0]["marker"]
            for event in terminals(report):
                event["marker_from_event"] = event["event_id"]
            for sample in report["runs"][0]["samples"]:
                if sample["marker"] == original:
                    sample["marker"] = terminals(report)[0]["event_id"]
        self.reject(mutate, "event ID is not marker ID")

    def test_repeated_and_restart_samples_must_keep_exact_identity(self):
        for index in (1, 2, 4):
            with self.subTest(index=index):
                self.reject(lambda r: r["runs"][0]["samples"][index].update(marker=key("foreign")), "identity")
        self.reject(lambda r: r["runs"][0]["samples"][2].update(error_type=None), "stable recovery")

    def test_unobserved_turn_cannot_reuse_previous_marker(self):
        def mutate(report):
            samples = report["runs"][0]["samples"]
            for index in (3, 4):
                samples[index]["marker"] = samples[0]["marker"]
        self.reject(mutate, "missed identity")
        self.reject(lambda r: r["runs"][0].update(observer_unchanged=False), "recovery coverage")

    def test_failure_abort_or_activity_cannot_become_done(self):
        for field, value in (("outcome", "succeeded"), ("marker_outcome", "succeeded"),
                             ("active", True), ("has_interruption_reason", True)):
            with self.subTest(field=field):
                self.reject(lambda r: r["runs"][0]["samples"][0].update({field: value}))
        for field in ("active", "has_interruption_reason"):
            self.reject(lambda r: r["runs"][0]["samples"][0].update({field: 0}), "sample booleans")
        self.reject(lambda r: r["runs"][0]["samples"][0].update(error_type="PRIVATE_SENTINEL"), "error enum")

    def test_raw_ids_and_oversized_samples_rejected(self):
        for field in ("session", "marker"):
            self.reject(lambda r: r["runs"][0]["samples"][0].update({field: "ses_PRIVATE_SENTINEL"}), "unhashed")
        self.reject(lambda r: r["runs"][0]["samples"].append(r["runs"][0]["samples"][-1]), "sample bound")
        self.reject(lambda r: terminals(r)[0].update(marker_from_event="evt_PRIVATE_SENTINEL"), "derived marker")

    def test_frame_fencing_sequence_and_selection_are_required(self):
        for mutate in (
            lambda r: r["runs"][0]["frames"].pop(),
            lambda r: r["runs"][0]["frames"][0].update(version_matches=False),
            lambda r: r["runs"][0]["frames"][1].update(index=99),
            lambda r: terminals(r)[0].update(seq=True),
            lambda r: terminals(r)[0].update(selected=None),
            lambda r: terminals(r)[0].update(pid=1),
        ):
            self.reject(mutate)

    def test_fixture_process_and_budget_proof_cannot_be_omitted(self):
        for mutate in (
            lambda r: r["runs"][0].update(control_calls=0),
            lambda r: r["runs"][0].update(control_calls=129),
            lambda r: r["runs"][0].update(control_calls=True),
            lambda r: r["runs"][0]["provider_requests"].update(failure=0),
            lambda r: r["runs"][0]["provider_requests"].update(success=1),
            lambda r: r["runs"][0].update(listener_owned=None),
            lambda r: r["runs"][0]["client"].update(tpgid=1),
        ):
            self.reject(mutate)

    def test_admission_case_matrix_hashes_and_cleanup_stay_gated(self):
        for mutate in (
            lambda r: r.update(production_admission=True),
            lambda r: r.update(connected_transport_qualified=True),
            lambda r: r["runs"].reverse(),
            lambda r: r["runs"][0].update(mode="standalone"),
            lambda r: r["runs"][0]["checks"].pop(),
            lambda r: r["runs"][0].update(cleanup=False),
            lambda r: r["artifact_hashes"].update({ARTIFACTS[0]: "0" * 64}),
        ):
            self.reject(mutate)
        report = fixture()
        report["status"] = "INCOMPLETE"
        report["runs"][0]["cleanup"] = False
        validate(report, require_pass=False)
        with self.assertRaises(ValueError):
            validate(report)
        report["runs"][0].update(status="INCOMPLETE", stage="recovery", failure="TimeoutError", checks=[], samples=[], frames=[])
        validate(report, require_pass=False)


class ReconciliationSafetyTests(unittest.TestCase):
    def test_wrong_binary_rejected_before_any_provider_or_process_launch(self):
        with patch("reconciliation_contract.Provider") as provider, patch("reconciliation_contract.ReconciliationExperiment") as experiment:
            with self.assertRaisesRegex(ValueError, "pinned_absolute_binary"):
                run(Path("relative-opencode"))
            provider.assert_not_called()
            experiment.assert_not_called()

    def test_read_budget_exhaustion_never_starts_an_extra_request(self):
        value = ReconciliationExperiment.__new__(ReconciliationExperiment)
        value.report = {"control_calls": 128}
        value.control = Mock()
        with self.assertRaisesRegex(ValueError, "read_call_bound"):
            value.read("/api/info")
        value.control.call.assert_not_called()

    def test_private_plugin_installation_makes_no_sdk_or_process_calls(self):
        with tempfile.TemporaryDirectory(prefix="verij-reconciliation-test-") as directory:
            root = Path(directory)
            value = ReconciliationExperiment(Path("/private-opencode"), root, Mock(url="http://127.0.0.1:12345/v1"), "failure")
            self.assertEqual((root / "fixture/tui.js").read_bytes(), (HERE / "reconciliation-plugin.mjs").read_bytes())
            self.assertIsNone(value.control)
            self.assertEqual(value.processes, [])

    def test_trace_or_launch_failure_still_closes_every_private_process_set(self):
        experiments, providers = [], []
        def experiment_factory(_binary, _root, _provider, case):
            value = Mock()
            value.report = {"case": case, "status": "INCOMPLETE", "cleanup": False}
            value.launch.side_effect = RuntimeError("PRIVATE_BODY")
            value.current.side_effect = ValueError("PRIVATE_TRACE")
            value.close.side_effect = lambda: value.report.update(cleanup=True)
            experiments.append(value)
            return value
        def provider_factory():
            value = Mock()
            value.snapshot.return_value = {"success": 0, "failure": 0, "blocked": 0}
            value.close.return_value = True
            providers.append(value)
            return value
        pin = read_report(HERE / "pin.json")
        with patch("reconciliation_contract.sha", return_value=pin["executable_sha256"]), \
             patch("reconciliation_contract.Provider", side_effect=provider_factory), \
             patch("reconciliation_contract.ReconciliationExperiment", side_effect=experiment_factory):
            report = run(Path("/private-opencode"))
        self.assertEqual(report["status"], "INCOMPLETE")
        self.assertEqual(len(experiments), len(CASES))
        self.assertNotIn("PRIVATE_", repr(report))
        for experiment, provider in zip(experiments, providers):
            experiment.close.assert_called_once()
            provider.close.assert_called_once()
            self.assertTrue(experiment.report["cleanup"])


if __name__ == "__main__":
    unittest.main()
