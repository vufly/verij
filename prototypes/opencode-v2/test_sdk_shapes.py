"""Deterministic SDK shape evidence/privacy/safety regressions; no OpenCode."""
import copy
import hashlib
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from marker_probe import key
from sdk_shapes import ARTIFACTS, ShapesExperiment, copy_sources, run
from sdk_shapes_evidence import validate
from smoke import sha
from verify_evidence import read_report

HERE = Path(__file__).resolve().parent


def fixture():
    report = copy.deepcopy(read_report(HERE / "sdk-shapes-results.json"))
    report["artifact_hashes"] = {name: sha(HERE / name) for name in ARTIFACTS}
    return report


class ShapeEvidenceTests(unittest.TestCase):
    def reject(self, mutate, reason=None):
        report = fixture()
        mutate(report)
        with self.assertRaisesRegex(ValueError, reason or ".*"):
            validate(report)

    def test_retained_runtime_matches_current_sources_and_never_admits_cli_support(self):
        result = validate(read_report(HERE / "sdk-shapes-results.json"))
        self.assertEqual(result["checks"], 12)
        for name in ("production_admission", "connected_transport_qualified", "p0_gate_passed"):
            self.assertIs(result[name], False)

    def test_unknown_private_fields_rejected_at_every_layer(self):
        for path in ((), ("target",), ("sdk_sources",), ("artifact_hashes",), ("run",), ("run", "fixture"),
                     ("run", "observation"), ("run", "observation", "children"), ("run", "observation", "messages"),
                     ("run", "observation", "permissions"), ("run", "observation", "forms"),
                     ("run", "observation", "metrics"), ("run", "provider_requests")):
            def mutate(report):
                target = report
                for part in path:
                    target = target[part]
                target["prompt"] = "PRIVATE_SENTINEL"
            with self.subTest(path=path):
                self.reject(mutate)

    def test_raw_ids_paths_and_error_bodies_cannot_be_retained(self):
        self.reject(lambda r: r["run"]["observation"].update(root="ses_raw"))
        self.reject(lambda r: r["run"]["observation"]["forms"].update(location_a="/private/path"))
        self.reject(lambda r: r["run"]["fixture"].update(permission_a="raw-permission"))
        self.reject(lambda r: r["run"].update(failure="PRIVATE_HTTP_BODY"))
        self.reject(lambda r: r["run"]["observation"]["messages"]["types"].__setitem__(1, "PRIVATE_TOOL_OUTPUT"))

    def test_fixture_and_sdk_namespaces_must_agree(self):
        self.reject(lambda r: r["run"]["fixture"].update(root=key("foreign")), "identity mismatch")
        self.reject(lambda r: r["run"]["fixture"].update(form_a=key("foreign-form")), "scope mismatch")
        self.reject(lambda r: r["run"]["fixture"]["children"].__setitem__(0, key("foreign-child")), "identity mismatch")
        self.reject(lambda r: r["run"]["fixture"].update(pending=key("foreign-input")), "identity mismatch")

    def test_location_scoping_and_per_session_pending_request_unwrap_required(self):
        for field in ("permissions", "forms"):
            with self.subTest(field=field):
                self.reject(lambda r: r["run"]["observation"][field].update(b=r["run"]["observation"][field]["a"].copy()), "scope/unwrap")
                self.reject(lambda r: r["run"]["observation"][field].update(session=[]), "scope/unwrap")
                self.reject(lambda r: r["run"]["observation"][field].update(location_b=r["run"]["observation"][field]["location_a"]), "location identities")

    def test_message_traversal_and_cursor_counts_fail_closed(self):
        self.reject(lambda r: r["run"]["observation"]["messages"]["ascending_ids"].reverse(), "traversal")
        self.reject(lambda r: r["run"]["observation"]["messages"].update(ascending_pages=3), "page counts")
        self.reject(lambda r: r["run"]["observation"]["children"].update(pages=True), "child pagination")
        self.reject(lambda r: r["run"]["observation"]["children"]["ids"].__setitem__(1, r["run"]["observation"]["children"]["ids"][0]), "child pagination")

    def test_activity_and_pending_inbox_cannot_be_forged(self):
        self.reject(lambda r: r["run"]["observation"].update(active=[]), "root/active")
        self.reject(lambda r: r["run"]["observation"].update(active=[r["run"]["observation"]["root"]]), "root/active")
        self.reject(lambda r: r["run"]["observation"].update(inbox=[]), "pending inbox")
        self.reject(lambda r: r["run"]["observation"].update(root_outcome="failed"), "root/marker")
        self.reject(lambda r: r["run"]["observation"].update(marker_outcome="interrupted"), "root/marker")
        self.reject(lambda r: r["run"].update(inputs_unchanged=False), "read-only")

    def test_preaborted_invocation_and_physical_budget_accounting_required(self):
        for field, value in (("invocations", 21), ("calls", 22), ("active", 1), ("pending", 1), ("errors", 1), ("errors", False)):
            with self.subTest(field=field, value=value):
                self.reject(lambda r: r["run"]["observation"]["metrics"].update({field: value}), "physical request")
        self.reject(lambda r: r["run"]["observation"].update(preaborted_rejected=False), "cancellation")

    def test_source_runtime_provenance_check_order_and_admission_are_closed(self):
        mutations = (
            lambda r: r.update(production_admission=True),
            lambda r: r.update(connected_transport_qualified=True),
            lambda r: r["run"]["observation"].update(production_admission=True),
            lambda r: r["run"]["observation"].update(connected_transport_qualified=True),
            lambda r: r["run"]["observation"].update(global_fetch_unchanged=False),
            lambda r: r["run"]["observation"]["checks"].reverse(),
            lambda r: r["run"]["provider_requests"].update(blocked=0),
            lambda r: r["sdk_sources"].update({"client.ts": "0" * 64}),
            lambda r: r["artifact_hashes"].update({ARTIFACTS[0]: "0" * 64}),
            lambda r: r.update(bun_sha256="unpinned-bun"),
        )
        for index, mutate in enumerate(mutations):
            with self.subTest(index=index):
                self.reject(mutate)

    def test_partial_or_cleanup_failed_reports_cannot_pass(self):
        report = fixture()
        report["run"]["cleanup"] = False
        validate(report, require_pass=False)
        with self.assertRaises(ValueError):
            validate(report)
        report["run"].update(status="INCOMPLETE", stage="observe", failure="RuntimeError", observation=None)
        validate(report, require_pass=False)
        with self.assertRaises(ValueError):
            validate(report)


