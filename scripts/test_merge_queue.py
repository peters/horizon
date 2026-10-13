#!/usr/bin/env python3
"""Tests for scripts/merge-queue.py. They use local fixture repositories and plain
data, never GitHub."""

import importlib.util
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location("merge_queue", ROOT / "scripts" / "merge-queue.py")
mq = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(mq)

GIT_ENV = {
    "GIT_AUTHOR_NAME": "Queue Fixture",
    "GIT_AUTHOR_EMAIL": "queue@example.invalid",
    "GIT_COMMITTER_NAME": "Queue Fixture",
    "GIT_COMMITTER_EMAIL": "queue@example.invalid",
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_SYSTEM": os.devnull,
}
HEAD_A = "a" * 40


def comment(body, created_at="2026-10-13T08:00:00Z", association="OWNER"):
    return {"body": body, "created_at": created_at, "author_association": association}


class MessageTests(unittest.TestCase):
    def test_a_plain_message_passes(self):
        self.assertEqual(mq.message_problems("fix(cloud): keep the panel", "Part of #1506."), [])

    def test_refused_messages_name_the_reason(self):
        cases = [
            ("ci: tidy", "Body " + mq.SKIP_MARKER, "skip ci"),
            ("ci: tidy", "Co-Authored-By: someone", "co-authored-by"),
            ("ci: tidy", "This Fixes #12.", "closing keyword"),
            ("ci: tidy", "resolves peters/horizon#7", "closing keyword"),
            ("  ", "body", "empty"),
            ("a\nb", "", "more than one line"),
        ]
        for subject, body, reason in cases:
            with self.subTest(reason=reason, body=body):
                problems = mq.message_problems(subject, body)
                self.assertTrue(any(reason in p.lower() for p in problems), problems)

    def test_words_that_only_look_like_keywords_pass(self):
        for body in ["Part of #1534.", "the fix for issue 12", "closer to #3 would be nice",
                     "Prefixes #4 are fine"]:
            with self.subTest(body=body):
                self.assertEqual(mq.message_problems("ci: tidy", body), [])


class EntryTests(unittest.TestCase):
    def test_an_entry_round_trips(self):
        text = mq.entry_comment(HEAD_A, "ci: add a queue", "Line one\n\n```\ncode\n```\n")
        self.assertEqual(mq.parse_entry(text), {"head": HEAD_A, "subject": "ci: add a queue",
                                                "body": "Line one\n\n```\ncode\n```\n"})

    def test_malformed_entries_are_ignored(self):
        bad = [
            "no marker\n```json\n{}\n```",
            mq.MARKER + "\nno json",
            mq.MARKER + '\n```json\n{"head": "abc", "subject": "s", "body": ""}\n```',
            mq.MARKER + '\n```json\n{"head": "' + HEAD_A + '", "subject": 3, "body": ""}\n```',
            mq.MARKER + "\n```json\n{not json}\n```",
        ]
        for body in bad:
            with self.subTest(body=body):
                self.assertIsNone(mq.parse_entry(body))

    def test_the_newest_trusted_entry_wins(self):
        old = mq.entry_comment(HEAD_A, "old", "")
        new = mq.entry_comment("b" * 40, "new", "")
        forged = mq.entry_comment("c" * 40, "forged", "")
        entry = mq.latest_entry([
            comment(new, "2026-10-13T09:00:00Z"),
            comment(old, "2026-10-13T08:00:00Z"),
            comment(forged, "2026-10-13T10:00:00Z", association="NONE"),
        ])
        self.assertEqual((entry["subject"], entry["queued_at"]), ("new", "2026-10-13T09:00:00Z"))

    def test_a_later_dequeue_note_voids_the_entry(self):
        entries = [comment(mq.entry_comment(HEAD_A, "s", ""), "2026-10-13T08:00:00Z"),
                   comment(mq.DEQUEUED + "\nRemoved", "2026-10-13T09:00:00Z")]
        self.assertIsNone(mq.latest_entry(entries))
        entries.append(comment(mq.entry_comment(HEAD_A, "again", ""), "2026-10-13T10:00:00Z"))
        self.assertEqual(mq.latest_entry(entries)["subject"], "again")

    def test_queue_order_is_entry_time_then_number(self):
        entries = [{"number": 9, "queued_at": "2026-10-13T08:00:00Z"},
                   {"number": 3, "queued_at": "2026-10-13T09:00:00Z"},
                   {"number": 5, "queued_at": "2026-10-13T08:00:00Z"}]
        self.assertEqual([e["number"] for e in mq.queue_order(entries)], [5, 9, 3])

    def test_drift_compares_the_recorded_head(self):
        self.assertFalse(mq.drifted({"head": HEAD_A}, HEAD_A))
        self.assertTrue(mq.drifted({"head": HEAD_A}, "b" * 40))


