#!/usr/bin/env python3
"""The cache pruner reports only the generations the run's own log retires."""

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
MAIN = "refs/heads/main"
# The two key shapes this repository produces. Both end in the lockfile hash.
RUST_KEY = "v0-rust-linux-test-rust-test-Linux-x64-c1a0964a"
PLAIN_KEY = "device-device-Linux-x64-c1a0964a"

# Copied from a run of this repository. `gh run view --log` puts the job name,
# the step name and a timestamp in front of each line of the action's output.
REAL_LOG = (
    "Browser engine\tRun Swatinem/rust-cache@v2\t2026-11-08T12:14:23.8931968Z     "
    "browser-engine-browser-engine-Linux-x64-c1a0964a\n"
    "Browser engine\tRun Swatinem/rust-cache@v2\t2026-11-08T12:14:23.8932679Z Cache Key:\n"
    "Browser engine\tRun Swatinem/rust-cache@v2\t2026-11-08T12:14:23.8933365Z     "
    "browser-engine-browser-engine-Linux-x64-c1a0964a-af2d2e7c\n"
    "Browser engine\tRun Swatinem/rust-cache@v2\t2026-11-08T12:14:23.8934146Z .. Prefix:\n"
    "Browser engine\tRun Swatinem/rust-cache@v2\t2026-11-08T12:14:23.8934653Z   - "
    "browser-engine-browser-engine-Linux-x64\n"
    "Clippy\tRun Swatinem/rust-cache@v2\t2026-11-08T12:14:21.3962217Z Cache Key:\n"
    "Clippy\tRun Swatinem/rust-cache@v2\t2026-11-08T12:14:21.3962628Z     "
    "v0-rust-clippy-Linux-x64-c1a0964a-af2d2e7c\n"
)


def stamp(days=0, minutes=0):
    return (NOW - timedelta(days=days, minutes=minutes)).isoformat().replace("+00:00", "Z")


def entry(identifier, key, *, created_days=30, ref=MAIN, size=1024, created=None):
    return {
        "id": identifier,
        "key": key,
        "ref": ref,
        "size_in_bytes": size,
        "created_at": stamp(days=created_days) if created is None else created,
    }


def ids(payload, live, ref=MAIN, **kwargs):
    kwargs.setdefault("now", NOW)
    return [item["id"] for item in prune.superseded(payload, set(live), ref, **kwargs)]


class RestoreKeyTests(unittest.TestCase):
    def test_the_trailing_lock_hash_is_not_part_of_the_restore_key(self):
        self.assertEqual(prune.restore_key(f"{RUST_KEY}-af2d2e7c"), RUST_KEY)
        self.assertEqual(prune.restore_key(f"{PLAIN_KEY}-af2d2e7c"), PLAIN_KEY)

    def test_a_key_without_a_separator_is_its_own_restore_key(self):
        self.assertEqual(prune.restore_key("solitary"), "solitary")

    def test_a_longer_key_is_not_grouped_with_the_key_it_starts_with(self):
        # `linux-test` is a prefix of `linux-test-libs`, and the two are separate
        # caches. Only the last component may be dropped.
        self.assertNotEqual(
            prune.restore_key("v0-rust-linux-test-rust-test-Linux-x64-c1a0964a-af2d2e7c"),
            prune.restore_key("v0-rust-linux-test-libs-rust-test-Linux-x64-c1a0964a-af2d2e7c"),
        )


class LiveKeyTests(unittest.TestCase):
    def test_it_reads_the_keys_out_of_a_real_run_log(self):
        self.assertEqual(
            prune.live_keys(REAL_LOG),
            {
                "browser-engine-browser-engine-Linux-x64-c1a0964a-af2d2e7c",
                "v0-rust-clippy-Linux-x64-c1a0964a-af2d2e7c",
            },
        )

    def test_it_reads_a_log_without_the_job_and_step_columns(self):
        log = f"Cache Key:\n    {RUST_KEY}-af2d2e7c\n"
        self.assertEqual(prune.live_keys(log), {f"{RUST_KEY}-af2d2e7c"})

    def test_the_restore_key_above_the_marker_is_not_taken_for_a_live_key(self):
        # The action prints `Restore Key:` and its value first. Only the line
        # after `Cache Key:` is the key the job used.
        self.assertNotIn(RUST_KEY, prune.live_keys(REAL_LOG))

    def test_an_unreadable_log_yields_no_live_keys(self):
        self.assertEqual(prune.live_keys(""), set())
        self.assertEqual(prune.live_keys("Cache Key:\n"), set())
        self.assertEqual(prune.live_keys("some other wording:\n    a-key\n"), set())
        self.assertEqual(prune.live_keys("Cache Key:\n    two tokens here\n"), set())


