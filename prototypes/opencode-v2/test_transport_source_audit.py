"""Offline source audit/evidence safety tests; no network or OpenCode launch."""
import copy
import hashlib
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from smoke import HERE, sha
from transport_source_audit import ARTIFACTS, audit, read_source, validate
from verify_evidence import read_report


def fixture():
    report = copy.deepcopy(read_report(HERE / "transport-source-results.json"))
    report["artifact_hashes"] = {name: sha(HERE / name) for name in ARTIFACTS}
    return report


class SourceEvidenceTests(unittest.TestCase):
    def reject(self, mutate):
        report = fixture()
        mutate(report)
        with self.assertRaises(ValueError):
            validate(report)

    def test_retained_source_audit_passes_but_p0_does_not(self):
        result = validate(read_report(HERE / "transport-source-results.json"))
        self.assertEqual((result["sources"], result["facts"], result["p0_status"]), (7, 8, "BLOCKED"))
        self.assertIs(result["production_admission"], False)
        self.assertIs(result["connected_transport_qualified"], False)

    def test_scope_and_admission_cannot_be_promoted(self):
        for field, value in (("p0_status", "PASS"), ("production_admission", True),
                             ("connected_transport_qualified", True), ("evidence_kind", "actual-cli-transport"),
                             ("status", "P0_PASS"), ("schema", True)):
            with self.subTest(field=field):
                self.reject(lambda r: r.update({field: value}))

    def test_facts_cannot_be_removed_relabeled_or_invented(self):
        for mutate in (
            lambda r: r["facts"].pop(),
            lambda r: r["facts"].reverse(),
            lambda r: r["facts"][0].update(observed=False),
            lambda r: r["facts"][0].update(observed=1),
            lambda r: r["facts"][0].update(name="factory_available"),
            lambda r: r["context_fields"].append("observation"),
            lambda r: r["request_fields"].append("fetch"),
            lambda r: r["remaining_blockers"].pop(),
        ):
            self.reject(mutate)

    def test_unknown_fields_reject_source_bodies_credentials_and_paths(self):
        for path in ((), ("sources",), ("target",), ("artifact_hashes",), ("facts", 0)):
            def mutate(report):
                value = report
                for part in path:
                    value = value[part]
                value["private_body"] = "PRIVATE_SENTINEL"
            with self.subTest(path=path):
                self.reject(mutate)

    def test_source_and_artifact_pins_cannot_change(self):
        self.reject(lambda r: r["sources"].update({next(iter(r["sources"])): "0" * 64}))
        self.reject(lambda r: r["artifact_hashes"].update({ARTIFACTS[0]: "0" * 64}))
        self.reject(lambda r: r["target"].update(version="2.0.33"))


class SourceReadSafetyTests(unittest.TestCase):
    def test_exact_bytes_required_without_importing_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            source = root / "fixture.ts"
            raw = b'throw new Error("PRIVATE_SENTINEL")\n'
            source.write_bytes(raw)
            digest = hashlib.sha256(raw).hexdigest()
            self.assertEqual(read_source(root, source.name, digest), raw.decode())
            source.write_bytes(raw + b"\n")
            with self.assertRaisesRegex(ValueError, "source pin"):
                read_source(root, source.name, digest)

    def test_oversized_source_rejected_before_decode(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            (root / "fixture.ts").write_bytes(b"\xff" * (128 * 1024 + 1))
            with self.assertRaisesRegex(ValueError, "source byte bound"):
                read_source(root, "fixture.ts", "0" * 64)

    def test_symlink_cannot_supply_even_correct_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            source = root / "real.ts"
            source.write_bytes(b"fixture")
            (root / "link.ts").symlink_to(source)
            with self.assertRaisesRegex(ValueError, "canonical source path"):
                read_source(root, "link.ts", hashlib.sha256(b"fixture").hexdigest())

    def test_fifo_rejected_without_blocking_or_reading(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            os.mkfifo(root / "fixture.ts")
            with self.assertRaisesRegex(ValueError, "regular source"):
                read_source(root, "fixture.ts", "0" * 64)

    def test_directory_and_missing_source_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            (root / "fixture.ts").mkdir()
            with self.assertRaises(ValueError):
                read_source(root, "fixture.ts", "0" * 64)
            with self.assertRaises(OSError):
                read_source(root, "absent.ts", "0" * 64)

    def test_relative_root_rejected_before_any_source_read(self):
        with patch("transport_source_audit.read_source") as reader:
            with self.assertRaisesRegex(ValueError, "canonical source directory"):
                audit(Path("relative"))
            reader.assert_not_called()


if __name__ == "__main__":
    unittest.main()
