#!/usr/bin/env python3
"""Synthetic progress and process ownership checks; no real shard or cloud."""

import importlib.util
import errno
import io
import json
import os
from pathlib import Path
import signal
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

SCRIPT = Path(__file__).with_name("ci-test-watchdog.py")
SPEC = importlib.util.spec_from_file_location("ci_test_watchdog", SCRIPT)
WATCHDOG = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(WATCHDOG)


class ProgressTests(unittest.TestCase):
    def test_compilation_does_not_arm_the_test_idle_clock(self):
        progress = WATCHDOG.Progress()
        progress.line("Compiling horizon-ui", 1)
        self.assertFalse(progress.stalled(600, 300))
        progress.line("running 2 tests", 600)
        self.assertFalse(progress.stalled(899, 300))
        self.assertTrue(progress.stalled(900, 300))

    def test_only_completed_tests_reset_progress(self):
        progress = WATCHDOG.Progress()
        progress.line("running 2 tests", 0)
        progress.line("test fixture::slow has been running for over 60 seconds", 60)
        progress.line("heartbeat", 299)
        self.assertTrue(progress.stalled(300, 300))
        progress.line("\x1b[32mtest fixture::fast ... ok\x1b[0m", 301)
        self.assertFalse(progress.stalled(600, 300))
        self.assertEqual(progress.completed, 1)
        self.assertEqual(progress.last_completed, "fixture::fast")

    def test_nested_start_notice_cannot_keep_a_hung_shard_alive(self):
        progress = WATCHDOG.Progress()
        progress.line("running 10 tests", 0)
        progress.line("running 1 test", 299)
        self.assertTrue(progress.stalled(300, 300))

    def test_stack_command_disables_variable_output_and_auto_loading(self):
        command = WATCHDOG.stack_command(123)
        self.assertIn("set print frame-arguments none", command)
        self.assertIn("set print entry-values no", command)
        self.assertIn("set auto-load off", command)
        self.assertIn("set debuginfod enabled off", command)
        self.assertIn("thread apply all bt 64", command)
        self.assertNotIn("full", " ".join(command))


