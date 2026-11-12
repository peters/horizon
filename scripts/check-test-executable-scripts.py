#!/usr/bin/env python3
"""A test never makes a file executable that another thread can still refuse.

A test of this workspace writes stand-in programs with the helper of
`horizon-core` (`stand_in_program_in`, `stand_in_shell_in`), which writes the
file from a `/bin/sh` child: no descriptor of the file is open in the test
process, so a fork of another test thread cannot copy one and an `exec` never
answers `ETXTBSY` (`Text file busy`).

This check fails when a source file of the workspace writes a file and then
makes it executable, unless the entry below names it with a reason.
"""

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXECUTABLE = re.compile(r"set_permissions\(\s*(?:&)?([A-Za-z_][A-Za-z0-9_.]*)\s*,.*0o7")
MODE_ON_FILE = re.compile(r"([A-Za-z_][A-Za-z0-9_]*)\.set_permissions\(.*0o7")
LOOK_BACK = 40

# A file path, then the reason that it writes an executable of its own.
ALLOWED = {
    "crates/horizon-vnc/src/session/host_key.rs": (
        "the crate has no horizon-core dependency; the test starts the program itself and retries "
        "`ExecutableFileBusy` until the window of a concurrent fork closes"
    ),
    "crates/horizon-browser-control/src/ssh_forward/tests.rs": (
        "the crate has no horizon-core dependency; one mutex serializes the tests that write a "
        "stand-in with the tests that start one"
    ),
    "crates/horizon-browser-cli/src/job/agent.rs": (
        "the file is empty and stands for a program of a computer; nothing executes it"
    ),
}


def violations() -> dict[str, list[int]]:
    found: dict[str, list[int]] = {}
    for file in sorted((ROOT / "crates").rglob("*.rs")):
        lines = file.read_text(errors="replace").splitlines()
        for index, line in enumerate(lines):
            if line.lstrip().startswith("//"):
                continue
            window = "\n".join(lines[max(0, index - LOOK_BACK) : index + 1])
            written = EXECUTABLE.search(line) or MODE_ON_FILE.search(line)
            if not written:
                continue
            name = re.escape(written.group(1).split(".")[0])
            if re.search(rf"(fs::write|write_all|OpenOptions)[^\n]*{name}", window) or re.search(
                rf"fs::write\(\s*(?:&)?{name}", window
            ):
                relative = file.relative_to(ROOT).as_posix()
                found.setdefault(relative, []).append(index + 1)
    return found


def main() -> int:
    found = violations()
    status = 0
    for path, lines in found.items():
        if path in ALLOWED:
            continue
        status = 1
        print(f"maintainability error: {path}:{lines[0]} writes a script and makes it executable", file=sys.stderr)
        print("  write it with horizon_core::stand_in_program_in or stand_in_shell_in", file=sys.stderr)
    for path in ALLOWED:
        if path not in found:
            status = 1
            print(f"maintainability error: the allow list names {path}, which no longer writes one", file=sys.stderr)
    return status


if __name__ == "__main__":
    sys.exit(main())
