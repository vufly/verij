"""Deterministic CLI evidence and harness safety tests; no OpenCode launch."""
import copy
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from cli_contract import ARTIFACTS, frames, run
from cli_evidence import validate
from marker_probe import key
from smoke import sha
from verify_evidence import read_report

HERE = Path(__file__).resolve().parent


def fixture():
    report = copy.deepcopy(read_report(HERE / "cli-results.json"))
    report["artifact_hashes"] = {name: sha(HERE / name) for name in ARTIFACTS}
    return report


class CliEvidenceTests(unittest.TestCase):
    def reject(self, mutate, reason=None):
        report = fixture()
        mutate(report)
        with self.assertRaisesRegex(ValueError, reason or ".*"):
            validate(report)

    def test_retained_cli_matrix_matches_current_artifacts_without_promoting_support(self):
        result = validate(read_report(HERE / "cli-results.json"))
        self.assertEqual((result["modes"], result["checks_per_mode"]), (2, 9))
        self.assertIs(result["p0_gate_passed"], False)
        self.assertIs(result["production_admission"], False)
        self.assertIs(result["connected_transport_qualified"], False)

    def test_unknown_private_fields_rejected_at_every_layer(self):
        for path in ((), ("target",), ("artifact_hashes",), ("runs", 0), ("runs", 0, "frames", 0),
                     ("runs", 0, "client"), ("runs", 0, "layout", 0), ("runs", 0, "layout", 0, "birth"),
                     ("runs", 0, "provider_requests")):
            def mutate(report):
                target = report
                for part in path:
                    target = target[part]
                target["private_body"] = "PRIVATE_SENTINEL"
            with self.subTest(path=path):
                self.reject(mutate)

    def test_raw_ids_arbitrary_types_and_error_bodies_rejected(self):
        for field in ("session", "selected", "aggregate", "event_id", "event", "channel"):
            with self.subTest(field=field):
                self.reject(lambda r: r["runs"][0]["frames"][3].update({field: "PRIVATE_SENTINEL"}))
        self.reject(lambda r: r["runs"][0].update(failure="PRIVATE_ERROR"))

    def test_setup_options_version_and_reload_generations_required(self):
        for field in ("options_match", "version_matches"):
            self.reject(lambda r: r["runs"][0]["frames"][0].update({field: False}))
        self.reject(lambda r: r["runs"][0]["frames"][0].update(generation=3), "mixed generation")
        self.reject(lambda r: r["runs"][0]["frames"][0].update(generation=True), "generation enum")

    def test_event_envelope_must_match_typed_callback_identity(self):
        self.reject(lambda r: r["runs"][0]["frames"][3].update(event_id=key("fabricated")), "typed/listen")
        self.reject(lambda r: r["runs"][0]["frames"][3].update(aggregate=key("foreign")), "aggregate mismatch")
        self.reject(lambda r: r["runs"][0]["frames"][3].update(seq=-1), "aggregate sequence")
        self.reject(lambda r: r["runs"][0]["frames"][3].update(seq=True), "aggregate sequence")
        self.reject(lambda r: r["runs"][0]["frames"][3].update(seq=2**53), "aggregate sequence")
        self.reject(lambda r: r["runs"][0]["frames"][3].update(version=True), "durable version")

    def test_home_and_plugin_routes_cannot_claim_session_authority(self):
        self.reject(lambda r: r["runs"][0]["frames"][1].update(selected=key("unowned")), "route authority")
        self.reject(lambda r: r["runs"][0]["frames"][2].update(route="plugin"), "route authority")

    def test_no_events_after_cleanup_and_no_overlapping_generations(self):
        def after_cleanup(report):
            frame = report["runs"][0]["frames"][-1].copy()
            frame.update(kind="route", route="home", selected=None)
            report["runs"][0]["frames"].append(frame)
        self.reject(after_cleanup, "after cleanup")
        def overlap(report):
            frames = report["runs"][0]["frames"]
            index = next(i for i, frame in enumerate(frames) if frame["kind"] == "cleanup")
            frames[index] = frames[0].copy()
        self.reject(overlap, "overlapping")

    def test_source_and_config_reload_cannot_replace_selected_session(self):
        def replace(report):
            frame = next(frame for frame in report["runs"][0]["frames"] if frame["kind"] == "route" and frame["generation"] == 2)
            frame["selected"] = key("foreign")
        self.reject(replace, "continuity")

    def test_disabled_observer_requires_actual_successful_provider_control(self):
        self.reject(lambda r: r["runs"][0].update(disabled_provider_requests=0), "disabled execution")
        self.reject(lambda r: r["runs"][0]["provider_requests"].update(success=0), "disabled execution")
        self.reject(lambda r: r["runs"][0].update(disabled_provider_requests=True), "request bound")

    def test_faults_are_fixed_and_cannot_pass(self):
        def fault(report):
            frame = report["runs"][0]["frames"][2]
            pid = frame["pid"]
            frame.clear()
            frame.update(kind="fault", generation=1, index=2, pid=pid, reason="schema")
        self.reject(fault, "observer fault")
        def private_fault(report):
            fault(report)
            report["runs"][0]["frames"][2]["reason"] = "PRIVATE_BODY"
        self.reject(private_fault, "fault enum")

    def test_support_cleanup_mode_order_process_ownership_and_hashes_fail_closed(self):
        mutations = (
            lambda r: r.update(production_admission=True),
            lambda r: r.update(connected_transport_qualified=True),
            lambda r: r["runs"].reverse(),
            lambda r: r["runs"][0]["checks"].pop(),
            lambda r: r["runs"][0].update(cleanup=False),
            lambda r: r["runs"][0]["client"].update(tpgid=1),
            lambda r: r["runs"][0]["frames"][0].update(pid=1),
            lambda r: r["runs"][1].update(listener_owned=None),
            lambda r: r["runs"][0]["layout"].pop(),
            lambda r: r["artifact_hashes"].update({ARTIFACTS[0]: "0" * 64}),
            lambda r: r["runs"][0]["frames"][3].update(index=999),
        )
        for index, mutate in enumerate(mutations):
            with self.subTest(index=index):
                self.reject(mutate)

    def test_partial_and_cleanup_failed_runs_can_be_retained_but_never_pass(self):
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


