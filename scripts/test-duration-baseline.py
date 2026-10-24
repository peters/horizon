#!/usr/bin/env python3
"""Rank the test suite by measured time instead of by test count.

A test count does not say where the wall clock goes. This helper runs the
workspace with `cargo nextest`, reads one `exec_time` per test from the
libtest JSON stream, and reports the slowest crates and tests, the serial time
the whole suite costs, and the share of that time the slowest tests carry.

The numbers are the input to a decision about what to delete or to make faster.
A test that costs a millisecond is not worth a review of its own.

Usage:
    scripts/test-duration-baseline.py                    # measure the workspace
    scripts/test-duration-baseline.py -p horizon-core    # measure one crate
    scripts/test-duration-baseline.py --from-json FILE   # render a saved run
    scripts/test-duration-baseline.py --json OUT.json    # keep the raw result
    scripts/test-duration-baseline.py -v                 # show cargo progress

`cargo nextest` must be installed: `cargo install cargo-nextest --locked`.
Exit status is 0 when the tests ran, also when some of them failed. A failed
test is a result of the measurement, not of the measurement helper.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
from collections import defaultdict
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CARGO = shutil.which("cargo") or "cargo"

# nextest hides its libtest JSON stream behind this switch. The stream carries
# one `exec_time` for each test, which the human output does not give in a form
# a script can read.
LIBTEST_JSON_ENV = "NEXTEST_EXPERIMENTAL_LIBTEST_JSON"

# A test name in that stream names its package and its test binary first, for
# example `horizon-core::horizon_core$board::moves`.
NAME_PREFIX = re.compile(r"^(?P<crate>[^:$]+)::(?P<binary>[^$]+)\$")

# One line of a report. `section` is where a reader should look first.
@dataclass(frozen=True)
class TestResult:
    """The measured time of one test."""

    name: str
    crate: str
    binary: str
    kind: str
    exec_time: float
    status: str

    @property
    def short_name(self) -> str:
        """The test path without the crate and binary prefixes nextest adds."""
        prefix = f"{self.crate}::{self.binary}$"
        return self.name[len(prefix):] if self.name.startswith(prefix) else self.name


@dataclass
class Run:
    """One measurement of a test run."""

    wall_clock: float
    results: list[TestResult] = field(default_factory=list)
    exit_code: int = 0
    # Tests whose name names no crate. They are counted against the last suite
    # that started, which is the only guess the stream allows.
    unattributed: int = 0

    def by_crate(self) -> list[tuple[str, int, float]]:
        """Every crate with its test count and serial time, slowest first."""
        counts: dict[str, int] = defaultdict(int)
        times: dict[str, float] = defaultdict(float)
        for result in self.results:
            counts[result.crate] += 1
            times[result.crate] += result.exec_time
        rows = [(crate, counts[crate], times[crate]) for crate in times]
        return sorted(rows, key=lambda row: (-row[2], row[0]))

    def by_binary(self) -> list[tuple[str, str, str, int, float]]:
        """Every test binary with its kind, test count and serial time, slowest first."""
        counts: dict[tuple[str, str, str], int] = defaultdict(int)
        times: dict[tuple[str, str, str], float] = defaultdict(float)
        for result in self.results:
            key = (result.crate, result.binary, result.kind)
            counts[key] += 1
            times[key] += result.exec_time
        rows = [(*key, counts[key], times[key]) for key in times]
        return sorted(rows, key=lambda row: (-row[4], row[0], row[1]))

    def by_kind(self) -> list[tuple[str, int, float]]:
        """Each target kind with its test count and serial time, slowest first."""
        counts: dict[str, int] = defaultdict(int)
        times: dict[str, float] = defaultdict(float)
        for result in self.results:
            counts[result.kind] += 1
            times[result.kind] += result.exec_time
        rows = [(kind, counts[kind], times[kind]) for kind in times]
        return sorted(rows, key=lambda row: (-row[2], row[0]))

    def slowest(self, limit: int) -> list[TestResult]:
        """The slowest tests, slowest first."""
        return sorted(self.results, key=lambda r: (-r.exec_time, r.name))[:limit]

    def failed(self) -> list[TestResult]:
        """Every test that did not pass."""
        return [r for r in self.results if r.status != "ok"]

    @property
    def serial_time(self) -> float:
        """The time the suite costs when no two tests run at the same moment."""
        return sum(r.exec_time for r in self.results)

    def pareto(self) -> list[tuple[float, float]]:
        """Points of (share of tests, share of serial time) for the slowest tests first.

        The last point is always (1.0, 1.0). A short curve means a few tests
        hold most of the time, and that the rest is not worth reviewing.
        """
        ordered = sorted(self.results, key=lambda r: -r.exec_time)
        total = self.serial_time
        if not ordered or total <= 0:
            return [(1.0, 1.0)]
        points, running = [], 0.0
        for index, result in enumerate(ordered, start=1):
            running += result.exec_time
            points.append((index / len(ordered), running / total))
        return points

    def share_of(self, count: int) -> float:
        """The share of serial time that the slowest `count` tests carry."""
        total = self.serial_time
        if total <= 0:
            return 0.0
        return sum(r.exec_time for r in self.slowest(count)) / total


def parse_stream(lines) -> tuple[list[TestResult], int]:
    """Read libtest JSON events and return one result for each finished test.

    The tests of several suites run at the same time, so their events interleave
    in one stream and the last `suite started` event is not the suite of the
    next test. Each test name names its own crate and binary, which is the only
    attribution the stream carries that parallelism cannot make wrong. The
    suite event is a fallback for a binary that names no prefix, for example a
    test binary with its own harness. The count of those tests is returned too,
    because a fallback that runs often makes the report wrong.
    """
    results: list[TestResult] = []
    crate = binary = kind = "unknown"
    unattributed = 0
    for line in lines:
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(event, dict):
            continue
        if event.get("type") == "suite":
            nextest = event.get("nextest") or {}
            crate = nextest.get("crate") or "unknown"
            binary = nextest.get("test_binary") or "unknown"
            kind = nextest.get("kind") or "unknown"
            continue
        if event.get("type") != "test":
            continue
        status = event.get("event")
        if status in ("started", "ignored", None):
            continue
        exec_time = event.get("exec_time")
        if not isinstance(exec_time, (int, float)):
            continue
        name = event.get("name") or "unknown"
        match = NAME_PREFIX.match(name)
        if match:
            test_crate, test_binary = match.group("crate"), match.group("binary")
        else:
            test_crate, test_binary = crate, binary
            unattributed += 1
        results.append(
            TestResult(
                name=name,
                crate=test_crate,
                binary=test_binary,
                kind=kind,
                exec_time=float(exec_time),
                status=str(status),
            )
        )
    return results, unattributed


def render(run: Run, top: int, crates: int) -> str:
    """Write the report for one run. The reader decides what to delete from it."""
    lines = ["# Test duration baseline", ""]
    if run.results:
        parallelism = run.serial_time / run.wall_clock if run.wall_clock > 0 else 0.0
        lines += [
            f"- Tests measured: {len(run.results)}",
            f"- Serial test time: {seconds(run.serial_time)}",
            f"- Wall clock: {seconds(run.wall_clock)}",
            f"- Effective parallelism: {parallelism:.1f}x",
            "",
        ]
    else:
        lines += ["No test result was read from the run.", ""]

    failed = run.failed()
    if failed:
        lines += [f"- Tests that did not pass: {len(failed)}", ""]
    if run.unattributed:
        lines += [f"- Tests whose name names no crate: {run.unattributed}", ""]

    if run.results:
        lines += ["## Where the time goes", "", "| Crate | Tests | Serial time | Share |", "|---|---|---|---|"]
        for crate, count, total in run.by_crate()[:crates]:
            lines.append(f"| {crate} | {count} | {seconds(total)} | {percent_of(total, run.serial_time)} |")
        lines += ["", "| Target kind | Tests | Serial time | Share |", "|---|---|---|---|"]
        for kind, count, total in run.by_kind():
            lines.append(f"| {kind} | {count} | {seconds(total)} | {percent_of(total, run.serial_time)} |")

        half = slowest_count_for(run, 0.5)
        if run.serial_time > 0:
            lines += [
                "",
                f"The slowest {half} of {len(run.results)} tests carry half of the serial time, "
                f"and the slowest {slowest_count_for(run, 0.8)} carry 80 percent.",
            ]
        lines += [
            "",
            f"## Slowest {min(top, len(run.results))} tests",
            "",
            "| Test | Crate | Time |",
            "|---|---|---|",
        ]
        for result in run.slowest(top):
            lines.append(f"| `{result.short_name}` | {result.crate} | {seconds(result.exec_time)} |")

    if failed:
        lines += ["", "## Tests that did not pass", "", "| Test | Crate | Status |", "|---|---|---|"]
        for result in failed:
            lines.append(f"| `{result.short_name}` | {result.crate} | {result.status} |")

    return "\n".join(lines) + "\n"


def slowest_count_for(run: Run, share: float) -> int:
    """How many of the slowest tests carry at least `share` of the serial time."""
    ordered = sorted(run.results, key=lambda r: -r.exec_time)
    total = run.serial_time
    if total <= 0:
        return 0
    running = 0.0
    for index, result in enumerate(ordered, start=1):
        running += result.exec_time
        if running / total >= share:
            return index
    return len(ordered)


def seconds(value: float) -> str:
    """A duration that a reader compares by eye."""
    if value < 1:
        return f"{value * 1000:.0f} ms"
    if value < 60:
        return f"{value:.1f} s"
    return f"{value / 60:.1f} min"


def percent(value: float) -> str:
    """A share of a total, one decimal place."""
    return f"{value * 100:.1f}%"


def percent_of(value: float, total: float) -> str:
    """A share of a total that may be zero, for a run of empty tests."""
    return percent(value / total) if total > 0 else percent(0.0)


def nextest_command(packages) -> list[str]:
    """The command for the run. A named package replaces the workspace selector.

    `--workspace` together with `-p` selects both, so a request for one crate
    would run every crate. One of the two selectors goes in the command.
    """
    command = [CARGO, "nextest", "run", "--no-fail-fast",
               "--message-format", "libtest-json-plus"]
    if packages:
        for package in packages:
            command += ["-p", package]
    else:
        command += ["--workspace"]
    return command


def measure(packages, verbose: bool) -> Run:
    """Run the tests and read their times. The caller prints the report.

    Two runs that share a machine collide: tests take fixed ports and shared
    paths, and the loser of that race reports failures that are not defects.
    Start one run at a time.
    """
    command = nextest_command(packages)
    environment = dict(os.environ)
    environment[LIBTEST_JSON_ENV] = "1"
    if not verbose:
        print("Running the tests. This takes a full build and the whole suite.", file=sys.stderr)
    started = time.monotonic()
    completed = subprocess.run(command, cwd=ROOT, env=environment, text=True, check=False,
                               stdout=subprocess.PIPE,
                               stderr=None if verbose else subprocess.DEVNULL)
    wall_clock = time.monotonic() - started
    results, unattributed = parse_stream(completed.stdout.splitlines())
    return Run(wall_clock=wall_clock, results=results, exit_code=completed.returncode,
               unattributed=unattributed)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("-p", "--package", action="append", help="measure one crate; repeat for more")
    parser.add_argument("--from-json", type=Path, help="render a saved run instead of running the tests")
    parser.add_argument("--json", type=Path, help="write the raw result to this file")
    parser.add_argument("--top", type=int, default=20, help="slowest tests to list (default: 20)")
    parser.add_argument("--crates", type=int, default=12, help="crates to list (default: 12)")
    parser.add_argument("-v", "--verbose", action="store_true", help="show the cargo and nextest output")
    arguments = parser.parse_args(argv)

    if arguments.from_json:
        saved = json.loads(arguments.from_json.read_text(encoding="utf-8"))
        run = Run(wall_clock=saved["wall_clock"],
                  results=[TestResult(**result) for result in saved["results"]],
                  unattributed=saved.get("unattributed", 0))
    else:
        if shutil.which("cargo-nextest") is None:
            print("cargo nextest is not installed. Run: cargo install cargo-nextest --locked",
                  file=sys.stderr)
            return 2
        run = measure(arguments.package, arguments.verbose)

    print(render(run, arguments.top, arguments.crates), end="")

    if arguments.json:
        arguments.json.write_text(json.dumps({
            "wall_clock": run.wall_clock,
            "unattributed": run.unattributed,
            "results": [result.__dict__ for result in run.results],
        }, indent=1), encoding="utf-8")
        print(f"\nRaw result written to {arguments.json}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
