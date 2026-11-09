#!/usr/bin/env python3
"""Keep every trust boundary routing, so a review cannot miss a subsystem.

A boundary page routes by its `applyTo` globs. An entry that matches no file, or
that uses a brace list, silently routes nothing: the review then never reads that
boundary, and the omission looks exactly like a subsystem that needs no boundary.

This applies steps 1 and 3 of the method in docs/review/trust-boundary-audit.md,
which are the steps that a script can decide:

1. Every `applyTo` entry matches at least one file, and no entry uses a brace list.
3. Every path that a page names in backticks exists.

The remaining steps read the text against the code, which a person or a review does.
"""

import argparse
import glob
import pathlib
import re
import sys


ROOT = pathlib.Path(__file__).resolve().parent.parent
BOUNDARIES = "docs/review/trust-boundaries"
APPLY_TO = re.compile(r'^applyTo:\s*"([^"]*)"\s*$', re.MULTILINE)
BACKTICKED = re.compile(r"`([^`\n]+)`")
# Only a token that is unmistakably a repository path is checked. A boundary also
# names symbols, flags and formats in backticks, and those are not paths.
PATH_SHAPED = re.compile(r"^(crates|scripts|docs|auto|\.github)/[A-Za-z0-9_./*-]+$")


def matches_a_file(pattern, root):
    """Whether `pattern` matches at least one file under `root`.

    `docs/review/README.md` requires a match on a file. A plain glob also matches a
    folder, so a bare folder name would pass while routing nothing.
    """
    return any((root / hit).is_file() for hit in glob.glob(pattern, root_dir=root, recursive=True))


def entries(text):
    """The `applyTo` globs of one page, in order."""
    found = APPLY_TO.search(text)
    if found is None:
        return None
    return [entry.strip() for entry in found.group(1).split(",") if entry.strip()]


def check(root):
    """Every problem found under `root`, as a list of messages."""
    pages = sorted((root / BOUNDARIES).glob("*.md"))
    problems = []
    if not pages:
        return [f"{BOUNDARIES} holds no boundary page"]
    for page in pages:
        name = page.relative_to(root)
        text = page.read_text(encoding="utf-8")
        globs = entries(text)
        if globs is None:
            problems.append(f"{name}: no applyTo front matter, so the boundary never routes")
            continue
        if not globs:
            problems.append(f"{name}: applyTo is empty, so the boundary never routes")
        for entry in globs:
            if "{" in entry or "}" in entry:
                problems.append(
                    f"{name}: applyTo entry {entry!r} uses a brace list, which a plain glob "
                    "reader does not match, so the boundary never routes"
                )
                continue
            if not matches_a_file(entry, root):
                problems.append(
                    f"{name}: applyTo entry {entry!r} matches no file, so it routes nothing. "
                    "A bare folder name matches the folder and routes nothing; name a tree "
                    "with `/**`. Correct the entry or remove it."
                )
        for token in BACKTICKED.findall(text):
            token = token.strip()
            if not PATH_SHAPED.match(token):
                continue
            if not glob.glob(token, root_dir=root, recursive=True):
                problems.append(f"{name}: names the path {token!r}, which does not exist")
    return problems


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--root",
        type=pathlib.Path,
        default=ROOT,
        help="The repository to read. The default is the one that holds this script.",
    )
    arguments = parser.parse_args()
    root = arguments.root.resolve()
    problems = check(root)
    for problem in problems:
        print(f"error: {problem}", file=sys.stderr)
    pages = len(sorted((root / BOUNDARIES).glob("*.md")))
    if problems:
        print(
            f"check-trust-boundaries: {len(problems)} problems in {pages} boundary pages",
            file=sys.stderr,
        )
        return 1
    print(f"check-trust-boundaries: {pages} boundary pages route")
    return 0


if __name__ == "__main__":
    sys.exit(main())
