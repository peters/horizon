#!/usr/bin/env python3
"""Run the narrowest test command for the paths a change touches.

During development one edit needs the tests of its crate and module. A whole
workspace run rebuilds every dependent crate, including the UI stack, and runs
every test in the workspace. This helper maps changed paths to a crate, a test
target and a module filter, then runs that selection.

It is the inner loop only. The full local matrix still runs on the candidate
before a pull request opens.

Usage:
    scripts/test-focus.py                     # paths changed against origin/main
    scripts/test-focus.py src/board/mod.rs    # named paths
    scripts/test-focus.py --print             # show the commands, run nothing
    scripts/test-focus.py --all               # whole crate test targets
    scripts/test-focus.py --no-nextest        # force cargo test

Set HORIZON_TEST_FOCUS_BASE to compare against another base commit.
"""

from __future__ import annotations

import argparse
import os
import re
import shlex
import shutil
import subprocess
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GIT = shutil.which("git") or "git"

# Files that no per-crate selection covers: they change how every crate builds.
WORKSPACE_WIDE = {
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "rustfmt.toml",
}


@dataclass(frozen=True)
class Selection:
    """One crate and the test targets and filters to run in it."""

    crate: str
    target: tuple[str, str | None]
    # None runs the whole target. An empty tuple names no filter either, so
    # `add` keeps whole-target selections whole.
    filters: tuple[str, ...] | None = None


@dataclass
class Plan:
    """The selections of a change and the paths that selected nothing."""

    selections: list[Selection] = field(default_factory=list)
    unselected: list[tuple[str, str]] = field(default_factory=list)

    def add(self, selection: Selection) -> None:
        """Merge a selection into the plan, grouping by crate and target."""
        for index, existing in enumerate(self.selections):
            if (existing.crate, existing.target) != (selection.crate, selection.target):
                continue
            if existing.filters is None or selection.filters is None:
                filters = None
            else:
                filters = existing.filters + tuple(
                    item for item in selection.filters if item not in existing.filters
                )
            if filters != existing.filters:
                self.selections[index] = Selection(
                    existing.crate, existing.target, filters
                )
            return
        self.selections.append(selection)


