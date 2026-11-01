#!/usr/bin/env python3
"""Tests for scripts/check-test-durations.py."""

import importlib.util
import io
import json
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "check-test-durations.py"


def load_module():
    """Import the hyphenated script by path."""
    spec = importlib.util.spec_from_file_location("check_test_durations", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


durations = load_module()


def test_event(name, exec_time, status="ok"):
    return json.dumps({"type": "test", "event": status, "name": name, "exec_time": exec_time})


class StreamTests(unittest.TestCase):
    def write(self, lines):
        path = Path(self.directory.name) / "stream.json"
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return path

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)

    def test_reads_one_time_per_finished_test(self):
        path = self.write([
            json.dumps({"type": "suite", "event": "started", "nextest": {"crate": "c"}}),
            test_event("horizon-core::horizon_core$board::moves", 0.5),
            test_event("horizon-core::horizon_core$board::slow", 12.5),
        ])
        self.assertEqual(
            durations.read_times(path),
            {"horizon-core::horizon_core$board::moves": 0.5,
             "horizon-core::horizon_core$board::slow": 12.5},
        )

    def test_skips_started_ignored_and_noise(self):
        path = self.write([
            "not json at all",
            "{broken",
            test_event("a$b::started", 9.0, status="started"),
            test_event("a$b::ignored", 9.0, status="ignored"),
            json.dumps({"type": "test", "event": "ok", "name": "a$b::kept", "exec_time": 1.0}),
        ])
        self.assertEqual(durations.read_times(path), {"a$b::kept": 1.0})

    def test_a_missing_stream_has_no_times(self):
        self.assertEqual(durations.read_times(Path(self.directory.name) / "absent"), {})


class CheckTests(unittest.TestCase):
    def test_a_test_over_the_ceiling_without_an_allowance_fails(self):
        findings = durations.check({"a$b::slow": 75.0}, {}, 60.0)
        self.assertEqual([finding.kind for finding in findings], ["unlisted"])
        self.assertEqual(findings[0].allowed, 60.0)

    def test_a_listed_test_within_its_allowance_passes(self):
        self.assertEqual(durations.check({"a$b::slow": 75.0}, {"a$b::slow": 90.0}, 60.0), [])

    def test_a_listed_test_that_breaches_the_growth_factor_fails(self):
        findings = durations.check({"a$b::slow": 120.0}, {"a$b::slow": 90.0}, 60.0)
        self.assertEqual([finding.kind for finding in findings], ["over-allowance"])

    def test_a_listed_test_inside_the_growth_factor_passes(self):
        self.assertEqual(durations.check({"a$b::slow": 110.0}, {"a$b::slow": 90.0}, 60.0), [])

    def test_a_missing_listed_test_is_only_a_warning(self):
        findings = durations.check({"a$b::fast": 1.0}, {"a$b::renamed": 90.0}, 60.0)
        self.assertEqual([finding.kind for finding in findings], ["missing"])

    def test_the_order_is_by_duration(self):
        findings = durations.check({"a$b::one": 61.0, "a$b::two": 99.0}, {}, 60.0)
        self.assertEqual([finding.name for finding in findings], ["a$b::two", "a$b::one"])


class ReportTests(unittest.TestCase):
    def test_the_report_names_the_slowest_tests_and_each_error(self):
        times = {"a$b::fast": 1.0, "a$b::slow": 75.0}
        findings = durations.check(times, {}, 60.0)
        report = durations.render(times, findings, 60.0, 10)
        self.assertIn("2 tests, ceiling 60 s, slowest 75.0 s", report)
        self.assertLess(report.index("a$b::slow"), report.index("a$b::fast"))
        self.assertIn("::error::a$b::slow took 75.0 s", report)

    def test_a_missing_allowance_renders_a_warning(self):
        findings = durations.check({"a$b::fast": 1.0}, {"a$b::renamed": 90.0}, 60.0)
        report = durations.render({"a$b::fast": 1.0}, findings, 60.0, 10)
        self.assertIn("::warning::a$b::renamed has an allowance of 90 s", report)
        self.assertNotIn("::error::", report)


class AllowanceTests(unittest.TestCase):
    def write(self, text):
        path = Path(self.directory.name) / "allowances.toml"
        path.write_text(text, encoding="utf-8")
        return path

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)

    def test_reads_the_seconds_of_each_listed_test(self):
        path = self.write('[tests]\n"a$b::slow" = { seconds = 95 }\n')
        self.assertEqual(durations.read_allowances(path), {"a$b::slow": 95.0})

    def test_an_entry_without_seconds_is_refused(self):
        path = self.write('[tests]\n"a$b::slow" = { note = "why" }\n')
        with self.assertRaises(SystemExit):
            durations.read_allowances(path)

    def test_the_repository_allowance_file_parses(self):
        self.assertIsInstance(durations.read_allowances(durations.DEFAULT_ALLOWANCES), dict)


class CommandTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)

    def run_check(self, lines, allowances="[tests]\n"):
        stream = Path(self.directory.name) / "stream.json"
        stream.write_text("\n".join(lines) + "\n", encoding="utf-8")
        path = Path(self.directory.name) / "allowances.toml"
        path.write_text(allowances, encoding="utf-8")
        output = io.StringIO()
        with redirect_stdout(output):
            status = durations.main(["--stream", str(stream), "--allowances", str(path)])
        return status, output.getvalue()

    def test_a_clean_run_passes_and_prints_the_slowest_test(self):
        status, output = self.run_check([test_event("a$b::slow", 42.0)])
        self.assertEqual(status, 0)
        self.assertIn("a$b::slow", output)

    def test_a_slow_test_fails_the_command(self):
        status, output = self.run_check([test_event("a$b::slow", 61.0)])
        self.assertEqual(status, 1)
        self.assertIn("::error::", output)

    def test_an_empty_stream_fails_the_command(self):
        status, _ = self.run_check([])
        self.assertEqual(status, 1)


if __name__ == "__main__":
    unittest.main()