class CiVerdictTests(unittest.TestCase):
    def run_(self, status="completed", conclusion="success", name="CI"):
        return {"name": name, "status": status, "conclusion": conclusion}

    def test_all_completed_and_good_passes(self):
        passed, problems = mq.ci_verdict(
            [self.run_()], [self.run_(name="Clippy"), self.run_(conclusion="skipped", name="Snap")])
        self.assertTrue(passed, problems)

    def test_open_failed_cancelled_or_missing_blocks(self):
        cases = [
            ([], []),
            ([self.run_(status="in_progress", conclusion=None)], []),
            ([self.run_()], [self.run_(conclusion="failure", name="Tests (macOS)")]),
            ([self.run_()], [self.run_(conclusion="cancelled", name="Tests (Windows x64 ui)")]),
            ([self.run_()], [self.run_(status="queued", conclusion=None, name="Clippy")]),
        ]
        for runs, checks in cases:
            with self.subTest(runs=runs, checks=checks):
                passed, problems = mq.ci_verdict(runs, checks)
                self.assertFalse(passed)
                self.assertTrue(problems)


class BatchTests(unittest.TestCase):
    """apply_batch on a fixture repository whose PR heads are local branches."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="merge-queue-test-"))
        self.addCleanup(subprocess.run, ["rm", "-rf", str(self.tmp)], check=False)
        os.environ["XDG_STATE_HOME"] = str(self.tmp / "state")
        self.addCleanup(os.environ.pop, "XDG_STATE_HOME", None)
        for key, value in GIT_ENV.items():
            os.environ[key] = value
            self.addCleanup(os.environ.pop, key, None)
        self.repo = self.tmp / "repo"
        self.repo.mkdir()
        self.git("init", "-q", "-b", "main")
        self.write("README.md", "base\n")
        self.base = self.commit("base")

    def git(self, *args):
        return subprocess.run(["git", *args], cwd=self.repo, capture_output=True, text=True,
                              check=True).stdout.strip()

    def write(self, path, text):
        (self.repo / path).write_text(text)

    def commit(self, message):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", message)
        return self.git("rev-parse", "HEAD")

    def branch(self, name, path, text):
        self.git("checkout", "-q", "-b", name, self.base)
        self.write(path, text)
        head = self.commit(f"{name} work")
        self.git("checkout", "-q", "main")
        return head

    def entry(self, number, head, subject):
        return {"number": number, "head": head, "subject": subject, "body": f"Body of {number}."}

    def test_heads_land_as_one_squash_commit_each_in_order(self):
        a = self.branch("a", "a.txt", "a\n")
        b = self.branch("b", "b.txt", "b\n")
        tip, applied, skipped = mq.apply_batch(
            self.repo, self.base, [self.entry(1, a, "feat: a"), self.entry(2, b, "fix: b")],
            lambda entry: None)
        self.assertEqual([e["number"] for e in applied], [1, 2])
        self.assertEqual(skipped, [])
        self.assertEqual(self.git("log", "-1", "--format=%B", tip), "fix: b\n\nBody of 2.")
        self.assertEqual(self.git("log", "-1", "--format=%s", f"{tip}~1"), "feat: a")
        self.assertEqual(self.git("rev-list", "--count", f"{self.base}..{tip}"), "2")
        self.assertEqual(self.git("show", f"{tip}:a.txt"), "a")
        self.assertEqual(self.git("worktree", "list").count("\n"), 0, "the batch worktree is gone")

    def test_a_conflicting_or_empty_head_is_skipped_and_the_rest_still_apply(self):
        first = self.branch("first", "same.txt", "one\n")
        clash = self.branch("clash", "same.txt", "two\n")
        empty = self.base
        other = self.branch("other", "other.txt", "x\n")
        entries = [self.entry(1, first, "a"), self.entry(2, clash, "b"),
                   self.entry(3, empty, "c"), self.entry(4, other, "d")]
        tip, applied, skipped = mq.apply_batch(self.repo, self.base, entries, lambda e: None)
        self.assertEqual([e["number"] for e in applied], [1, 4])
        self.assertEqual([(e["number"], "conflicts" in r or "nothing" in r) for e, r in skipped],
                         [(2, True), (3, True)])
        self.assertEqual(self.git("show", f"{tip}:same.txt"), "one")

    def test_a_head_that_cannot_be_fetched_is_skipped(self):
        def fetch(entry):
            raise mq.QueueError("gone")
        tip, applied, skipped = mq.apply_batch(
            self.repo, self.base, [self.entry(1, self.base, "a")], fetch)
        self.assertEqual((tip, applied), (self.base, []))
        self.assertIn("gone", skipped[0][1])

    def test_trees_match_compares_content_not_history(self):
        a = self.branch("a", "a.txt", "a\n")
        tip, _applied, _skipped = mq.apply_batch(self.repo, self.base,
                                                 [self.entry(1, a, "a")], lambda e: None)
        self.git("merge", "-q", "--squash", a)
        squash = self.commit("as GitHub would squash it")
        self.assertNotEqual(squash, tip)
        self.assertTrue(mq.trees_match(self.repo, squash, tip))
        self.assertFalse(mq.trees_match(self.repo, self.base, tip))


if __name__ == "__main__":
    unittest.main()
