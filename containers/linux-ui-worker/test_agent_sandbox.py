"""Offline regressions for sandbox qualification; never use ambient agent auth."""

import importlib.util
import os
import json
from pathlib import Path
import subprocess
import signal
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("sandbox_smoke", Path(__file__).with_name("agent-sandbox-smoke.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class SandboxTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)

    def test_plain_execution_fails_outside_write_check(self):
        workspace, outside = self.root / "workspace", self.root / "outside"
        workspace.mkdir()
        outside.mkdir()
        (outside / "existing").write_text("unchanged\n")
        result = subprocess.run([sys.executable, "-I", "-c", smoke.CANARY, str(workspace), str(outside)],
                                capture_output=True, timeout=5)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn(b"WORKER_SANDBOX_CANARY_PASSED", result.stdout)

    def test_zero_exit_without_canary_is_not_proof(self):
        with patch.object(smoke, "command", return_value=""):
            with self.assertRaisesRegex(RuntimeError, "did not complete"):
                smoke.qualify(self.root, "/unused/agent")

    def test_success_marker_without_retained_write_is_not_proof(self):
        with patch.object(smoke, "command", side_effect=["test-version", "WORKER_SANDBOX_CANARY_PASSED"]):
            with self.assertRaisesRegex(RuntimeError, "not retained"):
                smoke.qualify(self.root, "/unused/agent")

    def test_outside_mutation_is_not_proof(self):
        def fake_command(argv, env, cwd):
            if "--version" in argv:
                return "test-version"
            (cwd / "allowed").write_text("sandbox write\n")
            (self.root / "outside" / "existing").write_text("changed\n")
            return "WORKER_SANDBOX_CANARY_PASSED"
        with patch.object(smoke, "command", side_effect=fake_command):
            with self.assertRaisesRegex(RuntimeError, "outside files changed"):
                smoke.qualify(self.root, "/unused/agent")

    def test_commands_do_not_inherit_agent_auth_or_session_environment(self):
        environments = []
        def fake_command(argv, env, cwd):
            environments.append(env)
            if "--version" in argv:
                return "test-version"
            (cwd / "allowed").write_text("sandbox write\n")
            return "WORKER_SANDBOX_CANARY_PASSED"
        with patch.dict(os.environ, {"GH_TOKEN": "synthetic-secret", "CODEX_HOME": "/ambient", "HORIZON": "1"}):
            with patch.object(smoke, "command", side_effect=fake_command):
                result = smoke.qualify(self.root, "/unused/agent")
        self.assertTrue(result["passed"])
        for environment in environments:
            self.assertNotIn("GH_TOKEN", environment)
            self.assertNotIn("HORIZON", environment)
            self.assertEqual(environment["CODEX_HOME"], str(self.root / "home" / ".codex"))

    def test_nonzero_command_is_not_proof(self):
        with self.assertRaisesRegex(RuntimeError, "command failed"):
            smoke.command(["/bin/false"], {}, self.root)

    def test_stdin_is_closed(self):
        self.assertEqual(smoke.command(["/bin/cat"], {}, self.root), "")

    def test_excessive_output_fails_quickly(self):
        with self.assertRaisesRegex(RuntimeError, "excessive output"):
            smoke.command([sys.executable, "-I", "-c", "print('x' * 4097)"], {}, self.root)

    def test_timeout_preserves_unrelated_process(self):
        unrelated = subprocess.Popen(["/bin/sleep", "30"])
        try:
            start = time.monotonic()
            with self.assertRaisesRegex(RuntimeError, "timed out"):
                smoke.command(["/bin/sleep", "30"], {}, self.root, timeout=0.05)
            self.assertLess(time.monotonic() - start, 5)
            self.assertIsNone(unrelated.poll())
        finally:
            unrelated.terminate()
            unrelated.wait(timeout=5)

    def test_termination_cleans_owned_orphan_and_private_files(self):
        marker = self.root / "owned-pid"
        fake = self.root / "fake-agent"
        fake.write_text(f'''#!{sys.executable}
import pathlib, subprocess, sys, time
if "--version" in sys.argv:
    print("test-version")
else:
    child = subprocess.Popen(["/bin/sleep", "30"], start_new_session=True)
    pathlib.Path({str(marker)!r}).write_text(str(child.pid))
    time.sleep(30)
''')
        fake.chmod(0o700)
        launcher = f'''
import importlib.util, pathlib, tempfile
spec = importlib.util.spec_from_file_location("probe", {str(Path(smoke.__file__).resolve())!r})
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)
probe.shutil.which = lambda *args, **kwargs: {str(fake)!r}
original = tempfile.TemporaryDirectory
probe.tempfile.TemporaryDirectory = lambda **kwargs: original(prefix="owned-", dir={str(self.root)!r})
raise SystemExit(probe.main())
'''
        environment = {"PATH": "/usr/bin:/bin", "PYTHONPATH": str(Path(smoke.__file__).parent)}
        unrelated = subprocess.Popen(["/bin/sleep", "30"])
        supervisor = subprocess.Popen([sys.executable, "-c", launcher], env=environment,
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 5
            while not marker.exists() and time.monotonic() < deadline:
                time.sleep(0.02)
            self.assertTrue(marker.exists(), "owned child never started")
            os.kill(supervisor.pid, signal.SIGTERM)
            stdout, stderr = supervisor.communicate(timeout=15)
            self.assertEqual(supervisor.returncode, 1, stderr.decode())
            self.assertFalse(json.loads(stdout)["passed"])
            with self.assertRaises(ProcessLookupError):
                os.kill(int(marker.read_text()), 0)
            self.assertEqual(list(self.root.glob("owned-*/")), [])
            self.assertIsNone(unrelated.poll())
        finally:
            if supervisor.poll() is None:
                supervisor.kill()
                supervisor.wait(timeout=5)
            unrelated.terminate()
            unrelated.wait(timeout=5)


if __name__ == "__main__":
    unittest.main()