@unittest.skipUnless(sys.platform == "linux" and hasattr(signal, "pidfd_send_signal"), "Linux pidfds required")
class OwnedProcessTests(unittest.TestCase):
    def run_fixture(self, code, *, idle=0.35, total=3, capture=None):
        root = Path(self.temp.name)
        out = root / f"run-{len(list(root.iterdir()))}"
        output = io.BytesIO()
        result = WATCHDOG.run([sys.executable, "-u", "-c", code], out, idle, total,
                              root, capture_fn=capture or self.capture, stream=output)
        return result, out, output.getvalue()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.capture_calls = []

    def capture(self, scope, progress, reason, output, workspace):
        states = [scope.current(pid) for pid in scope.records if scope.current(pid)]
        self.capture_calls.append((reason, states))
        (output / "diagnostics.json").write_text(json.dumps({"reason": reason, "processes": states}))

    def test_child_status_and_large_final_output_are_preserved(self):
        result, _, output = self.run_fixture("import sys; print('x'*100000); sys.exit(7)")
        self.assertEqual(result, 7)
        self.assertEqual(output, b"x" * 100000 + b"\n")
        self.assertEqual(self.capture_calls, [])

    def test_quiet_compile_finishes_before_a_test_stall_clock_exists(self):
        code = "import time; time.sleep(.5); print('running 1 test'); print('test fixture::done ... ok')"
        result, _, _ = self.run_fixture(code)
        self.assertEqual(result, 0)
        self.assertEqual(self.capture_calls, [])

    def test_hang_captures_live_owned_children_and_preserves_unrelated_process(self):
        sentinel = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"], start_new_session=True)
        self.addCleanup(self.stop_child, sentinel)
        child_file = Path(self.temp.name) / "child.json"
        code = ("import subprocess,sys,time,json,pathlib; "
                "child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(30)'],start_new_session=True); "
                f"pathlib.Path({str(child_file)!r}).write_text(json.dumps(child.pid)); "
                "print('running 1 test',flush=True); time.sleep(30)")
        result, _, _ = self.run_fixture(code)
        self.assertEqual(result, 124)
        child_pid = json.loads(child_file.read_text())
        self.assertIn(child_pid, [item["pid"] for item in self.capture_calls[0][1]])
        self.assertIsNone(sentinel.poll())
        current = WATCHDOG.identity(child_pid)
        self.assertTrue(current is None or current["state"] == "Z")

    def test_overall_deadline_captures_a_quiet_startup(self):
        result, _, _ = self.run_fixture("import time; time.sleep(30)", total=.3)
        self.assertEqual(result, 124)
        self.assertEqual(self.capture_calls[0][0], "job deadline")

    def test_identity_change_prevents_a_signal(self):
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        self.addCleanup(self.stop_child, child)
        scope = WATCHDOG.OwnedProcesses(child.pid)
        self.addCleanup(scope.close)
        actual = WATCHDOG.identity(child.pid)
        with mock.patch.object(WATCHDOG, "identity", return_value={**actual, "start": actual["start"] + 1}):
            with mock.patch.object(signal, "pidfd_send_signal") as send:
                scope.send(child.pid, signal.SIGTERM)
                send.assert_not_called()

    def test_dead_identity_releases_its_descriptor_before_the_shard_ends(self):
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        self.addCleanup(self.stop_child, child)
        scope = WATCHDOG.OwnedProcesses(child.pid)
        self.addCleanup(scope.close)
        fd = scope.records[child.pid][1]
        self.stop_child(child)
        scope.observe()
        self.assertEqual(scope.records, {})
        with self.assertRaises(OSError) as error:
            os.fstat(fd)
        self.assertEqual(error.exception.errno, errno.EBADF)

    def test_pruned_identity_permits_a_new_record_for_the_same_pid(self):
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        self.addCleanup(self.stop_child, child)
        scope = WATCHDOG.OwnedProcesses(child.pid)
        self.addCleanup(scope.close)
        actual = WATCHDOG.identity(child.pid)
        with mock.patch.object(WATCHDOG, "identity", return_value={**actual, "start": actual["start"] + 1}):
            scope.prune()
        self.assertEqual(scope.records, {})
        scope._retain(actual)
        self.assertEqual(scope.records[child.pid][0], actual["start"])

    def test_descriptor_exhaustion_is_an_explicit_bookkeeping_failure(self):
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        self.addCleanup(self.stop_child, child)
        with mock.patch.object(os, "pidfd_open", side_effect=OSError(errno.EMFILE, "fixture exhaustion")):
            with self.assertRaisesRegex(WATCHDOG.BookkeepingError, "errno 24"):
                WATCHDOG.OwnedProcesses(child.pid)

    def test_initial_bookkeeping_failure_stops_the_direct_owned_child(self):
        with mock.patch.object(os, "pidfd_open", side_effect=OSError(errno.EMFILE, "fixture exhaustion")):
            result, out, _ = self.run_fixture("import time; time.sleep(30)")
        self.assertEqual(result, 125)
        evidence = json.loads((out / "diagnostics.json").read_text())
        self.assertIn("errno 24", evidence["reason"])
        self.assertEqual(self.capture_calls, [])

    def test_observation_failure_records_the_error_and_cleans_known_identities(self):
        with mock.patch.object(WATCHDOG.OwnedProcesses, "observe", side_effect=WATCHDOG.BookkeepingError("fixture failure")):
            with mock.patch.object(WATCHDOG, "tool_version", return_value=["fixture"]):
                result, out, _ = self.run_fixture("import time; time.sleep(30)", capture=WATCHDOG.capture)
        self.assertEqual(result, 125)
        evidence = json.loads((out / "diagnostics.json").read_text())
        self.assertEqual(evidence["bookkeeping_error"], "fixture failure")
        for item in evidence["processes"]:
            current = WATCHDOG.identity(item["pid"])
            self.assertTrue(current is None or current["state"] == "Z")

    def test_diagnostics_do_not_read_environment_arguments_or_full_stack_memory(self):
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        self.addCleanup(self.stop_child, child)
        scope = WATCHDOG.OwnedProcesses(child.pid)
        self.addCleanup(scope.close)
        out = Path(self.temp.name) / "allowlist"
        out.mkdir()
        original = Path.read_text

        def guarded(path, *args, **kwargs):
            self.assertNotIn(path.name, {"environ", "cmdline", "mem"})
            return original(path, *args, **kwargs)

        with mock.patch.object(Path, "read_text", guarded):
            with mock.patch.object(WATCHDOG, "tool_version", return_value=["fixture"]):
                WATCHDOG.capture(scope, WATCHDOG.Progress(), "fixture", out, Path(self.temp.name))
        evidence = json.loads((out / "diagnostics.json").read_text())
        self.assertIn("memory", evidence)
        self.assertNotIn("environment", evidence)
        self.assertNotIn("arguments", evidence)

    def test_stack_stub_receives_privacy_settings_before_the_backtrace(self):
        workspace = Path(self.temp.name)
        exe = workspace / "target/debug/deps/horizon-cafef00d"
        exe.parent.mkdir(parents=True)
        shutil.copy2(Path(sys.executable).resolve(), exe)
        child = subprocess.Popen([str(exe), "-c", "import time; time.sleep(30)"])
        self.addCleanup(self.stop_child, child)
        scope = WATCHDOG.OwnedProcesses(child.pid)
        self.addCleanup(scope.close)
        out = workspace / "stack-stub"
        out.mkdir()
        calls = []

        def debugger_stub(command, **kwargs):
            calls.append(command)
            before = command[:command.index("thread apply all bt 64")]
            self.assertIn("set print frame-arguments none", before)
            self.assertIn("set print entry-values no", before)
            self.assertIn("set auto-load off", before)
            self.assertIn("set debuginfod enabled off", before)
            self.assertEqual(WATCHDOG.identity(child.pid)["state"], "T")
            kwargs["stdout"].write("#0 fixture_function ()\n")
            return subprocess.CompletedProcess(command, 0)

        with mock.patch.object(WATCHDOG, "tool_version", return_value=["fixture"]):
            with mock.patch.object(subprocess, "run", side_effect=debugger_stub):
                WATCHDOG.capture(scope, WATCHDOG.Progress(), "fixture", out, workspace)
        self.assertEqual(len(calls), 1)
        self.assertEqual((out / f"stacks-{child.pid}.txt").read_text(), "#0 fixture_function ()\n")

    def test_total_stack_budget_preserves_states_and_skips_later_captures(self):
        workspace = Path(self.temp.name)
        exe = workspace / "target/debug/deps/horizon-cafef00d"
        exe.parent.mkdir(parents=True)
        shutil.copy2(Path(sys.executable).resolve(), exe)
        children = [subprocess.Popen([str(exe), "-c", "import time; time.sleep(30)"]) for _ in range(2)]
        for child in children:
            self.addCleanup(self.stop_child, child)
        scope = WATCHDOG.OwnedProcesses(children[0].pid)
        scope._retain(WATCHDOG.identity(children[1].pid))
        self.addCleanup(scope.close)
        out = workspace / "bounded-stacks"
        out.mkdir()
        clock = [0]
        calls = []

        def debugger_stub(command, **kwargs):
            calls.append(command)
            self.assertEqual(kwargs["timeout"], 20)
            kwargs["stdout"].write("#0 fixture_function ()\n")
            clock[0] = 45
            return subprocess.CompletedProcess(command, 0)

        with mock.patch.object(WATCHDOG, "tool_version", return_value=["fixture"]):
            with mock.patch.object(WATCHDOG.time, "monotonic", side_effect=lambda: clock[0]):
                with mock.patch.object(subprocess, "run", side_effect=debugger_stub):
                    WATCHDOG.capture(scope, WATCHDOG.Progress(), "fixture", out, workspace)
        evidence = json.loads((out / "diagnostics.json").read_text())
        self.assertEqual(len(calls), 1)
        self.assertEqual(len(evidence["processes"]), 2)
        skipped = [item for item in evidence["processes"] if item["stack_exit"] != 0]
        self.assertEqual(skipped[0]["stack_exit"], "total stack capture budget exhausted")
        self.assertIn("threads", skipped[0])

    @staticmethod
    def stop_child(child):
        if child.poll() is None:
            child.send_signal(signal.SIGCONT)
            child.terminate()
            child.wait(timeout=3)


if __name__ == "__main__":
    unittest.main()