def git_lines(*args: str) -> list[str]:
    """Return the non-empty lines of a git command, or nothing when it fails."""
    result = subprocess.run(
        [GIT, *args], cwd=ROOT, capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        return []
    return [line.strip() for line in result.stdout.splitlines() if line.strip()]


def changed_paths() -> list[str]:
    """Return the paths this branch changes, including uncommitted work."""
    base = os.environ.get("HORIZON_TEST_FOCUS_BASE")
    if not base:
        base = (git_lines("merge-base", "HEAD", "origin/main") or ["HEAD"])[0]
    paths = git_lines("diff", "--name-only", base, "--")
    paths += git_lines("ls-files", "--others", "--exclude-standard")
    seen: dict[str, None] = {}
    for path in paths:
        seen.setdefault(path, None)
    return list(seen)


def crate_root(path: Path, root: Path = ROOT) -> Path | None:
    """Return the crate directory that owns `path`, or None outside a crate."""
    for directory in [path, *path.parents]:
        if directory == root or root not in directory.parents:
            return None
        if (directory / "Cargo.toml").is_file():
            return directory
    return None


def crate_name(directory: Path) -> str | None:
    """Return the package name of a crate directory.

    The workspace manifest has no package section, so it names no crate.
    """
    manifest = directory / "Cargo.toml"
    try:
        document = tomllib.loads(manifest.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError):
        return None
    package = document.get("package")
    if not isinstance(package, dict):
        return None
    name = package.get("name")
    return name if isinstance(name, str) else None


def bin_names(crate: Path) -> list[str]:
    """Return the binary target names that a crate declares."""
    try:
        document = tomllib.loads((crate / "Cargo.toml").read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError):
        return []
    declared = document.get("bin") or []
    names = [entry["name"] for entry in declared if isinstance(entry, dict) and "name" in entry]
    if names:
        return names
    if (crate / "src" / "main.rs").is_file():
        name = crate_name(crate)
        return [name] if name else []
    return []


def default_target(crate: Path) -> tuple[str, str | None]:
    """Return the library target, or a binary target of a binary-only crate."""
    if (crate / "src" / "lib.rs").is_file():
        return ("lib", None)
    names = bin_names(crate)
    if len(names) == 1:
        return ("bin", names[0])
    if names:
        return ("bins", None)
    return ("lib", None)


def module_filter(relative: Path) -> str | None:
    """Return the test filter of a source file, or None when it has no module.

    A filter is the module path with a trailing `::`, which is what the test
    harness matches against a test name. `src/board/tests/mod.rs` becomes
    `board::tests::`; `src/lib.rs` has no module below the crate root.
    """
    parts = relative.parts
    if not parts or parts[0] != "src":
        return None
    stem = Path(parts[-1]).stem
    # `src/bin/...` and `src/main.rs` are binaries, not modules of the library.
    if len(parts) > 1 and parts[1] in {"bin", "main.rs"}:
        return None
    modules = list(parts[1:-1])
    if stem not in {"lib", "main", "mod"}:
        modules.append(stem)
    if not modules:
        return None
    return "::".join(modules) + "::"


def selection_for(path: Path, crate: Path) -> Selection | str:
    """Return the selection for one file, or a note when it covers no test."""
    name = crate_name(crate)
    if name is None:
        return "the workspace manifest changes how every crate builds"
    inner = path.relative_to(crate)
    parts = inner.parts
    target = default_target(crate)
    if not parts or parts[0] == "Cargo.toml":
        return Selection(name, target)
    if parts[0] == "tests":
        # A file directly below tests/ is its own integration target; a helper
        # module or a nested directory is not, so cover every target instead.
        if len(parts) == 2:
            return Selection(name, ("test", Path(parts[1]).stem))
        return Selection(name, ("tests", None))
    if parts[0] == "examples":
        # Cargo discovers `examples/<name>.rs` and `examples/<name>/main.rs`.
        # A fixture beside them belongs to every example that reads it.
        if len(parts) == 2 and parts[1].endswith(".rs"):
            return Selection(name, ("example", Path(parts[1]).stem))
        if len(parts) > 2:
            return Selection(name, ("example", parts[1]))
        return Selection(name, ("examples", None))
    if inner.name == "build.rs":
        return Selection(name, target)
    if parts[0] != "src":
        return f"`{inner}` belongs to no test target"
    if len(parts) == 3 and parts[1] == "bin":
        return Selection(name, ("bin", Path(parts[2]).stem))
    filter_ = module_filter(inner)
    if filter_ is None:
        return Selection(name, target)
    return Selection(name, target, (filter_,))


def build_plan(paths: list[str], root: Path = ROOT) -> Plan:
    """Map changed paths to the test selections they need."""
    plan = Plan()
    for entry in paths:
        path = root / entry
        crate = crate_root(path, root)
        if crate is None:
            if Path(entry).name in WORKSPACE_WIDE:
                reason = "a workspace-wide file: run the full matrix"
            else:
                reason = "outside every crate: run the full matrix"
            plan.unselected.append((entry, reason))
            continue
        outcome = selection_for(path, crate)
        if isinstance(outcome, str):
            plan.unselected.append((entry, outcome))
        else:
            plan.add(outcome)
    return plan


def filterset(selection: Selection) -> str:
    """Return the nextest filterset expression of one selection."""
    return " | ".join(f"test({item})" for item in selection.filters or ())


def commands(
    selection: Selection,
    use_nextest: bool,
    whole_crate: bool,
    features: str | None = None,
) -> list[list[str]]:
    """Return the cargo commands of one selection."""
    target_flag, target_name = selection.target
    target = [f"--{target_flag}"]
    if target_name is not None:
        target.append(target_name)
    extra = ["--features", features] if features else []
    if use_nextest:
        command = ["cargo", "nextest", "run", "-p", selection.crate, *target]
        if selection.filters is not None and not whole_crate:
            command += ["-E", filterset(selection)]
        return [command + extra]
    if whole_crate or selection.filters is None:
        return [["cargo", "test", "-p", selection.crate, *target, *extra]]
    return [
        ["cargo", "test", "-p", selection.crate, *target, *extra, filter_]
        for filter_ in selection.filters
    ]


def nextest_available() -> bool:
    """Return whether cargo-nextest answers here."""
    try:
        result = subprocess.run(
            ["cargo", "nextest", "--version"], capture_output=True, check=False
        )
    except OSError:
        return False
    return result.returncode == 0


def has_tests(output: str) -> bool:
    """Return whether a test run selected at least one test.

    libtest answers `test result: ok. 0 passed; ... filtered out` and nextest
    fails with `no tests to run`. Both mean the filter missed, which is not a
    test failure.
    """
    if "no tests to run" in output:
        return False
    if re.search(r"^test result: .*\b0 passed; 0 failed\b", output, re.MULTILINE):
        return False
    return not re.search(r"\d+ tests run: 0 passed", output)


def report(plan: Plan) -> None:
    """Print the selections of the plan and the paths that selected nothing."""
    print("Test focus")
    for selection in plan.selections:
        target = selection.target[0] + (
            f" {selection.target[1]}" if selection.target[1] else ""
        )
        filters = ", ".join(selection.filters or ()) or "whole target"
        print(f"  {selection.crate}: {target} -> {filters}")
    for entry, reason in plan.unselected:
        print(f"  skipped {entry}: {reason}")
    if not plan.selections and not plan.unselected:
        print("No changed paths. Name paths, or set HORIZON_TEST_FOCUS_BASE.")


def main(argv: list[str] | None = None) -> int:
    """Run the plan, or print its commands."""
    parser = argparse.ArgumentParser(
        description="Run the tests of the paths a change touches."
    )
    parser.add_argument("paths", nargs="*", help="changed paths, default git")
    parser.add_argument("--print", dest="print_only", action="store_true")
    parser.add_argument("--all", dest="whole_crate", action="store_true")
    parser.add_argument("--no-nextest", dest="no_nextest", action="store_true")
    parser.add_argument("--keep-going", action="store_true")
    parser.add_argument(
        "--features", help="cargo features for the run, for a feature-gated module"
    )
    arguments = parser.parse_args(argv)

    paths = arguments.paths or changed_paths()
    plan = build_plan(paths)
    report(plan)
    if not plan.selections:
        return 0

    use_nextest = not arguments.no_nextest and nextest_available()
    runs = [
        (selection, command)
        for selection in plan.selections
        for command in commands(
            selection, use_nextest, arguments.whole_crate, arguments.features
        )
    ]
    if arguments.print_only:
        for _, command in runs:
            print(f"$ {shlex.join(command)}")
        return 0

    status = 0
    for selection, command in runs:
        print(f"\n$ {shlex.join(command)}", flush=True)
        result = subprocess.run(
            command, cwd=ROOT, capture_output=True, text=True, check=False
        )
        sys.stdout.write(result.stdout)
        sys.stderr.write(result.stderr)
        if not has_tests(result.stdout + result.stderr):
            print(
                f"warning: the filter selected no test in {selection.crate}. "
                f"If the module is behind a cargo feature, pass it with "
                f"--features; otherwise run the whole target with --all.",
                file=sys.stderr,
            )
            continue
        if result.returncode == 0:
            continue
        status = result.returncode
        if not arguments.keep_going:
            return status
    return status


if __name__ == "__main__":
    sys.exit(main())
