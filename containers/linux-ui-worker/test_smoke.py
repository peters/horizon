"""Local process and isolation regressions; no Docker, display or cloud required."""

import importlib.util
import os
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import MagicMock, patch

spec = importlib.util.spec_from_file_location("ui_smoke", Path(__file__).with_name("smoke.py"))
smoke_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke_module)


class SmokeTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.smoke = smoke_module.Smoke(Path("/bin/true"), self.root, 30)
        self.addCleanup(self.smoke.cleanup)

    def test_candidate_environment_does_not_inherit_session_or_credentials(self):
        with patch.dict(os.environ, {
            "HORIZON": "1", "GH_TOKEN": "synthetic", "DISPLAY": ":0",
            "CODEX_HOME": "/other/session", "LD_PRELOAD": "/other/library",
        }):
            environment = self.smoke.command(["/usr/bin/env"]).stdout.splitlines()
        self.assertNotIn("HORIZON=1", environment)
        self.assertFalse(any("synthetic" in line or "/other/" in line for line in environment))
        self.assertFalse(any(line.startswith("DISPLAY=") for line in environment))
        self.assertIn("HOME=" + str(self.root / "home"), environment)
        self.assertEqual((self.root / "runtime").stat().st_mode & 0o777, 0o700)

    def test_nonzero_tool_result_is_not_success(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.smoke.command(["/bin/false"])

    def test_command_failure_retains_bounded_private_diagnostics(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.smoke.command(["/bin/bash", "-c", "printf 'token=synthetic-secret\\ninvalid display' >&2; exit 7"])
        result = json.loads((self.root / "command-failure.json").read_text())
        self.assertEqual(result["returncode"], 7)
        self.assertEqual(result["tool"], "bash")
        self.assertIn("invalid display", result["stderr"])
        self.assertNotIn("synthetic-secret", result["stderr"])
        self.assertLessEqual(len(result["stderr"]), 2048)

    def test_fit_failure_reports_fit_stage_instead_of_previous_wait(self):
        self.smoke.stage = "previous wait"
        self.smoke.window = "123"
        with patch.object(smoke_module.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "xdotool", stderr="missing window")):
            with self.assertRaises(subprocess.CalledProcessError):
                self.smoke.fit()
        result = json.loads((self.root / "command-failure.json").read_text())
        self.assertEqual(result["stage"], "fit workspace")

    def test_expired_deadline_cannot_run_another_command(self):
        self.smoke.deadline = 0
        with self.assertRaises(TimeoutError):
            self.smoke.command(["/bin/touch", str(self.root / "unexpected")])
        self.assertFalse((self.root / "unexpected").exists())

    def test_process_exit_fails_even_when_window_predicate_would_pass(self):
        child = self.smoke.spawn("early-exit", ["/bin/false"])
        child.wait(timeout=5)
        with self.assertRaises(RuntimeError):
            self.smoke.wait_for("window", lambda: True)

    def test_cleanup_keeps_unrelated_process_alive(self):
        unrelated = subprocess.Popen(["/bin/sleep", "30"])
        try:
            owned = self.smoke.spawn("owned", ["/bin/sleep", "30"])
            self.assertTrue(self.smoke.cleanup())
            self.assertIsNotNone(owned.poll())
            self.assertIsNone(unrelated.poll())
        finally:
            unrelated.terminate()
            unrelated.wait(timeout=5)

    def test_forced_candidate_cleanup_cannot_count_as_normal_close(self):
        self.smoke.app = self.smoke.spawn("candidate", ["/bin/sleep", "30"])
        self.assertFalse(self.smoke.cleanup())
        self.assertNotIn("normal_window_close", self.smoke.checks)

    def test_existing_artifact_directory_is_not_reused(self):
        sentinel = self.root / "existing-proof"
        sentinel.write_text("preserve me")
        with patch("sys.argv", ["smoke", "--binary", "/bin/true", "--artifacts", str(self.root)]):
            with patch.object(smoke_module.shutil, "which", return_value="/unused/tool"):
                with self.assertRaises(FileExistsError):
                    smoke_module.main()
        self.assertEqual(sentinel.read_text(), "preserve me")

    def test_candidate_snapshot_survives_concurrent_build_replacement(self):
        binary = self.root / "build-output"
        binary.write_text("#!/bin/sh\nprintf original")
        candidate = smoke_module.snapshot_candidate(binary, self.root)
        replacement = self.root / "replacement"
        replacement.write_text("#!/bin/sh\nprintf replacement")
        replacement.replace(binary)
        self.assertEqual(subprocess.check_output([str(candidate)]), b"original")
        self.assertEqual(candidate.stat().st_mode & 0o777, 0o500)
        with candidate.open("rb") as source:
            digest = smoke_module.hashlib.file_digest(source, "sha256").hexdigest()
        self.assertEqual(digest, smoke_module.hashlib.sha256(b"#!/bin/sh\nprintf original").hexdigest())

    def test_cleanup_only_failure_records_stage_and_removes_snapshot(self):
        root = self.root / "cleanup-failure"
        fake = MagicMock()
        fake.checks = ["normal_window_close"]
        fake.browser = None
        fake.cleanup.return_value = False
        with patch("sys.argv", ["smoke", "--binary", "/bin/true", "--artifacts", str(root)]), \
                patch.object(smoke_module.shutil, "which", return_value="/unused/tool"), \
                patch.object(smoke_module, "Smoke", return_value=fake), \
                patch.object(smoke_module.processes, "adopt_orphans"), \
                patch.object(smoke_module.signal, "signal"):
            self.assertEqual(smoke_module.main(), 1)
        result = json.loads((root / "result.json").read_text())
        self.assertEqual(result["failed_stage"], "cleanup")
        self.assertFalse(result["cleanup_complete"])
        self.assertEqual(result["checks"], ["normal_window_close"])
        self.assertFalse((root / "candidate-horizon").exists())

    def test_supervisor_refusal_preserves_failure_receipt_and_removes_snapshot(self):
        root = self.root / "supervisor-failure"
        with patch("sys.argv", ["smoke", "--binary", "/bin/true", "--artifacts", str(root)]), \
                patch.object(smoke_module.shutil, "which", return_value="/unused/tool"), \
                patch.object(smoke_module.processes, "adopt_orphans", side_effect=OSError("denied")):
            self.assertEqual(smoke_module.main(), 1)
        result = json.loads((root / "result.json").read_text())
        self.assertEqual(result["failed_stage"], "supervisor setup")
        self.assertFalse((root / "candidate-horizon").exists())

    def test_cleanup_exception_keeps_primary_failure_and_attempts_finalization(self):
        root = self.root / "cleanup-exception"
        with patch("sys.argv", ["smoke", "--binary", "/bin/true", "--artifacts", str(root)]), \
                patch.object(smoke_module.shutil, "which", return_value="/unused/tool"), \
                patch.object(smoke_module.processes, "adopt_orphans", side_effect=OSError("denied")), \
                patch.object(smoke_module.Smoke, "cleanup", side_effect=OSError("pidfd refused")):
            self.assertEqual(smoke_module.main(), 1)
        result = json.loads((root / "result.json").read_text())
        self.assertEqual(result["failed_stage"], "supervisor setup")
        self.assertEqual(result["cleanup_error"], "OSError")
        self.assertFalse(result["cleanup_complete"])
        self.assertFalse((root / "candidate-horizon").exists())

    def test_subreaper_cleans_children_after_their_leader_exits(self):
        self.assert_orphan_cleanup("sleep 30 &")

    def test_subreaper_cleans_separately_sessioned_children(self):
        self.assert_orphan_cleanup("setsid sleep 30 &")

    def assert_orphan_cleanup(self, command):
        # A separate supervisor is essential: adoption must not affect the test
        # runner or claim its other subprocesses as smoke-owned descendants.
        program = """
import subprocess
import processes
processes.adopt_orphans()
parent = subprocess.Popen(['/bin/bash', '--noprofile', '--norc', '-c', COMMAND])
parent.wait(timeout=5)
assert processes.children(), 'fixture did not retain an orphan'
assert processes.cleanup_descendants()
assert not processes.children(), 'owned descendants survived cleanup'
""".replace("COMMAND", repr(command))
        subprocess.run(
            [sys.executable, "-c", program], check=True,
            cwd=Path(__file__).parent, timeout=15,
        )


if __name__ == "__main__":
    unittest.main()
