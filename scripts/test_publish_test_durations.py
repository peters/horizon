#!/usr/bin/env python3
"""Tests for scripts/publish-test-durations.py."""

import importlib.util
import io
import json
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "publish-test-durations.py"


def load_module():
    """Import the hyphenated script by path."""
    spec = importlib.util.spec_from_file_location("publish_test_durations", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


durations = load_module()


def test_event(name, exec_time, status="ok"):
    return json.dumps({"type": "test", "event": status, "name": name, "exec_time": exec_time})


class ReadTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)

    def write(self, lines):
        path = Path(self.directory.name) / "stream.json"
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return path

    def test_reads_one_time_per_finished_test(self):
        path = self.write([
            json.dumps({"type": "suite", "event": "started"}),
            test_event("horizon-core::horizon_core$board::moves", 0.5),
        ])
        self.assertEqual(durations.read_times(path), {"horizon-core::horizon_core$board::moves": 0.5})

    def test_skips_started_ignored_and_noise(self):
        path = self.write([
            "noise",
            "{broken",
            test_event("a$b::started", 9.0, status="started"),
            test_event("a$b::ignored", 9.0, status="ignored"),
            test_event("a$b::kept", 1.0),
        ])
        self.assertEqual(durations.read_times(path), {"a$b::kept": 1.0})

    def test_a_missing_stream_has_no_times(self):
        self.assertEqual(durations.read_times(Path(self.directory.name) / "absent"), {})

    def test_the_crate_is_the_package_of_either_name_shape(self):
        # nextest writes `<package>::<binary>$<path>`, and a package with one test
        # binary can appear as `<package>$<path>`.
        self.assertEqual(durations.crate_of("horizon-core::horizon_core$board::moves"), "horizon-core")
        self.assertEqual(durations.crate_of("horizon-core$board::moves"), "horizon-core")


class ReportTests(unittest.TestCase):
    def test_reports_the_total_the_count_and_the_crates(self):
        times = {
            "horizon-ui::horizon_ui$b::two": 40.0,
            "horizon-core::horizon_core$a::one": 12.5,
        }
        report = durations.render(times, "Linux x64 libs", 10)
        self.assertIn("### Test durations, Linux x64 libs", report)
        self.assertIn("| tests | 2 |", report)
        self.assertIn("**0.9 min** (52 s)", report)
        self.assertIn("| horizon-ui | 1 | 40.0 s |", report)
        self.assertLess(report.index("horizon-ui"), report.index("horizon-core"))

    def test_names_the_slowest_tests_in_order(self):
        times = {"a$a::slow": 30.0, "a$a::fast": 1.0}
        report = durations.render(times, "libs", 1)
        self.assertIn("The 1 slowest tests:", report)
        self.assertIn("`a$a::slow`", report)
        self.assertNotIn("`a$a::fast`", report)

    def test_an_empty_stream_says_so(self):
        self.assertIn("No per-test times in the stream.", durations.render({}, "libs", 10))


class CommandTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)

    def run_script(self, lines, *extra):
        path = Path(self.directory.name) / "stream.json"
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        output = io.StringIO()
        with redirect_stdout(output):
            status = durations.main(["--stream", str(path), *extra])
        return status, output.getvalue()

    def test_a_run_with_times_passes(self):
        status, output = self.run_script([test_event("a$a::one", 2.0)])
        self.assertEqual(status, 0)
        self.assertIn("| tests | 1 |", output)

    def test_an_empty_stream_passes_unless_the_caller_asks_otherwise(self):
        self.assertEqual(self.run_script([])[0], 0)
        self.assertEqual(self.run_script([], "--exit-nonzero-on-empty")[0], 1)


if __name__ == "__main__":
    unittest.main()