class SupersededTests(unittest.TestCase):
    def test_the_other_generations_of_a_live_key_are_reported(self):
        payload = [
            entry(1, f"{RUST_KEY}-old"),
            entry(2, f"{RUST_KEY}-live"),
            entry(3, f"{RUST_KEY}-older"),
        ]
        self.assertEqual(sorted(ids(payload, [f"{RUST_KEY}-live"])), [1, 3])

    def test_the_live_generation_is_never_reported(self):
        payload = [entry(1, f"{RUST_KEY}-live")]
        self.assertEqual(ids(payload, [f"{RUST_KEY}-live"]), [])

    def test_a_restore_key_the_run_did_not_name_is_left_alone(self):
        # A docs-only merge skips the Rust jobs, so the run names no Rust key.
        # Nothing of that cache may be retired by such a run, whatever the
        # timestamps say.
        payload = [
            entry(1, f"{RUST_KEY}-old"),
            entry(2, f"{RUST_KEY}-new", created_days=1),
            entry(3, f"{PLAIN_KEY}-old"),
        ]
        self.assertEqual(ids(payload, [f"{PLAIN_KEY}-live"]), [3])

    def test_a_branch_reading_the_old_generation_cannot_retire_the_live_one(self):
        # The live set names main's generation, so the generation a stale branch
        # keeps reading is the one reported, never the other way round.
        payload = [
            entry(1, f"{RUST_KEY}-old", created_days=9),
            entry(2, f"{RUST_KEY}-live", created_days=8),
        ]
        self.assertEqual(ids(payload, [f"{RUST_KEY}-live"]), [1])

    def test_creation_order_does_not_protect_a_superseded_generation(self):
        # Measured here: for `linux-test` and `macos-test-libs` the newest entry
        # by creation was the superseded one, 1.28 GB that a rule anchored on
        # creation would keep for ever.
        live = entry(1, f"{RUST_KEY}-af2d2e7c", created=stamp(days=2, minutes=3))
        newest_created = entry(2, f"{RUST_KEY}-83b9b9f8", created=stamp(days=2))
        self.assertEqual(ids([live, newest_created], [f"{RUST_KEY}-af2d2e7c"]), [2])

    def test_an_entry_of_another_ref_is_left_alone(self):
        # A run cannot restore a sibling branch's cache, so a run of main says
        # nothing about what that branch still needs.
        payload = [
            entry(1, f"{RUST_KEY}-old", ref="refs/heads/feature"),
            entry(2, f"{RUST_KEY}-old"),
        ]
        self.assertEqual(ids(payload, [f"{RUST_KEY}-live"]), [2])

    def test_a_freshly_created_entry_is_never_reported(self):
        # A run can sit between one job's save and the next job's restore.
        payload = [entry(1, f"{RUST_KEY}-old", created=stamp(minutes=10))]
        live = [f"{RUST_KEY}-live"]
        self.assertEqual(ids(payload, live, min_age_minutes=60), [])
        self.assertEqual(ids(payload, live, min_age_minutes=1), [1])

    def test_an_unreadable_creation_time_is_never_reported(self):
        payload = [entry(1, f"{RUST_KEY}-old", created="not a time")]
        self.assertEqual(ids(payload, [f"{RUST_KEY}-live"]), [])

    def test_an_empty_live_set_reports_nothing(self):
        payload = [entry(1, f"{RUST_KEY}-a"), entry(2, f"{RUST_KEY}-b")]
        self.assertEqual(ids(payload, []), [])

    def test_a_malformed_record_is_skipped(self):
        payload = [
            "not an entry",
            {"key": f"{RUST_KEY}-x"},
            {"id": "x", "key": f"{RUST_KEY}-y", "created_at": stamp(days=3), "ref": MAIN},
            entry(1, f"{RUST_KEY}-old"),
        ]
        self.assertEqual(ids(payload, [f"{RUST_KEY}-live"]), [1])

    def test_both_response_shapes_are_accepted(self):
        rows = [entry(1, f"{RUST_KEY}-old")]
        live = [f"{RUST_KEY}-live"]
        self.assertEqual(ids(rows, live), [1])
        self.assertEqual(ids({"total_count": 1, "actions_caches": rows}, live), [1])
        self.assertEqual(ids({}, live), [])

    def test_the_largest_entry_is_reported_first(self):
        payload = [
            entry(1, f"{RUST_KEY}-a", size=10),
            entry(2, f"{RUST_KEY}-b", size=3000),
            entry(3, f"{RUST_KEY}-c", size=200),
        ]
        self.assertEqual(ids(payload, [f"{RUST_KEY}-live"]), [2, 3, 1])


class CommandTests(unittest.TestCase):
    def run_script(self, payload, log, *arguments, ref=MAIN):
        with tempfile.TemporaryDirectory() as folder:
            caches = Path(folder) / "caches.json"
            caches.write_text(json.dumps(payload))
            log_file = Path(folder) / "run.log"
            log_file.write_text(log)
            return subprocess.run(
                [
                    sys.executable, "-B", str(SCRIPT),
                    "--caches", str(caches),
                    "--log", str(log_file),
                    "--ref", ref,
                    "--now", NOW.isoformat(),
                    *arguments,
                ],
                capture_output=True,
                text=True,
                check=False,
            )

    def test_it_prints_the_id_size_and_key_of_each_entry(self):
        key = "v0-rust-clippy-Linux-x64-c1a0964a"
        payload = [entry(11, f"{key}-83b9b9f8", size=2048), entry(22, f"{key}-af2d2e7c")]
        result = self.run_script(payload, REAL_LOG)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, f"11\t2048\t{key}-83b9b9f8\n")
        self.assertIn("2 live cache keys, 1 superseded entries", result.stderr)

    def test_nothing_to_do_prints_no_lines_and_succeeds(self):
        result = self.run_script([entry(1, f"{RUST_KEY}-only")], REAL_LOG)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")
        self.assertIn("0 superseded entries", result.stderr)

    def test_an_unreadable_now_is_refused(self):
        result = self.run_script([], REAL_LOG, "--now", "yesterday")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ISO 8601", result.stderr)

    def test_a_negative_min_age_is_refused(self):
        result = self.run_script([], REAL_LOG, "--min-age-minutes", "-1")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("cannot be negative", result.stderr)


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

    def test_the_workflow_feeds_the_log_and_ref_of_the_triggering_run(self):
        workflow = WORKFLOW.read_text()
        self.assertIn("--log", workflow)
        self.assertIn("--ref", workflow)
        # The decision needs that run's own log, not a fresh one.
        self.assertIn("github.event.workflow_run.id", workflow)


if __name__ == "__main__":
    unittest.main(verbosity=2)
