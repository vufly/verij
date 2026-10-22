"""Deterministic terminal-event evidence/privacy/safety tests; no CLI launch."""
import copy
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from cli_contract import frames
from marker_probe import key
from smoke import sha
from terminal_contract import ARTIFACTS, CASES, TerminalExperiment, run
from terminal_evidence import validate
from verify_evidence import read_report

HERE = Path(__file__).resolve().parent


def fixture():
    report = copy.deepcopy(read_report(HERE / "terminal-results.json"))
    report["artifact_hashes"] = {name: sha(HERE / name) for name in ARTIFACTS}
    return report


def terminal_events(report, index=0):
    return [f for f in report["runs"][index]["frames"] if f["kind"] == "event" and f["event"] != "session.execution.started"]


class TerminalEvidenceTests(unittest.TestCase):
    def reject(self, mutate, reason=".*"):
        report = fixture()
        mutate(report)
        with self.assertRaisesRegex(ValueError, reason):
            validate(report)

    def test_retained_four_run_matrix_matches_artifacts_without_admission(self):
        result = validate(read_report(HERE / "terminal-results.json"))
        self.assertEqual((result["runs"], result["checks_per_run"]), (4, 6))
        for field in ("production_admission", "connected_transport_qualified", "p0_gate_passed"):
            self.assertIs(result[field], False)

    def test_unknown_fields_rejected_at_all_layers(self):
        for path in ((), ("target",), ("artifact_hashes",), ("runs", 0), ("runs", 0, "frames", 0),
                     ("runs", 0, "frames", 3), ("runs", 0, "client"), ("runs", 0, "layout", 0),
                     ("runs", 0, "layout", 0, "birth"), ("runs", 0, "provider_requests")):
            def mutate(report):
                value = report
                for part in path:
                    value = value[part]
                value["private_body"] = "PRIVATE_SENTINEL"
            with self.subTest(path=path):
                self.reject(mutate)

    def test_raw_ids_error_text_and_arbitrary_error_types_rejected(self):
        for field in ("session", "selected", "aggregate", "event_id", "error_type", "interruption_reason"):
            with self.subTest(field=field):
                self.reject(lambda r: terminal_events(r)[0].update({field: "PRIVATE_SENTINEL"}))
        self.reject(lambda r: r["runs"][0].update(failure="PRIVATE_ERROR"))
        self.reject(lambda r: terminal_events(r)[0].update(error_type=[]), "error type enum")
        self.reject(lambda r: terminal_events(r, 1)[0].update(interruption_reason={}), "reason enum")

    def test_error_type_and_reason_must_match_event_kind(self):
        self.reject(lambda r: terminal_events(r)[0].update(error_type=None), "error/event")
        self.reject(lambda r: terminal_events(r)[0].update(interruption_reason="user"), "reason/event")
        self.reject(lambda r: terminal_events(r, 1)[0].update(error_type="unknown"), "error/event")
        self.reject(lambda r: terminal_events(r, 1)[0].update(interruption_reason=None), "reason/event")

    def test_typed_and_envelope_payload_identity_must_agree(self):
        self.reject(lambda r: terminal_events(r)[0].update(error_type="provider.auth"), "typed/listen")
        self.reject(lambda r: terminal_events(r)[0].update(event_id=key("foreign")), "typed/listen")
        self.reject(lambda r: terminal_events(r)[0].update(seq=999), "typed/listen")

    def test_both_channels_cannot_relabel_provider_failure_or_user_abort(self):
        def error(report):
            for f in terminal_events(report):
                f["error_type"] = "other"
        self.reject(error, "terminal metadata")
        for reason in ("shutdown", "superseded", "inactivity"):
            def interruption(report):
                for f in terminal_events(report, 1):
                    f["interruption_reason"] = reason
            with self.subTest(reason=reason):
                self.reject(interruption, "terminal metadata")

    def test_terminal_events_cannot_be_reclassified_as_success(self):
        def success(report):
            for f in terminal_events(report):
                f.update(event="session.execution.succeeded", error_type=None)
        self.reject(success, "typed/listen")

    def test_durable_identity_sequence_version_and_selected_authority_fail_closed(self):
        for mutate in (
            lambda r: terminal_events(r)[0].update(aggregate=key("foreign")),
            lambda r: terminal_events(r)[0].update(seq=True),
            lambda r: terminal_events(r)[0].update(seq=2**53),
            lambda r: terminal_events(r)[0].update(version=True),
            lambda r: terminal_events(r)[0].update(route="plugin"),
            lambda r: terminal_events(r)[0].update(selected=None),
        ):
            self.reject(mutate)

    def test_setup_cleanup_faults_and_frame_order_required(self):
        self.reject(lambda r: r["runs"][0]["frames"][0].update(options_match=False), "lifecycle")
        self.reject(lambda r: r["runs"][0]["frames"].pop(), "lifecycle")
        self.reject(lambda r: r["runs"][0]["frames"][1].update(index=99), "ordering")
        def fault(report):
            frame = report["runs"][0]["frames"][2]
            keep = {k: frame[k] for k in ("generation", "index", "pid")}
            frame.clear()
            frame.update(kind="fault", reason="schema", **keep)
        self.reject(fault, "observer fault")

    def test_provider_control_not_event_names_alone_required(self):
        for mutate in (
            lambda r: r["runs"][0]["provider_requests"].update(failure=0),
            lambda r: r["runs"][1]["provider_requests"].update(blocked=0),
            lambda r: r["runs"][0]["provider_requests"].update(success=1),
            lambda r: r["runs"][1]["provider_requests"].update(blocked=True),
        ):
            self.reject(mutate)

    def test_mode_case_hash_and_ownership_gates(self):
        for mutate in (
            lambda r: r.update(production_admission=True),
            lambda r: r.update(connected_transport_qualified=True),
            lambda r: r["runs"].reverse(),
            lambda r: r["runs"][0].update(case="user_abort"),
            lambda r: r["runs"][0]["checks"].pop(),
            lambda r: r["runs"][0].update(cleanup=False),
            lambda r: r["runs"][0]["client"].update(tpgid=1),
            lambda r: r["runs"][0]["frames"][0].update(pid=1),
            lambda r: r["runs"][2].update(listener_owned=None),
            lambda r: r["artifact_hashes"].update({ARTIFACTS[0]: "0" * 64}),
        ):
            self.reject(mutate)

    def test_incomplete_and_cleanup_failed_runs_never_pass(self):
        report = fixture()
        report["status"] = "INCOMPLETE"
        report["runs"][0]["cleanup"] = False
        validate(report, require_pass=False)
        with self.assertRaises(ValueError):
            validate(report)
        report["runs"][0].update(status="INCOMPLETE", failure="TimeoutError", stage="execution", frames=[], checks=[])
        validate(report, require_pass=False)
        with self.assertRaises(ValueError):
            validate(report)


