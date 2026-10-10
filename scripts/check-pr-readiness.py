#!/usr/bin/env python3
"""Check a branch against the pull request rules in AGENTS.md before the PR opens.

Copilot reports these rules as review findings, and each finding costs a review
round. The check finds them in a second instead:

- scope: more than 10 source or test files, or more than 1,500 lines, needs approval;
- UI changes: a test procedure, and an animated GIF in the PR body (at the top);
- features (a `feat` title): a test procedure;
- the two bundled skill copies change together;
- new test plans go under docs/testing/procedures/ or docs/testing/reports/.

Usage: check-pr-readiness.py [--base REF] [--title TEXT] [--body FILE]
       [--scope-approved] [--no-visible-change]
"""

import argparse
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CODE = {".rs", ".py", ".sh", ".ps1", ".js", ".cjs", ".mjs", ".ts", ".tsx", ".swift", ".kt", ".java", ".c", ".h", ".m"}
SKILL_COPIES = ("assets/plugins/claude-code/skills/", "assets/plugins/codex/skills/")
UI_SOURCE = "crates/horizon-ui/src/"
PROCEDURES = "docs/testing/procedures/"
MAX_FILES = 10
MAX_LINES = 1500
SKIP_MARKER = "[" + "skip ci]"


def git(repo, *args):
    out = subprocess.run(["git", "-C", str(repo), "-c", "core.quotepath=off", *args],
                         capture_output=True, text=True, check=False)
    if out.returncode != 0:
        raise SystemExit(f"check-pr-readiness: git {' '.join(args)}: {out.stderr.strip()}")
    return out.stdout


def changed_files(repo, base):
    """(path, added, deleted, status) for each file that differs from the base."""
    status = {}
    for line in git(repo, "diff", "--name-status", "--no-renames", base, "HEAD").splitlines():
        code, path = line.split("\t", 1)
        status[path] = code[0]
    files = []
    for line in git(repo, "diff", "--numstat", "--no-renames", base, "HEAD").splitlines():
        added, deleted, path = line.split("\t", 2)
        files.append((path, 0 if added == "-" else int(added), 0 if deleted == "-" else int(deleted), status.get(path, "M")))
    return files


def is_test(path):
    return "/tests/" in path or path.endswith(("tests.rs", "_test.rs", "_tests.rs")) or Path(path).name.startswith("test_")


def is_source(path):
    return Path(path).suffix in CODE and not path.startswith("docs/testing/")


def check(repo, base, title, body, scope_approved, no_visible_change):
    files = changed_files(repo, base)
    paths = {path for path, _, _, _ in files}
    errors, notes = [], []

    source = [(p, a, d) for p, a, d, _ in files if is_source(p)]
    lines = sum(a + d for _, a, d in source)
    if (len(source) > MAX_FILES or lines > MAX_LINES) and not scope_approved:
        errors.append(("scope", f"{len(source)} source or test files and {lines} changed lines. More than "
                       f"{MAX_FILES} files or {MAX_LINES} lines needs explicit approval from peters. After the "
                       "approval, run again with --scope-approved, or split the PR."))

    procedure_changed = any(p.startswith(PROCEDURES) and p.endswith(".md") and not p.endswith("TEMPLATE.md") for p in paths)
    ui = sorted(p for p in paths if p.startswith(UI_SOURCE) and p.endswith(".rs") and not is_test(p))
    if ui and not no_visible_change:
        if not procedure_changed:
            errors.append(("ui-procedure", f"{len(ui)} UI source files changed (for example {ui[0]}), but no test "
                           f"procedure under {PROCEDURES} changed. If the change is not visible, say so in the PR "
                           "body and run again with --no-visible-change."))
        if body is None:
            notes.append(("ui-gif", "UI files changed: pass --body FILE to check for the animated GIF."))
        else:
            lines = [l for l in body.splitlines() if l.strip()]
            media = [i for i, l in enumerate(lines) if re.search(r"\.gif\b|user-attachments/assets/", l, re.I)]
            if not media:
                errors.append(("ui-gif", "UI files changed, but the PR body has no animated GIF of the change."))
            elif media[0] >= 15:
                notes.append(("ui-gif", "AGENTS.md asks for the animated GIF at the top of the PR body."))

    if title and re.match(r"feat(\(|:|!)", title.strip()) and not procedure_changed:
        errors.append(("feature-procedure", f"The title marks a feature, but no test procedure under {PROCEDURES} changed."))

    for path in sorted(paths):
        for mine, other in (SKILL_COPIES, SKILL_COPIES[::-1]):
            if path.startswith(mine) and other + path[len(mine):] not in paths:
                errors.append(("skill-copies", f"{path} changed, but {other + path[len(mine):]} did not. Change both "
                               "bundled skill copies together."))

    for path, _, _, status in files:
        if status == "A" and re.fullmatch(r"docs/testing/[^/]+\.md", path):
            errors.append(("test-plan-location", f"{path} is a new plan directly under docs/testing/. Put a "
                           f"procedure under {PROCEDURES} or a report under docs/testing/reports/."))

    if SKIP_MARKER in git(repo, "log", "-1", "--format=%B"):
        notes.append(("skip-marker", "The head commit has the skip marker, so GitHub runs no CI for this head. "
                      "Push the final head without it."))
    if git(repo, "status", "--porcelain", "--untracked-files=no").strip():
        notes.append(("uncommitted", "There are uncommitted changes; the check covers committed changes only."))
    return errors, notes


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--repo", default=str(ROOT), help=argparse.SUPPRESS)
    parser.add_argument("--base", help="the base commit (default: the merge base with origin/main)")
    parser.add_argument("--title", help="the PR title")
    parser.add_argument("--body", help="a file with the PR body")
    parser.add_argument("--scope-approved", action="store_true", help="peters approved a larger PR")
    parser.add_argument("--no-visible-change", action="store_true", help="the UI files change no visible behavior")
    args = parser.parse_args(argv)
    repo = Path(args.repo)
    base = args.base or git(repo, "merge-base", "HEAD", "origin/main").strip()
    body = Path(args.body).read_text(encoding="utf-8") if args.body else None
    errors, notes = check(repo, base, args.title, body, args.scope_approved, args.no_visible_change)
    for rule, message in errors:
        print(f"error {rule}: {message}")
    for rule, message in notes:
        print(f"note  {rule}: {message}")
    print(f"check-pr-readiness: {len(errors)} errors, {len(notes)} notes")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