class ShapeHarnessSafetyTests(unittest.TestCase):
    def test_source_copy_excludes_ambient_package_and_shadow_modules(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            source, destination = root / "source", root / "copy"
            source.mkdir()
            sources = {"client.ts": b"export {}", "client-error.ts": b"export class ClientError {}"}
            pins = {name: hashlib.sha256(content).hexdigest() for name, content in sources.items()}
            for name, content in sources.items():
                (source / name).write_bytes(content)
            for name in ("package.json", "tsconfig.json", "client-error.js"):
                (source / name).write_text("PRIVATE_AMBIENT_MODULE")
            with patch("sdk_shapes.SDK_SOURCES", pins):
                copy_sources(source, destination)
            self.assertEqual({p.name for p in destination.iterdir()}, set(sources))
            self.assertEqual({p.name: p.read_bytes() for p in destination.iterdir()}, sources)

    def test_wrong_hash_symlink_and_oversized_source_refused_before_copy(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            source = root / "source"
            source.mkdir()
            path = source / "client.ts"
            path.write_bytes(b"wrong-source")
            with self.assertRaisesRegex(ValueError, "tagged_source_pin"):
                copy_sources(source, root / "copy")
            path.unlink(); path.symlink_to(root / "missing")
            with self.assertRaisesRegex(ValueError, "tagged_source_pin"):
                copy_sources(source, root / "copy")
            path.unlink(); path.write_bytes(b"x" * (128 * 1024 + 1))
            with self.assertRaisesRegex(ValueError, "tagged_source_pin"):
                copy_sources(source, root / "copy")
            self.assertFalse((root / "copy").exists())

    def test_wrong_binary_rejected_before_provider_or_process_start(self):
        with patch("sdk_shapes.Provider") as provider, patch("sdk_shapes.ShapesExperiment") as experiment:
            with self.assertRaisesRegex(ValueError, "pinned_absolute_binary"):
                run(Path("relative-opencode"), Path("/unused"), Path("/unused"))
            provider.assert_not_called(); experiment.assert_not_called()

    def test_child_stdout_is_bounded_before_parsing_and_pipes_close_on_failure(self):
        experiment = ShapesExperiment.__new__(ShapesExperiment)
        experiment.root = Path("/private-fixture-root")
        experiment.bun = Path("/private-bun")
        experiment.private = Mock()
        experiment.private.env = {}
        with tempfile.TemporaryFile() as output:
            output.write(b"x" * 9000); output.seek(0)
            child = Mock(stdin=io.BytesIO(), stdout=output)
            experiment.private.remember.return_value = child
            with patch("sdk_shapes.subprocess.Popen", return_value=child), \
                 patch("sdk_shapes.select.select", return_value=([output], [], [])):
                with self.assertRaisesRegex(ValueError, "observer_output_bound"):
                    experiment.execute_observer({"private": "fixture-only"})
            self.assertTrue(child.stdin.closed)
            self.assertTrue(child.stdout.closed)

    def test_fixture_failure_closes_private_processes_and_provider_without_exception_body(self):
        instances, providers = [], []
        def experiment_factory(*_args):
            value = Mock()
            value.report = {"status": "INCOMPLETE", "failure": None, "cleanup": False}
            value.run.side_effect = RuntimeError("PRIVATE_SERVER_BODY")
            value.private.close.side_effect = lambda: value.private.report.update(cleanup=True)
            value.private.report = {"cleanup": False}
            instances.append(value)
            return value
        def provider_factory():
            value = Mock()
            value.close.return_value = True
            value.snapshot.return_value = {"success": 0, "failure": 0, "blocked": 0}
            providers.append(value)
            return value
        pin = read_report(HERE / "pin.json")
        bun = Path("/usr/bin/true").resolve()
        with patch("sdk_shapes.sha", return_value=pin["executable_sha256"]), \
             patch("sdk_shapes.Provider", side_effect=provider_factory), \
             patch("sdk_shapes.ShapesExperiment", side_effect=experiment_factory):
            report = run(Path("/private-fixture-opencode"), bun, Path("/unused-source"))
        self.assertEqual(report["run"]["failure"], "RuntimeError")
        self.assertTrue(report["run"]["cleanup"])
        self.assertNotIn("PRIVATE_", repr(report))
        instances[0].private.close.assert_called_once()
        providers[0].close.assert_called_once()


if __name__ == "__main__":
    unittest.main()
