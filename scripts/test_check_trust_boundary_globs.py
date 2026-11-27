#!/usr/bin/env python3
"""Tests for scripts/check-trust-boundary-globs.py."""

import importlib.util
import shutil
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "check-trust-boundary-globs.py"

FIXTURE_FILES = (
    "pkg/readme.md",
    "pkg/src/left/mod.rs",
    "pkg/src/left/deep/leaf.rs",
    "pkg/src/right.rs",
)


def load_module():
    """Import the hyphenated script by path."""
    spec = importlib.util.spec_from_file_location("check_trust_boundary_globs", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


check = load_module()


class FilesOfTests(unittest.TestCase):
    """A glob of a page names the same files on every Python version."""

    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="trust-boundary-globs-"))
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)
        for name in FIXTURE_FILES:
            file = self.root / name
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_text("", encoding="utf-8")
        self.addCleanup(setattr, check, "ROOT", check.ROOT)
        check.ROOT = self.root

    def names_of(self, glob):
        return sorted(path.relative_to(self.root).as_posix() for path in check.files_of(glob))

    def test_glob_that_ends_in_double_star_names_the_files_below_the_directory(self):
        self.assertEqual(
            self.names_of("pkg/src/left/**"),
            ["pkg/src/left/deep/leaf.rs", "pkg/src/left/mod.rs"],
        )

    def test_recursive_glob_of_the_root_names_every_file(self):
        self.assertEqual(self.names_of("pkg/**"), sorted(FIXTURE_FILES))

    def test_glob_that_names_a_file_still_names_only_that_file(self):
        self.assertEqual(self.names_of("pkg/src/left/mod.rs"), ["pkg/src/left/mod.rs"])
        self.assertEqual(self.names_of("pkg/src/*.rs"), ["pkg/src/right.rs"])

    def test_glob_of_a_missing_path_names_no_file(self):
        self.assertEqual(self.names_of("pkg/src/missing/**"), [])


if __name__ == "__main__":
    unittest.main()
