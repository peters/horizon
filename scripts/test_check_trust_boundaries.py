#!/usr/bin/env python3
"""Tests for scripts/check-trust-boundaries.py."""

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "check-trust-boundaries.py"
BOUNDARIES = "docs/review/trust-boundaries"


def fixture(root, applyto, body=""):
    """A repository with one boundary page and one file that a glob can match."""
    (root / BOUNDARIES).mkdir(parents=True, exist_ok=True)
    (root / "crates/example/src").mkdir(parents=True, exist_ok=True)
    (root / "crates/example/src/thing.rs").write_text("// fixture\n", encoding="utf-8")
    page = root / BOUNDARIES / "example.md"
    page.write_text(f'---\napplyTo: "{applyto}"\n---\n\n# Example boundary\n\n{body}\n', encoding="utf-8")
    return page


def run(root):
    return subprocess.run(
        [sys.executable, "-I", str(SCRIPT), "--root", str(root)],
        capture_output=True,
        text=True,
        check=False,
    )


class CheckTrustBoundaries(unittest.TestCase):
    def test_the_repository_routes_every_boundary_today(self):
        """The rule holds on this checkout, so the check is a guardrail and not a backlog."""
        result = run(ROOT)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("boundary pages route", result.stdout)

    def test_a_glob_that_matches_no_file_is_an_error(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root, "crates/example/src/thing.rs,crates/example/src/absent.rs")
            result = run(root)
            self.assertEqual(result.returncode, 1)
            self.assertIn("matches no file", result.stderr)
            self.assertIn("absent.rs", result.stderr)

    def test_a_brace_list_is_an_error(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root, "crates/example/src/{thing,other}.rs")
            result = run(root)
            self.assertEqual(result.returncode, 1)
            self.assertIn("brace list", result.stderr)

    def test_a_backticked_path_that_does_not_exist_is_an_error(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root, "crates/example/src/thing.rs", body="Read `crates/example/src/gone.rs` first.")
            result = run(root)
            self.assertEqual(result.returncode, 1)
            self.assertIn("does not exist", result.stderr)
            self.assertIn("gone.rs", result.stderr)

    def test_a_backticked_symbol_is_not_read_as_a_path(self):
        """A boundary names flags, symbols and formats in backticks; only paths are checked."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(
                root,
                "crates/example/src/thing.rs",
                body="It refuses `--reuse-state`, returns `Ok(false)` and reads `schema_version` 1.",
            )
            result = run(root)
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_a_page_without_apply_to_is_an_error(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / BOUNDARIES).mkdir(parents=True)
            (root / BOUNDARIES / "example.md").write_text("# No front matter\n", encoding="utf-8")
            result = run(root)
            self.assertEqual(result.returncode, 1)
            self.assertIn("never routes", result.stderr)

    def test_a_bare_folder_name_routes_nothing(self):
        """A plain glob matches the folder itself, so a bare name would pass while routing
        no file. docs/review/README.md requires a match on a file."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root, "crates/example/src")
            result = run(root)
            self.assertEqual(result.returncode, 1)
            self.assertIn("matches no file", result.stderr)
            self.assertIn("name a tree", result.stderr)

    def test_a_folder_tree_glob_routes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root, "crates/example/src/**")
            result = run(root)
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
