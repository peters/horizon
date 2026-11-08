#!/usr/bin/env python3
"""The cache pruner reports only the generations that no run can reach."""

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "prune-actions-caches.py"
WORKFLOW = ROOT / ".github" / "workflows" / "cache-prune.yml"

_spec = importlib.util.spec_from_file_location("prune_actions_caches", SCRIPT)
prune = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(prune)

NOW = datetime(2026, 11, 8, 12, 0, tzinfo=timezone.utc)
# The two key shapes the repository produces: a rust-cache key and a plain one.
# Both end in the lockfile hash.
RUST_KEY = "v0-rust-linux-test-rust-test-Linux-x64-c1a0964a"
PLAIN_KEY = "device-device-Linux-x64-c1a0964a"


def stamp(days=0, minutes=0):
    return (NOW - timedelta(days=days, minutes=minutes)).isoformat().replace("+00:00", "Z")


def entry(
    identifier,
    key,
    *,
    accessed_days=0,
    created_days=30,
    ref="refs/heads/main",
    size=1024,
    accessed=None,
    created=None,
):
    return {
        "id": identifier,
        "key": key,
        "ref": ref,
        "size_in_bytes": size,
        "created_at": stamp(days=created_days) if created is None else created,
        "last_accessed_at": stamp(days=accessed_days) if accessed is None else accessed,
    }


def ids(payload, **kwargs):
    kwargs.setdefault("now", NOW)
    return [item["id"] for item in prune.superseded(payload, **kwargs)]


IDLE = 3  # days without a read, past the default threshold


class RestoreKeyTests(unittest.TestCase):
    def test_the_trailing_lock_hash_is_not_part_of_the_group(self):
        self.assertEqual(prune.restore_key(f"{RUST_KEY}-af2d2e7c"), RUST_KEY)
        self.assertEqual(prune.restore_key(f"{PLAIN_KEY}-af2d2e7c"), PLAIN_KEY)

    def test_a_key_without_a_separator_is_its_own_group(self):
        self.assertEqual(prune.restore_key("solitary"), "solitary")

    def test_a_longer_key_is_not_grouped_with_the_key_it_starts_with(self):
        # `linux-test` is a prefix of `linux-test-libs`, and the two are separate
        # caches. Only the last component may be dropped.
        shared = prune.restore_key("v0-rust-linux-test-rust-test-Linux-x64-c1a0964a-af2d2e7c")
        dedicated = prune.restore_key("v0-rust-linux-test-libs-rust-test-Linux-x64-c1a0964a-af2d2e7c")
        self.assertNotEqual(shared, dedicated)


