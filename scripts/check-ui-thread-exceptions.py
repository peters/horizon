#!/usr/bin/env python3
"""Check that the exceptions of the UI-thread guard and the guide list the same work.

The guard of `crates/horizon-core/src/ui_thread.rs` accepts a call that can wait
on the UI thread inside an exception scope, and each scope carries the reason
that the repository accepts it. The section "Known UI-thread work" of
`docs/architecture/ui-thread.md` lists the same reasons, one for one.

This check fails when a reason of the code has no entry in that section, or when
the section names a reason that no code holds any more.
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
GUIDE = ROOT / "docs" / "architecture" / "ui-thread.md"
SECTION = "## Known UI-thread work"
PATTERN = re.compile(r'ui_thread::exception\("((?:[^"\\]|\\.)*)"\)', re.S)
SOURCES = ("crates", "scripts")


def reasons_of_the_code() -> list[str]:
    reasons: list[str] = []
    for base in SOURCES:
        for path in sorted((ROOT / base).rglob("*.rs")):
            reasons.extend(PATTERN.findall(path.read_text(encoding="utf-8")))
    return reasons


def reasons_of_the_guide(known: set[str]) -> list[str]:
    text = GUIDE.read_text(encoding="utf-8")
    start = text.find(SECTION)
    if start < 0:
        raise SystemExit(f"{GUIDE}: the section {SECTION!r} is missing")
    section = text[start:]
    found: list[str] = []
    for line in section.splitlines()[1:]:
        if line.startswith("## "):
            break
        for reason in re.findall(r"`([^`]+)`", line):
            if reason in known:
                found.append(reason)
    return found


def main() -> int:
    code = reasons_of_the_code()
    guide = reasons_of_the_guide(set(code))
    missing = [reason for reason in code if reason not in guide]
    extra = [reason for reason in guide if reason not in code]
    status = 0
    if len(code) != len(set(code)):
        for reason in sorted({reason for reason in code if code.count(reason) > 1}):
            print(f"the reason {reason!r} is written in more than one exception scope", file=sys.stderr)
            status = 1
    for reason in missing:
        print(f"the guide lists no exception for {reason!r}", file=sys.stderr)
        status = 1
    for reason in extra:
        print(f"the guide lists {reason!r}, and no exception scope holds it", file=sys.stderr)
        status = 1
    if status == 0:
        print(f"the guide and the code name the same {len(code)} exceptions")
    return status


if __name__ == "__main__":
    sys.exit(main())
