#!/usr/bin/env python3
"""Fail a run when one test takes longer than its allowed time.

The shard that runs `cargo nextest` keeps nextest's libtest-json stream, which
carries one `exec_time` for each test. This check reads that stream and compares
it with `scripts/test-duration-allowances.toml`. It adds no run time: the stream
is a by-product of the run that already happened.

A test over the ceiling must be listed with its own ceiling, so the list can only
shrink and a new slow test fails the run that introduces it. A listed test that a
run no longer reports is a warning, because a rename is not a duration, and a
missing name must not fail an unrelated change.
"""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_ALLOWANCES = ROOT / "scripts" / "test-duration-allowances.toml"
# A listed test may grow by this factor before the check fails, because a busy
# runner moves a wall measure. #1796 measured one core test at 20.11 s alone and
# 66.7 s inside the suite.
GROWTH = 1.25


@dataclass(frozen=True)
class Finding:
    """One reason the check fails, or one thing a reader should know."""

    kind: str
    name: str
    seconds: float
    allowed: float


def read_allowances(path: Path) -> dict[str, float]:
    """Read the permitted seconds of each listed test, keyed by its full name."""
    if not path.is_file():
        return {}
    document = tomllib.loads(path.read_text(encoding="utf-8"))
    tests = document.get("tests", {})
    allowances: dict[str, float] = {}
    for name, entry in tests.items():
        seconds = entry.get("seconds") if isinstance(entry, dict) else None
        if not isinstance(seconds, (int, float)):
            raise SystemExit(f"{path}: {name} needs a numeric `seconds`")
        allowances[name] = float(seconds)
    return allowances


def read_times(path: Path) -> dict[str, float]:
    """Read one execution time per test from nextest's libtest-json stream."""
    times: dict[str, float] = {}
    if not path.is_file():
        return times
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(event, dict) or event.get("type") != "test":
            continue
        if event.get("event") in ("started", "ignored", None):
            continue
        seconds = event.get("exec_time")
        name = event.get("name")
        if isinstance(seconds, (int, float)) and isinstance(name, str):
            times[name] = float(seconds)
    return times


def check(times: dict[str, float], allowances: dict[str, float], ceiling: float) -> list[Finding]:
    """Compare one run against the allowances. The order is the report's order."""
    findings: list[Finding] = []
    for name, seconds in sorted(times.items(), key=lambda item: item[1], reverse=True):
        allowed = allowances.get(name)
        if allowed is None:
            if seconds > ceiling:
                findings.append(Finding("unlisted", name, seconds, ceiling))
            continue
        if seconds > allowed * GROWTH:
            findings.append(Finding("over-allowance", name, seconds, allowed))
    missing = sorted(set(allowances) - set(times))
    findings.extend(Finding("missing", name, 0.0, allowances[name]) for name in missing)
    return findings


def render(times: dict[str, float], findings: list[Finding], ceiling: float, top: int) -> str:
    """Write the report a reader sees in the job log."""
    total = sum(times.values())
    lines = [
        f"Test durations: {len(times)} tests, {total / 60:.1f} min of test time "
        f"one test at a time, ceiling {ceiling:.0f} s, "
        f"slowest {max(times.values(), default=0.0):.1f} s",
    ]
    slowest = sorted(times.items(), key=lambda item: item[1], reverse=True)[:top]
    for name, seconds in slowest:
        lines.append(f"  {seconds:7.1f} s  {name}")
    failures = [finding for finding in findings if finding.kind != "missing"]
    for finding in failures:
        if finding.kind == "unlisted":
            lines.append(
                f"::error::{finding.name} took {finding.seconds:.1f} s, over the "
                f"{finding.allowed:.0f} s ceiling and not in the allowances",
            )
        else:
            lines.append(
                f"::error::{finding.name} took {finding.seconds:.1f} s, over its "
                f"{finding.allowed:.0f} s allowance by more than {GROWTH:.2f}x",
            )
    for finding in findings:
        if finding.kind == "missing":
            lines.append(
                f"::warning::{finding.name} has an allowance of {finding.allowed:.0f} s "
                "but this run did not report it: remove the allowance or fix the name",
            )
    return "\n".join(lines)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stream", required=True, type=Path,
                        help="the libtest-json stream of the run that just finished")
    parser.add_argument("--allowances", type=Path, default=DEFAULT_ALLOWANCES)
    parser.add_argument("--ceiling", type=float, default=60.0,
                        help="the longest a test may take without an allowance")
    parser.add_argument("--top", type=int, default=10)
    arguments = parser.parse_args(argv)

    times = read_times(arguments.stream)
    if not times:
        print(f"Test durations: no per-test times in {arguments.stream}", file=sys.stderr)
        return 1
    findings = check(times, read_allowances(arguments.allowances), arguments.ceiling)
    print(render(times, findings, arguments.ceiling, arguments.top))
    return 1 if any(finding.kind != "missing" for finding in findings) else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