class SupersededTests(unittest.TestCase):
    def test_the_generation_runs_still_use_survives(self):
        payload = [
            entry(1, f"{RUST_KEY}-a", accessed_days=IDLE),
            entry(2, f"{RUST_KEY}-b", accessed_days=IDLE),
            entry(3, f"{RUST_KEY}-c", accessed_days=0),
        ]
        self.assertEqual(ids(payload), [2, 1])

    def test_creation_order_does_not_decide_which_generation_survives(self):
        # Measured on this repository: the live `linux-test` entry was created
        # three minutes before the superseded one beside it, and was the only one
        # any run had touched in two days. The hash is not a sequence, so a
        # lockfile that returns to an earlier state keeps an older entry live.
        live = entry(1, f"{RUST_KEY}-af2d2e7c", created_days=2, accessed_days=0)
        superseded = entry(2, f"{RUST_KEY}-83b9b9f8", created=stamp(days=2, minutes=-3), accessed_days=IDLE)
        self.assertEqual(ids([live, superseded]), [2])

    def test_a_generation_a_branch_still_reads_survives_the_order(self):
        # A pull request run restores the entries of its base branch, so a
        # branch still on the old lockfile can take an exact hit on the old
        # generation after main has saved the new one. That read puts the old
        # generation first in the order, and main's own generation second.
        # Deleting the second would give main a cold build, so a generation
        # that anything still reads is kept whatever its place.
        old_generation = entry(1, f"{RUST_KEY}-old", created_days=9, accessed=stamp(minutes=60))
        mains_generation = entry(2, f"{RUST_KEY}-new", created_days=8, accessed=stamp(minutes=120))
        self.assertEqual(ids([old_generation, mains_generation]), [])

    def test_the_old_generation_goes_once_no_branch_reads_it(self):
        old_generation = entry(1, f"{RUST_KEY}-old", created_days=9, accessed_days=IDLE)
        mains_generation = entry(2, f"{RUST_KEY}-new", created_days=8, accessed=stamp(minutes=120))
        self.assertEqual(ids([old_generation, mains_generation]), [1])

    def test_the_newest_entry_by_creation_is_not_protected(self):
        # Measured here: for `linux-test` and `macos-test-libs` the newest entry
        # by creation was the superseded one, 1.28 GB that a rule anchored on
        # creation would keep for ever.
        live = entry(1, f"{RUST_KEY}-af2d2e7c", created=stamp(days=2, minutes=3), accessed_days=0)
        newest_created = entry(2, f"{RUST_KEY}-83b9b9f8", created=stamp(days=2), accessed_days=IDLE)
        self.assertEqual(ids([live, newest_created]), [2])

    def test_max_idle_days_sets_how_long_a_quiet_generation_waits(self):
        payload = [
            entry(1, f"{RUST_KEY}-a", created_days=9, accessed_days=1),
            entry(2, f"{RUST_KEY}-b", created_days=8, accessed_days=0),
        ]
        self.assertEqual(ids(payload), [])
        self.assertEqual(ids(payload, max_idle_days=0.5), [1])

    def test_a_single_generation_is_never_reported(self):
        # A toolchain change leaves a group of one behind. GitHub removes it
        # after 7 days without access, so the pruner leaves it alone.
        payload = [entry(1, f"{RUST_KEY}-only"), entry(2, f"{PLAIN_KEY}-only")]
        self.assertEqual(ids(payload), [])

    def test_keep_can_hold_more_than_one_generation(self):
        payload = [
            entry(1, f"{RUST_KEY}-a", accessed_days=4),
            entry(2, f"{RUST_KEY}-b", accessed_days=IDLE),
            entry(3, f"{RUST_KEY}-c", accessed_days=0),
        ]
        self.assertEqual(ids(payload, keep=2), [1])

    def test_each_restore_key_keeps_its_own_survivor(self):
        payload = [
            entry(1, f"{RUST_KEY}-a", accessed_days=IDLE),
            entry(2, f"{RUST_KEY}-b", accessed_days=0),
            entry(3, f"{PLAIN_KEY}-a", accessed_days=IDLE),
            entry(4, f"{PLAIN_KEY}-b", accessed_days=0),
        ]
        self.assertEqual(sorted(ids(payload)), [1, 3])

    def test_another_ref_keeps_its_own_generations(self):
        # A run cannot restore a sibling branch's cache, so main's entry does
        # not supersede the branch's one.
        payload = [
            entry(1, f"{RUST_KEY}-a", accessed_days=IDLE, ref="refs/heads/feature"),
            entry(2, f"{RUST_KEY}-b", accessed_days=0, ref="refs/heads/main"),
        ]
        self.assertEqual(ids(payload), [])

    def test_a_ref_with_several_generations_is_pruned_within_that_ref(self):
        payload = [
            entry(1, f"{RUST_KEY}-a", accessed_days=IDLE, ref="refs/heads/feature"),
            entry(2, f"{RUST_KEY}-b", accessed_days=0, ref="refs/heads/feature"),
            entry(3, f"{RUST_KEY}-c", accessed_days=0, ref="refs/heads/main"),
        ]
        self.assertEqual(ids(payload), [1])

    def test_a_freshly_created_entry_is_never_reported(self):
        # A run can sit between one job's save and the next job's restore.
        payload = [
            entry(1, f"{RUST_KEY}-a", created=stamp(minutes=10), accessed=stamp(minutes=10)),
            entry(2, f"{RUST_KEY}-b", created=stamp(minutes=5), accessed=stamp(minutes=5)),
        ]
        # max_idle_days=0 takes the idle condition out of the way, so this
        # case tests the youth guard on its own.
        self.assertEqual(ids(payload, min_age_minutes=60, max_idle_days=0), [])
        self.assertEqual(ids(payload, min_age_minutes=1, max_idle_days=0), [1])

    def test_an_unreadable_access_time_is_never_reported(self):
        payload = [
            entry(1, f"{RUST_KEY}-a", accessed="not a time"),
            entry(2, f"{RUST_KEY}-b", accessed_days=0),
        ]
        self.assertEqual(ids(payload), [])

    def test_an_entry_without_an_access_time_does_not_take_a_kept_slot(self):
        payload = [
            entry(1, f"{RUST_KEY}-a", accessed=None, accessed_days=IDLE),
            entry(2, f"{RUST_KEY}-b", accessed_days=0),
            entry(3, f"{RUST_KEY}-c", accessed="not a time"),
        ]
        # The unreadable one is ignored on both sides: the two datable entries
        # still resolve to one survivor and one report.
        self.assertEqual(ids(payload), [1])

    def test_an_unreadable_creation_time_is_never_reported(self):
        payload = [
            entry(1, f"{RUST_KEY}-a", accessed_days=IDLE, created="not a time"),
            entry(2, f"{RUST_KEY}-b", accessed_days=0),
        ]
        self.assertEqual(ids(payload), [])

    def test_a_malformed_record_is_skipped(self):
        payload = [
            "not an entry",
            {"key": f"{RUST_KEY}-a"},
            {"id": "x", "key": f"{RUST_KEY}-b", "last_accessed_at": stamp(days=IDLE)},
            entry(1, f"{RUST_KEY}-c", accessed_days=IDLE),
            entry(2, f"{RUST_KEY}-d", accessed_days=0),
        ]
        self.assertEqual(ids(payload), [1])

    def test_both_response_shapes_are_accepted(self):
        rows = [entry(1, f"{RUST_KEY}-a", accessed_days=IDLE), entry(2, f"{RUST_KEY}-b", accessed_days=0)]
        self.assertEqual(ids(rows), [1])
        self.assertEqual(ids({"total_count": 2, "actions_caches": rows}), [1])
        self.assertEqual(ids({}), [])


