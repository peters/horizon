#!/usr/bin/env python3
"""Publish the test durations of one shard run as a Markdown table.

The shard that runs `cargo nextest` keeps its libtest-json stream, which carries
one `exec_time` for every test. This report reads that stream and writes what a
reader needs to see a duration regression: the test time of the whole run, the
number of tests, the split by crate, and the slowest tests.

The repository publishes it in the job summary of every push to `main`, so a
change to the main branch leaves its duration record behind without a second run
and without a second runner.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path


def crate_of(name: str) -> str:
    """The package that owns a test name from the stream."""
    head = name.split("::", 1)[0]
    return head or "unknown"


def read_times(path: Path) -> dict[str, float]:
    """Read one execution time per finished test from the libtest-json stream."""
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


def minutes(seconds: float) -> str:
    """A duration a reader can compare with a budget."""
    return f"{seconds / 60:.1f} min"


def render(times: dict[str, float], label: str, top: int) -> str:
    """Write the Markdown report for one shard run."""
    if not times:
        return f"### Test durations, {label}\n\nNo per-test times in the stream.\n"
    total = sum(times.values())
    slowest = sorted(times.items(), key=lambda item: item[1], reverse=True)
    by_crate: dict[str, list[float]] = defaultdict(list)
    for name, seconds in times.items():
        by_crate[crate_of(name)].append(seconds)
    lines = [
        f"### Test durations, {label}",
        "",
        "| measurement | value |",
        "|---|---|",
        f"| tests | {len(times)} |",
        f"| test time, one test at a time | **{minutes(total)}** ({total:.0f} s) |",
        f"| slowest test | {slowest[0][1]:.1f} s |",
        "",
        "| crate | tests | test time | share |",
        "|---|---|---|---|",
    ]
    for crate, seconds_list in sorted(by_crate.items(), key=lambda item: sum(item[1]), reverse=True):
        crate_total = sum(seconds_list)
        lines.append(
            f"| {crate} | {len(seconds_list)} | {crate_total:.1f} s | "
            f"{100 * crate_total / total:.1f} percent |",
        )
    lines += ["", f"The {min(top, len(slowest))} slowest tests:", ""]
    lines += [f"1. `{name}` — {seconds:.1f} s" for name, seconds in slowest[:top]]
    return "\n".join(lines) + "\n"


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stream", required=True, type=Path)
    parser.add_argument("--label", default="libs")
    parser.add_argument("--top", type=int, default=10)
    parser.add_argument("--exit-nonzero-on-empty", action="store_true",
                        help="fail when the stream holds no per-test times")
    arguments = parser.parse_args(argv)
    times = read_times(arguments.stream)
    print(render(times, arguments.label, arguments.top))
    if not times and arguments.exit_nonzero_on_empty:
        print(f"no per-test times in {arguments.stream}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