class TerminalHarnessSafetyTests(unittest.TestCase):
    def test_wrong_binary_rejected_before_provider_or_process_launch(self):
        with patch("terminal_contract.Provider") as provider, patch("terminal_contract.TerminalExperiment") as experiment:
            with self.assertRaisesRegex(ValueError, "pinned_absolute_binary"):
                run(Path("relative-opencode"))
            provider.assert_not_called()
            experiment.assert_not_called()

    def test_private_configuration_selects_case_without_model_flag_or_sdk_mutation(self):
        provider = Mock(url="http://127.0.0.1:12345/v1")
        for case, model in (("failure", "failure"), ("user_abort", "blocked")):
            with tempfile.TemporaryDirectory(prefix="verij-terminal-test-") as directory:
                root = Path(directory)
                experiment = TerminalExperiment(Path("/private-opencode"), root, "standalone", provider, case)
                import json
                config = json.loads((root / "config/opencode/opencode.json").read_text())
                self.assertEqual(config["model"], "fixture/" + model)
                self.assertEqual((root / "fixture/tui.js").read_bytes(), (HERE / "terminal-observer.mjs").read_bytes())
                self.assertIsNone(experiment.master)
                self.assertEqual(experiment.processes, [])

    def test_trace_failure_still_closes_every_private_process_set_and_provider(self):
        instances, providers = [], []
        def experiment_factory(_binary, _root, mode, _provider, case):
            value = Mock()
            value.report = {"mode": mode, "case": case, "status": "INCOMPLETE", "failure": None, "cleanup": False}
            value.launch.side_effect = RuntimeError("PRIVATE_HTTP_BODY")
            value.current.side_effect = ValueError("PRIVATE_TRACE_BODY")
            value.close.side_effect = lambda: value.report.update(cleanup=True)
            instances.append(value)
            return value
        def provider_factory():
            value = Mock()
            value.snapshot.return_value = {"success": 0, "failure": 0, "blocked": 0}
            value.close.return_value = True
            providers.append(value)
            return value
        pin = read_report(HERE / "pin.json")
        with patch("terminal_contract.sha", return_value=pin["executable_sha256"]), \
             patch("terminal_contract.Provider", side_effect=provider_factory), \
             patch("terminal_contract.TerminalExperiment", side_effect=experiment_factory):
            report = run(Path("/private-fixture-opencode"))
        self.assertEqual(report["status"], "INCOMPLETE")
        self.assertEqual(len(instances), len(CASES))
        self.assertNotIn("PRIVATE_", repr(report))
        for instance, provider in zip(instances, providers):
            instance.close.assert_called_once()
            provider.close.assert_called_once()
            self.assertTrue(instance.report["cleanup"])
            self.assertEqual(instance.report["failure"], "ValueError")

    def test_trace_bounds_apply_before_parsing(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trace"
            for raw in (b" " * (65536 + 1), b"{}\n" * 97, b" " * 2049 + b"\n"):
                path.write_bytes(raw)
                with self.assertRaisesRegex(ValueError, "bound"):
                    frames(path)


if __name__ == "__main__":
    unittest.main()
