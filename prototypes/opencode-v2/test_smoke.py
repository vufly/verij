"""Deterministic P0 safety/evidence tests; no OpenCode launch or global changes."""
import json
import os
from pathlib import Path
import signal
import tempfile
import unittest
from unittest.mock import patch

from smoke import ARTIFACTS, environment, prepare, records, run, sha, signal_birth
from verify_evidence import BLOCKERS, SHAPES, bounded_json, read_report, validate

HERE = Path(__file__).resolve().parent


def evidence():
    runs = []
    for mode in ("standalone", "server"):
        client = {"pid": 100, "ppid": 99, "pgrp": 100, "session": 100, "tty": 1000, "tpgid": 100, "start": 10000}
        runs.append({"mode": mode, "status": "SMOKE_PASS", "failure": None, "client": client,
                     "listener_owned": True if mode == "server" else None, "cleanup": True,
                     "layout": [{"role": "client", "birth": client.copy()},
                                {"role": "server", "birth": {**client, "pid": 101}}],
                     "frames": [
                         {"kind": "setup", "index": 0, "pid": 100, "version_matches": True,
                          "options_match": True, "shape": dict.fromkeys(SHAPES, True)},
                         {"kind": "route", "index": 1, "pid": 100, "route": "home", "selected": None},
                         {"kind": "cleanup", "index": 2, "pid": 100}]})
    return {"schema": 1, "target": json.loads((HERE / "pin.json").read_text()),
            "artifact_hashes": {name: sha(HERE / name) for name in ARTIFACTS},
            "status": "SMOKE_PASS", "production_admission": False, "blockers": BLOCKERS.copy(), "runs": runs}


