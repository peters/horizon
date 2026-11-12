#!/usr/bin/env python3
"""A test never makes a file executable that another thread can still refuse.

A test of this workspace writes stand-in programs with the helper of
`horizon-core` (`stand_in_program_in`, `stand_in_shell_in`), which writes the
file from a `/bin/sh` child: no descriptor of the file is open in the test
process, so a fork of another test thread cannot copy one and an `exec` never
answers `ETXTBSY` (`Text file busy`). A crate that has no dependency on
`horizon-core` writes the file with a `/bin/sh` child of its own, or starts the
file through `/bin/sh` instead of executing it.

This check fails when a source file of the workspace writes a file and then
makes it executable, unless the entry below names it with a reason. The mode of
the file counts in both of its forms: `set_permissions(path, mode)` with `0o755`
on the line of the call, and `permissions.set_mode(0o755)` above
`set_permissions(path, permissions)`.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MODE_ON_FILE = re.compile(r"([A-Za-z_][A-Za-z0-9_]*)\.set_permissions\(.*0o7")
SET_PERMISSIONS = re.compile(r"set_permissions\(\s*(?:&)?([A-Za-z_][A-Za-z0-9_.]*)\s*,\s*(.+?)\)")
MODE_IN_VARIABLE = re.compile(r"([A-Za-z_][A-Za-z0-9_]*)\.set_mode\(\s*0o7")
WRITTEN = re.compile(r"(fs::write|write_all|OpenOptions)[^\n]*{}")
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


def made_executable(line: str, window: str) -> set[str]:
    """The files that `line` makes executable, with a mode of its own or of a value.

    The mode of a file can be set on the call itself, as `0o755`, or on a value that
    the file takes first: `permissions.set_mode(0o755)` and then
    `set_permissions(&program, permissions)`. The second form names no mode on the
    line of the call, so the value is looked up in the lines above it.
    """
    names: set[str] = set()
    direct = MODE_ON_FILE.search(line)
    if direct:
        names.add(direct.group(1))
    modes = set(MODE_IN_VARIABLE.findall(window))
    call = SET_PERMISSIONS.search(line)
    if call and ("0o7" in call.group(2) or call.group(2).strip("& ") in modes):
        names.add(call.group(1).split(".")[0])
    return names


def violations() -> dict[str, list[int]]:
    found: dict[str, list[int]] = {}
    for file in sorted((ROOT / "crates").rglob("*.rs")):
        lines = file.read_text(errors="replace").splitlines()
        for index, line in enumerate(lines):
            if line.lstrip().startswith("//"):
                continue
            window = "\n".join(lines[max(0, index - LOOK_BACK) : index + 1])
            for name in sorted(made_executable(line, window)):
                written = WRITTEN.pattern.format(re.escape(name))
                if re.search(written, window) or re.search(rf"fs::write\(\s*(?:&)?{re.escape(name)}", window):
                    relative = file.relative_to(ROOT).as_posix()
                    found.setdefault(relative, []).append(index + 1)
                    break
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
        print("  a crate without horizon-core: a /bin/sh child writes the file, or /bin/sh reads it", file=sys.stderr)
    for path in ALLOWED:
        if path not in found:
            status = 1
            print(f"maintainability error: the allow list names {path}, which no longer writes one", file=sys.stderr)
    return status


if __name__ == "__main__":
    sys.exit(main())