class CommandTests(unittest.TestCase):
    def run_script(self, payload, *arguments):
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as handle:
            json.dump(payload, handle)
            path = handle.name
        try:
            return subprocess.run(
                [sys.executable, "-B", str(SCRIPT), "--caches", path, "--now", NOW.isoformat(), *arguments],
                capture_output=True,
                text=True,
                check=False,
            )
        finally:
            Path(path).unlink()

    def test_it_prints_the_id_size_and_key_of_each_entry(self):
        payload = [
            entry(11, f"{RUST_KEY}-a", accessed_days=IDLE, size=2048),
            entry(22, f"{RUST_KEY}-b", accessed_days=0),
        ]
        result = self.run_script(payload)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, f"11\t2048\t{RUST_KEY}-a\n")
        self.assertIn("1 superseded cache entries", result.stderr)

    def test_nothing_to_do_prints_no_lines_and_succeeds(self):
        result = self.run_script([entry(1, f"{RUST_KEY}-only")])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")
        self.assertIn("0 superseded cache entries", result.stderr)

    def test_keep_below_one_is_refused(self):
        result = self.run_script([], "--keep", "0")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("at least one generation", result.stderr)

    def test_an_unreadable_now_is_refused(self):
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as handle:
            json.dump([], handle)
            path = handle.name
        try:
            result = subprocess.run(
                [sys.executable, "-B", str(SCRIPT), "--caches", path, "--now", "yesterday"],
                capture_output=True,
                text=True,
                check=False,
            )
        finally:
            Path(path).unlink()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ISO 8601", result.stderr)


class WorkflowTests(unittest.TestCase):
    def test_the_workflow_deletes_only_with_the_write_permission_it_asks_for(self):
        workflow = WORKFLOW.read_text()
        self.assertIn("actions: write", workflow)
        self.assertIn("contents: read", workflow)
        # The prune must follow the run that saved the entries, and only on main.
        self.assertIn("workflow_run", workflow)
        self.assertIn("github.event.workflow_run.head_branch == 'main'", workflow)
        self.assertIn("scripts/prune-actions-caches.py", workflow)
        # A dispatch must be able to report without deleting.
        self.assertIn("dry_run", workflow)


if __name__ == "__main__":
    unittest.main(verbosity=2)