class CliHarnessSafetyTests(unittest.TestCase):
    def test_wrong_binary_rejected_before_any_provider_or_process_launch(self):
        with patch("cli_contract.Provider") as provider, patch("cli_contract.CliExperiment") as experiment:
            with self.assertRaisesRegex(ValueError, "pinned_absolute_binary"):
                run(Path("relative-opencode"))
            provider.assert_not_called()
            experiment.assert_not_called()

    def test_trace_failure_still_closes_both_private_runs_and_providers(self):
        instances, providers = [], []
        def experiment_factory(_binary, _root, mode, _provider):
            value = Mock()
            value.report = {"mode": mode, "status": "INCOMPLETE", "failure": None, "cleanup": False}
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
        with patch("cli_contract.sha", return_value=pin["executable_sha256"]), \
             patch("cli_contract.Provider", side_effect=provider_factory), \
             patch("cli_contract.CliExperiment", side_effect=experiment_factory):
            report = run(Path("/private-fixture-opencode"))
        self.assertEqual(report["status"], "INCOMPLETE")
        self.assertEqual(len(instances), 2)
        self.assertNotIn("PRIVATE_", repr(report))
        for instance, provider in zip(instances, providers):
            instance.close.assert_called_once()
            provider.close.assert_called_once()
            self.assertTrue(instance.report["cleanup"])
            self.assertEqual(instance.report["failure"], "ValueError")

    def test_trace_reader_bounds_frames_bytes_and_ignores_incomplete_append(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trace.ndjson"
            self.assertEqual(frames(path), [])
            path.write_bytes(b'{"kind":"setup"}\n{"kind":')
            self.assertEqual(frames(path), [{"kind": "setup"}])
            for raw in (b" " * (64 * 1024 + 1), b"{}\n" * 97, b" " * 2049 + b"\n"):
                path.write_bytes(raw)
                with self.assertRaisesRegex(ValueError, "bound"):
                    frames(path)


if __name__ == "__main__":
    unittest.main()
