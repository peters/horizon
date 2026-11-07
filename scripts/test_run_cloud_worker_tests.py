#!/usr/bin/env python3
"""Tests for scripts/run-cloud-worker-tests.sh."""

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "run-cloud-worker-tests.sh"

PASSING = '''
import unittest

class Passing(unittest.TestCase):
    def test_one(self):
        self.assertTrue(True)
'''

FAILING = '''
import unittest

class Failing(unittest.TestCase):
    def test_one(self):
        self.assertEqual(1, 2)
'''

SKIPPING = PASSING + '''
class Skipping(unittest.TestCase):
    @unittest.skip("a prerequisite is missing")
    def test_one(self):
        pass
'''


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.work = Path(tempfile.mkdtemp(prefix="cloud-worker-runner-"))
        self.addCleanup(subprocess.run, ["rm", "-rf", str(self.work)], check=False)
        self.modules = self.work / "modules"
        self.modules.mkdir()

    def write(self, name, text):
        (self.modules / f"{name}.py").write_text(text)

    def run_script(self, *arguments):
        return subprocess.run(
            ["bash", str(SCRIPT), str(self.work / "run.log"), *arguments],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_every_module_runs_and_a_pass_exits_zero(self):
        self.write("test_alpha", PASSING)
        self.write("test_beta", PASSING)
        result = self.run_script("2", "--module-dir", str(self.modules))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Ran 2 tests across 2 modules", result.stdout)
        logs = sorted(path.name for path in (self.work / "run.d").iterdir())
        self.assertEqual(logs, ["test_alpha.log", "test_beta.log"])

    def test_a_failing_module_fails_the_run_and_is_named(self):
        self.write("test_alpha", PASSING)
        self.write("test_beta", FAILING)
        result = self.run_script("2", "--module-dir", str(self.modules))
        self.assertEqual(result.returncode, 1)
        self.assertIn("test_beta.log", result.stderr)
        # The passing module still ran: one failure does not cancel the others.
        self.assertIn("OK", (self.work / "run.d" / "test_alpha.log").read_text())

    def test_a_relative_log_path_resolves_before_the_modules_change_directory(self):
        self.write("test_alpha", PASSING)
        relative = "logs/run.log"
        result = subprocess.run(
            ["bash", str(SCRIPT), relative, "1", "--module-dir", str(self.modules)],
            cwd=self.work,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.work / "logs" / "run.log").is_file())

    def test_a_failure_prints_its_traceback_in_the_run_output(self):
        self.write("test_beta", FAILING)
        result = self.run_script("1", "--module-dir", str(self.modules))
        self.assertEqual(result.returncode, 1)
        self.assertIn("AssertionError", result.stderr, "the CI log holds the traceback, not only a file")

    def test_a_run_that_found_no_module_is_not_a_pass(self):
        result = self.run_script("1", "--module-dir", str(self.modules))
        self.assertNotEqual(result.returncode, 0, "an empty folder must not look like a passing suite")
        self.assertNotEqual(result.returncode, 3)

    def test_a_skipped_test_gets_its_own_exit_code(self):
        self.write("test_alpha", SKIPPING)
        result = self.run_script("2", "--module-dir", str(self.modules))
        self.assertEqual(result.returncode, 3, result.stdout)


if __name__ == "__main__":
    unittest.main()
