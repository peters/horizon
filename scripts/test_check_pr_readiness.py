#!/usr/bin/env python3
"""Tests for scripts/check-pr-readiness.py."""

import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "check-pr-readiness.py"

GIT_ENV = {
    "GIT_AUTHOR_NAME": "CI Fixture",
    "GIT_AUTHOR_EMAIL": "ci@example.invalid",
    "GIT_COMMITTER_NAME": "CI Fixture",
    "GIT_COMMITTER_EMAIL": "ci@example.invalid",
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_SYSTEM": os.devnull,
}


class ReadinessTests(unittest.TestCase):
    def setUp(self):
        self.repo = Path(tempfile.mkdtemp(prefix="pr-readiness-"))
        self.addCleanup(subprocess.run, ["rm", "-rf", str(self.repo)], check=False)
        self.git("init", "-q", "-b", "main")
        self.write("README.md", "base\n")
        self.commit("base")
        self.base = self.git("rev-parse", "HEAD").strip()

    def git(self, *args):
        env = {**os.environ, **GIT_ENV}
        return subprocess.run(["git", "-C", str(self.repo), *args], env=env, check=True,
                              capture_output=True, text=True).stdout

    def write(self, path, text="x\n"):
        file = self.repo / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text)

    def commit(self, message="change"):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", message)

    def run_check(self, *args, body=None):
        extra = []
        if body is not None:
            body_file = self.repo.parent / f"{self.repo.name}-body.md"
            body_file.write_text(body)
            self.addCleanup(body_file.unlink, missing_ok=True)
            extra = ["--body", str(body_file)]
        out = subprocess.run([sys.executable, str(SCRIPT), "--repo", str(self.repo), "--base", self.base, *args, *extra],
                             capture_output=True, text=True, check=False)
        return out.returncode, out.stdout

    def test_small_change_passes(self):
        self.write("crates/horizon-core/src/lib.rs")
        self.commit()
        self.assertEqual(self.run_check()[0], 0)

    def test_scope_needs_approval(self):
        for i in range(11):
            self.write(f"crates/horizon-core/src/m{i}.rs")
        self.commit()
        code, out = self.run_check()
        self.assertEqual(code, 1)
        self.assertIn("error scope:", out)
        self.assertEqual(self.run_check("--scope-approved")[0], 0)

    def test_scope_counts_lines(self):
        self.write("crates/horizon-core/src/big.rs", "x\n" * 1501)
        self.commit()
        self.assertIn("error scope:", self.run_check()[1])

    def test_test_procedures_do_not_count_toward_scope(self):
        for i in range(11):
            self.write(f"docs/testing/procedures/p{i}.md")
        self.commit()
        self.assertEqual(self.run_check()[0], 0)

    def test_ui_change_needs_procedure_and_gif(self):
        self.write("crates/horizon-ui/src/app/panel.rs")
        self.commit()
        code, out = self.run_check(body="Changes the panel.\n")
        self.assertEqual(code, 1)
        self.assertIn("error ui-procedure:", out)
        self.assertIn("error ui-gif:", out)
        self.write("docs/testing/procedures/panel.md")
        self.commit()
        body = "![panel](https://github.com/user-attachments/assets/abc)\n\nChanges the panel.\n"
        self.assertEqual(self.run_check(body=body)[0], 0)

    def test_ui_gif_low_in_the_body_is_a_note(self):
        self.write("crates/horizon-ui/src/app/panel.rs")
        self.write("docs/testing/procedures/panel.md")
        self.commit()
        body = "".join(f"Line {i}.\n" for i in range(20)) + "![panel](https://example.invalid/panel.gif)\n"
        code, out = self.run_check(body=body)
        self.assertEqual(code, 0)
        self.assertIn("note  ui-gif:", out)

    def test_ui_gif_must_be_embedded(self):
        self.write("crates/horizon-ui/src/app/panel.rs")
        self.write("docs/testing/procedures/panel.md")
        self.commit()
        for body in ["The recording is recording.gif.\n", "See https://github.com/user-attachments/assets/abc for details.\n",
                     "![still](https://example.invalid/shot.png)\n", "<video src=\"https://example.invalid/a.mp4\"></video>\n"]:
            with self.subTest(body=body):
                self.assertIn("error ui-gif:", self.run_check(body=body)[1])
        for body in ["<img src=\"https://example.invalid/a.gif\" width=\"600\">\n", "https://github.com/user-attachments/assets/abc\n",
                     "![flow](https://github.com/user-attachments/assets/abc)\n"]:
            with self.subTest(body=body):
                self.assertEqual(self.run_check(body=body)[0], 0)

    def test_untracked_files_give_a_note(self):
        self.write("crates/horizon-core/src/lib.rs")
        self.commit()
        self.write("crates/horizon-core/src/new.rs")
        self.assertIn("note  uncommitted:", self.run_check()[1])

    def test_ui_gif_is_a_note_without_body(self):
        self.write("crates/horizon-ui/src/app/panel.rs")
        self.write("docs/testing/procedures/panel.md")
        self.commit()
        code, out = self.run_check()
        self.assertEqual(code, 0)
        self.assertIn("note  ui-gif:", out)

    def test_ui_tests_and_invisible_changes(self):
        self.write("crates/horizon-ui/src/app/panel/tests.rs")
        self.commit()
        self.assertEqual(self.run_check()[0], 0)
        self.write("crates/horizon-ui/src/app/panel.rs")
        self.commit()
        self.assertEqual(self.run_check()[0], 1)
        self.assertEqual(self.run_check("--no-visible-change")[0], 0)

    def test_feature_title_needs_procedure(self):
        self.write("crates/horizon-core/src/lib.rs")
        self.commit()
        code, out = self.run_check("--title", "feat(cloud): add a thing")
        self.assertEqual(code, 1)
        self.assertIn("error feature-procedure:", out)
        self.assertEqual(self.run_check("--title", "fix(cloud): repair a thing")[0], 0)

    def test_skill_copies_change_together(self):
        self.write("assets/plugins/claude-code/skills/horizon-cloud/SKILL.md")
        self.commit()
        self.assertIn("error skill-copies:", self.run_check()[1])
        self.write("assets/plugins/codex/skills/horizon-cloud/SKILL.md")
        self.commit()
        self.assertEqual(self.run_check()[0], 0)

    def test_new_plans_go_under_procedures(self):
        self.write("docs/testing/2026-10-10-thing-smoke.md")
        self.commit()
        self.assertIn("error test-plan-location:", self.run_check()[1])

    def test_skip_marker_on_head_is_a_note(self):
        self.write("crates/horizon-core/src/lib.rs")
        self.commit("fix a thing [" + "skip ci]")
        code, out = self.run_check()
        self.assertEqual(code, 0)
        self.assertIn("note  skip-marker:", out)


if __name__ == "__main__":
    unittest.main()
