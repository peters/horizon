#!/usr/bin/env python3
"""Check that every trust boundary page names files that exist.

Each page under `docs/review/trust-boundaries/` starts with one `applyTo` line
that lists the globs of the files it governs. The review reads that line to route
a changed file to its page, so a glob that matches no file routes nothing: it is
a stale name of a file that moved, and a changed file of that area then reaches a
review without its boundary.

This check fails when a page holds no `applyTo` line, and when one of its globs
matches no file of the repository.
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
PAGES = ROOT / "docs" / "review" / "trust-boundaries"
APPLY_TO = re.compile(r'^applyTo:[ \t]*"(?P<globs>[^"]*)"[ \t]*$', re.M)


def globs_of(page: pathlib.Path) -> list[str]:
    """The globs of one page, in the order that its `applyTo` line names them."""
    match = APPLY_TO.search(page.read_text(encoding="utf-8"))
    if match is None:
        raise SystemExit(f"{page}: the page has no applyTo line")
    return [glob.strip() for glob in match.group("globs").split(",") if glob.strip()]


def files_of(glob: str) -> list[pathlib.Path]:
    """The files of the repository that `glob` names, relative to the root."""
    return [path for path in ROOT.glob(glob) if path.is_file()]


def main() -> int:
    pages = sorted(PAGES.glob("*.md"))
    if not pages:
        print(f"{PAGES}: no trust boundary page", file=sys.stderr)
        return 1
    status = 0
    globs = 0
    for page in pages:
        for glob in globs_of(page):
            globs += 1
            if files_of(glob):
                continue
            print(
                f"{page.relative_to(ROOT)}: the applyTo glob {glob!r} matches no file",
                file=sys.stderr,
            )
            status = 1
    if status == 0:
        print(f"{len(pages)} trust boundary pages name {globs} globs that match a file")
    return status


if __name__ == "__main__":
    sys.exit(main())