class SafetyTests(unittest.TestCase):
    def test_environment_does_not_inherit_secrets_or_runtime_injection(self):
        with patch.dict(os.environ, {"OPENCODE_SERVER": "PRIVATE", "OPENCODE_PASSWORD": "SECRET",
                                    "OPENCODE_CLI_CONFIG_CONTENT": "PRIVATE", "NODE_OPTIONS": "PRIVATE",
                                    "BUN_OPTIONS": "PRIVATE", "HTTP_PROXY": "PRIVATE", "PATH": "PRIVATE",
                                    "VERIJ_STATES_DIR": "PRIVATE", "XDG_CONFIG_DIRS": "PRIVATE"}):
            env = environment(Path("/tmp/private-fixture"))
        self.assertNotIn("PRIVATE", env.values())
        self.assertNotIn("SECRET", env.values())
        self.assertGreaterEqual(len(env["OPENCODE_PASSWORD"]), 40)
        self.assertNotIn("OPENCODE_CLI_CONFIG_CONTENT", env)
        self.assertEqual(env["PATH"], "/usr/bin:/bin")
        self.assertEqual(env["OPENCODE_CONFIG_DIR"], "/tmp/private-fixture/config/opencode")
        self.assertEqual(env["VERIJ_STATES_DIR"], "/tmp/private-fixture/verij/inventory")

    def test_config_is_only_private_cli_config_not_legacy_tui_or_project_config(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            env = prepare(root)
            config = json.loads((Path(env["OPENCODE_CONFIG_DIR"]) / "cli.json").read_text())
            self.assertEqual(config["plugins"][0]["options"], {"fixture": "verij-v2-p0"})
            self.assertEqual(list((root / "work").iterdir()), [])
            self.assertFalse((root / "config/opencode/tui.json").exists())
            plugin_directory = Path(config["plugins"][0]["package"])
            self.assertEqual(plugin_directory, root / "fixture")
            self.assertEqual((plugin_directory / "tui.js").read_bytes(), (HERE / "contract-probe.mjs").read_bytes())

    def test_ancestor_project_resources_refused_before_launch(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for marker in (".opencode", ".git", ".hg", "AGENTS.md"):
                path = root / marker
                path.write_text("PRIVATE")
                child = root / "scratch"
                child.mkdir()
                try:
                    with self.subTest(marker=marker), self.assertRaisesRegex(ValueError, "ambient"):
                        prepare(child)
                finally:
                    path.unlink()
                    child.rmdir()

    def test_wrong_binary_rejected_before_any_launch(self):
        with tempfile.NamedTemporaryFile() as binary, patch("smoke.subprocess.Popen") as launch:
            with self.assertRaisesRegex(ValueError, "hash"):
                run(Path(binary.name))
            launch.assert_not_called()
        with self.assertRaisesRegex(ValueError, "absolute"):
            run(Path("opencode"))

    def test_launch_failure_cleans_both_private_roots_without_retaining_error_body(self):
        probes = []
        class FailedRun:
            def __init__(self, binary, root, mode):
                self.root = root
                self.report = {"status": "INCOMPLETE", "cleanup": False}
                probes.append(self)
            def launch(self):
                raise RuntimeError("PRIVATE_PROVIDER_BODY")
            def close(self):
                self.report["cleanup"] = True
        pin = json.loads((HERE / "pin.json").read_text())
        with tempfile.NamedTemporaryFile() as stream:
            stream.write(b"\x7fELF")
            stream.flush()
            binary = Path(stream.name)
            with patch("smoke.PrivateRun", FailedRun), patch("smoke.sha", side_effect=lambda path:
                    pin["executable_sha256"] if Path(path) == binary else sha(path)):
                report = run(binary)
        self.assertEqual(len(probes), 2)
        self.assertTrue(all(item.report["cleanup"] and not item.root.exists() for item in probes))
        self.assertEqual(report["status"], "INCOMPLETE")
        self.assertNotIn("PRIVATE_PROVIDER_BODY", json.dumps(report))
        self.assertTrue(all(item["failure"] == "RuntimeError" for item in report["runs"]))

    def test_reused_pid_not_signalled(self):
        old = {"pid": 100, "start": 123}
        with patch("smoke.os.pidfd_open", return_value=42), patch("smoke.os.close") as close, \
                patch("smoke.identity", return_value={"pid": 100, "start": 456}), \
                patch("smoke.signal.pidfd_send_signal") as send:
            signal_birth(old, signal.SIGTERM)
            send.assert_not_called()
            close.assert_called_once_with(42)

    def test_signal_is_pidfd_bound_and_birth_checked(self):
        birth = {"pid": 100, "start": 123}
        with patch("smoke.os.pidfd_open", return_value=42), patch("smoke.os.close"), \
                patch("smoke.identity", return_value=birth), patch("smoke.signal.pidfd_send_signal") as send:
            signal_birth(birth, signal.SIGTERM)
            send.assert_called_once_with(42, signal.SIGTERM)

    def test_trace_reader_bounds_and_ignores_partial_append(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "probe.ndjson"
            path.write_bytes(b'{"kind":"cleanup"}\n{"kind":')
            self.assertEqual(records(path), [{"kind": "cleanup"}])
            path.write_bytes(b"[" * 33 + b"0" + b"]" * 33 + b"\n")
            with self.assertRaisesRegex(ValueError, "depth bound"):
                records(path)
            path.write_bytes(b" " * (128 * 1024 + 1))
            with self.assertRaisesRegex(ValueError, "trace_bound"):
                records(path)


class EvidenceTests(unittest.TestCase):
    def test_retained_smoke_matches_current_fixture(self):
        result = validate(read_report(HERE / "smoke-results.json"))
        self.assertEqual(result["status"], "SMOKE_PASS")
        self.assertFalse(result["p0_gate_passed"])

    def test_complete_smoke_is_never_p0_or_production_qualification(self):
        result = validate(evidence())
        self.assertEqual(result["status"], "SMOKE_PASS")
        self.assertFalse(result["production_admission"])
        self.assertFalse(result["p0_gate_passed"])

    def test_unknown_fields_rejected_at_every_layer(self):
        for location in ((), ("target",), ("runs", 0), ("runs", 0, "client"),
                         ("runs", 0, "frames", 0), ("runs", 0, "frames", 0, "shape"),
                         ("runs", 0, "layout", 0), ("runs", 0, "layout", 0, "birth")):
            report = evidence()
            target = report
            for key in location:
                target = target[key]
            target["prompt"] = "PRIVATE_CONTENT_SENTINEL"
            with self.subTest(location=location), self.assertRaises(ValueError):
                validate(report)

    def test_forged_authority_and_missing_proofs_rejected(self):
        mutations = [lambda r: r.update(production_admission=True), lambda r: r.update(blockers=[]),
                     lambda r: r["runs"].pop(), lambda r: r["runs"][0].update(cleanup=False),
                     lambda r: r["runs"][0].update(client=None),
                     lambda r: r["runs"][1].update(listener_owned=None),
                     lambda r: r["runs"][0].update(layout=[]),
                     lambda r: r["runs"][0]["client"].update(tpgid=101),
                     lambda r: r["runs"][0]["frames"].pop(),
                     lambda r: r["runs"][0]["frames"][0]["shape"].update(message_list=False),
                     lambda r: r["runs"][0]["frames"][1].update(selected="PRIVATE_SESSION"),
                     lambda r: r["runs"][0]["frames"][1].update(route="PRIVATE_CONTENT"),
                     lambda r: r["runs"][0].update(failure="PRIVATE_ERROR_BODY"),
                     lambda r: r["artifact_hashes"].update({"contract-probe.mjs": "0" * 64})]
        for mutate in mutations:
            report = evidence()
            mutate(report)
            with self.subTest(mutation=mutate), self.assertRaises(ValueError):
                validate(report)

    def test_incomplete_evidence_can_be_written_but_not_pass(self):
        report = evidence()
        report["status"] = report["runs"][0]["status"] = "INCOMPLETE"
        report["runs"][0].update(frames=[], client=None, failure="TimeoutError")
        validate(report, require_pass=False)
        with self.assertRaisesRegex(ValueError, "incomplete"):
            validate(report)

    def test_cleanup_failure_cannot_be_promoted(self):
        report = evidence()
        report["status"] = "INCOMPLETE"
        report["runs"][0]["cleanup"] = False
        validate(report, require_pass=False)
        with self.assertRaisesRegex(ValueError, "incomplete"):
            validate(report)

    def test_duplicate_evidence_fields_rejected_before_validation(self):
        with self.assertRaisesRegex(ValueError, "duplicate"):
            bounded_json(b'{"status":"INCOMPLETE","status":"SMOKE_PASS"}')

    def test_preparse_byte_depth_and_structural_limits(self):
        for raw in (b" " * (128 * 1024 + 1), b"[" * 33 + b"0" + b"]" * 33,
                    b"[" + b"0," * 16384 + b"0]"):
            with self.subTest(length=len(raw)), self.assertRaisesRegex(ValueError, "bound"):
                bounded_json(raw)
        raw = json.dumps({"text": '[{,:' * 100, "escaped": '\\"'}).encode()
        self.assertEqual(bounded_json(raw), json.loads(raw))


if __name__ == "__main__":
    unittest.main()
