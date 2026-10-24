#!/usr/bin/env python3
"""Tests for scripts/test-duration-baseline.py."""

import importlib.util
import io
import json
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "test-duration-baseline.py"


def load_module():
    """Import the hyphenated script by path."""
    spec = importlib.util.spec_from_file_location("test_duration_baseline", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    # A dataclass resolves its own module while it is created.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


baseline = load_module()


def suite_event(crate, binary, kind="lib"):
    return json.dumps({"type": "suite", "event": "started", "test_count": 1,
                       "nextest": {"crate": crate, "test_binary": binary, "kind": kind}})


def test_event(name, exec_time, status="ok"):
    return json.dumps({"type": "test", "event": status, "name": name, "exec_time": exec_time})


def stream(*events):
    return list(events)


class ParseStreamTests(unittest.TestCase):
    def test_reads_the_crate_and_binary_of_each_test(self):
        results, unattributed = baseline.parse_stream(stream(
            suite_event("horizon-core", "horizon_core"),
            test_event("horizon-core::horizon_core$board::moves", 0.5),
            suite_event("horizon-ui", "horizon_ui", kind="bin"),
            test_event("horizon-ui::horizon_ui$theme::dark", 0.25),
        ))
        self.assertEqual([r.crate for r in results], ["horizon-core", "horizon-ui"])
        self.assertEqual([r.binary for r in results], ["horizon_core", "horizon_ui"])
        self.assertEqual([r.kind for r in results], ["lib", "bin"])
        self.assertEqual([r.exec_time for r in results], [0.5, 0.25])
        self.assertEqual(unattributed, 0)

    def test_a_test_keeps_its_own_crate_when_suites_interleave(self):
        # Two suites run at the same time and their events arrive mixed. The
        # last suite to start is not the suite of the next test.
        results, unattributed = baseline.parse_stream(stream(
            suite_event("horizon-cloud-worker", "horizon_cloud_worker"),
            suite_event("horizon-ui", "horizon", kind="bin"),
            test_event("horizon-ui::horizon$app::press_frame::moves", 1.0),
            test_event("horizon-cloud-worker::horizon_cloud_worker$bootstrap::init", 2.0),
        ))
        self.assertEqual([r.crate for r in results], ["horizon-ui", "horizon-cloud-worker"])
        self.assertEqual([r.short_name for r in results],
                         ["app::press_frame::moves", "bootstrap::init"])
        self.assertEqual(unattributed, 0)

    def test_ignores_started_and_ignored_events(self):
        results, _ = baseline.parse_stream(stream(
            suite_event("horizon-core", "horizon_core"),
            test_event("horizon-core::horizon_core$board::moves", 0.0, status="started"),
            test_event("horizon-core::horizon_core$board::slow", None, status="ignored"),
            test_event("horizon-core::horizon_core$board::keeps", 0.125),
        ))
        self.assertEqual([r.short_name for r in results], ["board::keeps"])

    def test_keeps_a_failed_test_with_its_time(self):
        results, _ = baseline.parse_stream(stream(
            suite_event("horizon-core", "horizon_core"),
            test_event("horizon-core::horizon_core$board::fails", 1.5, status="failed"),
        ))
        self.assertEqual(results[0].status, "failed")
        self.assertEqual(results[0].exec_time, 1.5)

    def test_ignores_lines_that_are_not_events(self):
        results, _ = baseline.parse_stream(stream(
            "Running the tests",
            "{not json",
            json.dumps([1, 2, 3]),
            suite_event("horizon-core", "horizon_core"),
            test_event("horizon-core::horizon_core$board::moves", 0.5),
        ))
        self.assertEqual(len(results), 1)

    def test_a_name_without_a_prefix_uses_the_suite_and_is_counted(self):
        results, unattributed = baseline.parse_stream(stream(
            suite_event("horizon-browser", "webauthn_live", kind="test"),
            test_event("webauthn_live: test", 0.5),
        ))
        self.assertEqual(results[0].crate, "horizon-browser")
        self.assertEqual(results[0].binary, "webauthn_live")
        self.assertEqual(unattributed, 1)


class AggregateTests(unittest.TestCase):
    def run_of(self, *results):
        return baseline.Run(wall_clock=1.0, results=list(results))

    def result(self, crate, exec_time, binary="tests", kind="lib"):
        return baseline.TestResult(name=f"{crate}::{binary}$test", crate=crate, binary=binary,
                                   kind=kind, exec_time=exec_time, status="ok")

    def test_serial_time_sums_every_test(self):
        run = self.run_of(self.result("a", 1.0), self.result("b", 2.5))
        self.assertEqual(run.serial_time, 3.5)

    def test_by_crate_is_slowest_first(self):
        run = self.run_of(self.result("fast", 0.1), self.result("slow", 5.0),
                          self.result("slow", 1.0))
        self.assertEqual(run.by_crate(), [("slow", 2, 6.0), ("fast", 1, 0.1)])

    def test_by_kind_separates_an_integration_target(self):
        run = self.run_of(self.result("a", 1.0, kind="lib"),
                          self.result("a", 3.0, binary="integration", kind="test"))
        self.assertEqual(run.by_kind(), [("test", 1, 3.0), ("lib", 1, 1.0)])

    def test_by_binary_names_the_target(self):
        run = self.run_of(self.result("a", 1.0, binary="lib_target", kind="lib"),
                          self.result("a", 3.0, binary="integration", kind="test"))
        self.assertEqual(run.by_binary()[0], ("a", "integration", "test", 1, 3.0))

    def test_slowest_limits_and_orders(self):
        run = self.run_of(self.result("a", 0.1), self.result("b", 9.0), self.result("c", 3.0))
        self.assertEqual([r.exec_time for r in run.slowest(2)], [9.0, 3.0])

    def test_failed_selects_every_status_but_ok(self):
        run = self.run_of(self.result("a", 1.0),
                          baseline.TestResult(name="n", crate="a", binary="b", kind="lib",
                                              exec_time=1.0, status="timeout"))
        self.assertEqual([r.status for r in run.failed()], ["timeout"])

    def test_share_of_counts_the_slowest_tests(self):
        run = self.run_of(*[self.result("a", 1.0) for _ in range(9)], self.result("a", 9.0))
        self.assertAlmostEqual(run.share_of(1), 9.0 / 18.0)

    def test_pareto_ends_at_one(self):
        run = self.run_of(self.result("a", 1.0), self.result("b", 3.0))
        self.assertEqual(run.pareto()[-1], (1.0, 1.0))
        self.assertAlmostEqual(run.pareto()[0][1], 0.75)

    def test_pareto_of_an_empty_run_is_one_point(self):
        self.assertEqual(self.run_of().pareto(), [(1.0, 1.0)])

    def test_slowest_count_for_finds_the_half_point(self):
        run = self.run_of(*[self.result("a", 1.0) for _ in range(10)])
        self.assertEqual(baseline.slowest_count_for(run, 0.5), 5)
        self.assertEqual(baseline.slowest_count_for(run, 0.8), 8)


class RenderTests(unittest.TestCase):
    def test_render_names_the_slowest_test_and_the_share(self):
        run = baseline.Run(wall_clock=1.0, results=[
            baseline.TestResult(name="horizon-core::horizon_core$board::moves", crate="horizon-core",
                                binary="horizon_core", kind="lib", exec_time=2.0, status="ok"),
            baseline.TestResult(name="horizon-core::horizon_core$board::keeps", crate="horizon-core",
                                binary="horizon_core", kind="lib", exec_time=6.0, status="ok"),
        ])
        report = baseline.render(run, top=5, crates=5)
        self.assertIn("Tests measured: 2", report)
        self.assertIn("`board::moves`", report)
        self.assertIn("| horizon-core | 2 |", report)
        self.assertIn("carry half of the serial time", report)
        self.assertLess(report.index("board::keeps"), report.index("board::moves"))

    def test_render_counts_tests_whose_name_names_no_crate(self):
        run = baseline.Run(wall_clock=1.0, unattributed=3, results=[
            baseline.TestResult(name="webauthn_live: test", crate="horizon-browser",
                                binary="webauthn_live", kind="test", exec_time=1.0, status="ok")])
        self.assertIn("Tests whose name names no crate: 3", baseline.render(run, top=1, crates=1))

    def test_render_of_an_empty_run_says_so(self):
        report = baseline.render(baseline.Run(wall_clock=0.5), top=5, crates=5)
        self.assertIn("No test result was read", report)

    def test_render_of_a_run_of_zero_times_does_not_divide_by_zero(self):
        run = baseline.Run(wall_clock=0.5, results=[
            baseline.TestResult(name="a::b$c", crate="a", binary="b", kind="lib",
                                exec_time=0.0, status="ok")])
        report = baseline.render(run, top=5, crates=5)
        self.assertIn("| a | 1 | 0 ms | 0.0% |", report)
        self.assertNotIn("carry half of the serial time", report)

    def test_render_lists_a_failed_test(self):
        run = baseline.Run(wall_clock=1.0, results=[
            baseline.TestResult(name="a::b$c", crate="a", binary="b", kind="lib",
                                exec_time=1.0, status="failed")])
        report = baseline.render(run, top=5, crates=5)
        self.assertIn("Tests that did not pass: 1", report)
        self.assertIn("| `c` | a | failed |", report)


class CommandTests(unittest.TestCase):
    def test_a_whole_run_selects_the_workspace(self):
        command = baseline.nextest_command([])
        self.assertIn("--workspace", command)
        self.assertNotIn("-p", command)

    def test_a_named_package_replaces_the_workspace_selector(self):
        # `--workspace` with `-p` selects both, so one crate would run all of them.
        command = baseline.nextest_command(["horizon-core"])
        self.assertNotIn("--workspace", command)
        self.assertEqual(command[-2:], ["-p", "horizon-core"])

    def test_two_named_packages_are_both_selected(self):
        command = baseline.nextest_command(["a", "b"])
        self.assertEqual(command.count("-p"), 2)

    def test_the_run_reads_the_libtest_json_stream(self):
        self.assertIn("libtest-json-plus", baseline.nextest_command([]))


class FormatTests(unittest.TestCase):
    def test_seconds_switches_unit(self):
        self.assertEqual(baseline.seconds(0.25), "250 ms")
        self.assertEqual(baseline.seconds(12.34), "12.3 s")
        self.assertEqual(baseline.seconds(150), "2.5 min")

    def test_percent_has_one_decimal(self):
        self.assertEqual(baseline.percent(0.4567), "45.7%")

    def test_percent_of_a_zero_total_is_zero(self):
        self.assertEqual(baseline.percent_of(0.0, 0.0), "0.0%")


class MainTests(unittest.TestCase):
    def test_renders_a_saved_run_without_cargo(self):
        saved = {"wall_clock": 2.0, "results": [
            {"name": "a::b$slow", "crate": "a", "binary": "b", "kind": "lib",
             "exec_time": 4.0, "status": "ok"}]}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "saved.json"
            path.write_text(json.dumps(saved), encoding="utf-8")
            out, err = io.StringIO(), io.StringIO()
            with redirect_stdout(out), redirect_stderr(err):
                status = baseline.main(["--from-json", str(path)])
        self.assertEqual(status, 0)
        self.assertIn("Tests measured: 1", out.getvalue())
        self.assertIn("`slow`", out.getvalue())


if __name__ == "__main__":
    unittest.main()
