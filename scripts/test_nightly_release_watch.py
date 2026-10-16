#!/usr/bin/env python3
"""Tests for scripts/nightly-release-watch.py and its Watch job."""

import importlib.util
import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "nightly-release-watch.py"
WORKFLOW = ROOT / ".github" / "workflows" / "macos-release-builds.yml"
RUN_URL = "https://github.com/owner/repo/actions/runs/30"
SHA = "0123456789abcdef0123456789abcdef01234567"

spec = importlib.util.spec_from_file_location("nightly_release_watch", SCRIPT)
watch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(watch)


def needs(**results):
    return {name.replace("_", "-"): {"result": result, "outputs": {}}
            for name, result in results.items()}


def run(run_id, conclusion, event="schedule", status="completed"):
    return {"id": run_id, "event": event, "status": status, "conclusion": conclusion}


GREEN = needs(changes="success", simulate_failure="skipped", rust_artifacts="success")
QUIET = needs(changes="success", simulate_failure="skipped", rust_artifacts="skipped")
RED = needs(changes="success", simulate_failure="skipped", rust_artifacts="failure")


class DecisionTests(unittest.TestCase):
    def test_every_failed_run_posts_a_failure(self):
        for previous in [[], [run(20, "success")], [run(20, "failure")]]:
            with self.subTest(previous=previous):
                self.assertEqual(watch.decide(RED, previous, 30), "failure")
        simulated = needs(changes="skipped", simulate_failure="failure", rust_artifacts="skipped")
        self.assertEqual(watch.decide(simulated, [], 30), "failure")
        detection = needs(changes="failure", simulate_failure="skipped", rust_artifacts="success")
        self.assertEqual(watch.decide(detection, [run(20, "success")], 30), "failure")

    def test_success_after_a_failed_run_posts_a_recovery(self):
        for conclusion in ["failure", "timed_out", "startup_failure"]:
            for current in [GREEN, QUIET]:
                with self.subTest(conclusion=conclusion):
                    runs = [run(20, conclusion), run(10, "success")]
                    self.assertEqual(watch.decide(current, runs, 30), "recovery")
        dispatched = [run(20, "failure", event="workflow_dispatch")]
        self.assertEqual(watch.decide(GREEN, dispatched, 30), "recovery")

    def test_routine_success_and_unknown_history_stay_silent(self):
        for runs in [[], [run(20, "success"), run(10, "failure")],
                     [run(20, "failure", event="push")], [{"bad": "entry"}]]:
            with self.subTest(runs=runs):
                self.assertEqual(watch.decide(GREEN, runs, 30), "none")

    def test_the_search_skips_this_run_and_runs_without_a_verdict(self):
        runs = [run(30, "failure"), run(29, "failure", status="in_progress"),
                run(28, "cancelled"), run(27, "skipped"), run(26, "failure"),
                run(25, "success")]
        self.assertEqual(watch.decide(GREEN, runs, 30), "recovery")
        self.assertEqual(watch.decide(GREEN, runs[:1] + runs[2:4] + runs[5:], "30"), "none")

    def test_runs_are_ordered_by_id_not_by_list_position(self):
        runs = [run(10, "failure"), run(20, "success"), {"id": "x", "event": "schedule"}]
        self.assertEqual(watch.decide(GREEN, runs, 30), "none")
        self.assertEqual(watch.decide(GREEN, list(reversed(runs)), 30), "none")

    def test_a_re_run_compares_with_its_previous_attempt(self):
        older_green = [run(20, "success")]
        self.assertEqual(watch.decide(GREEN, older_green, 30, 2, "failure"), "recovery")
        self.assertEqual(watch.decide(GREEN, [run(20, "failure")], 30, 2, "success"), "none")
        # An attempt without a verdict falls back to the older runs.
        for unknown in [None, "", "cancelled"]:
            with self.subTest(unknown=unknown):
                self.assertEqual(watch.decide(GREEN, older_green, 30, 2, unknown), "none")
                self.assertEqual(watch.decide(GREEN, [run(20, "failure")], 30, 2, unknown),
                                 "recovery")
        self.assertEqual(watch.decide(RED, older_green, 30, 2, "failure"), "failure")

    def test_a_green_run_stays_silent_once_a_newer_run_finished(self):
        for newer in ["failure", "success"]:
            with self.subTest(newer=newer):
                runs = [run(40, newer), run(20, "failure")]
                self.assertEqual(watch.decide(GREEN, runs, 30, 2, "failure"), "none")
                self.assertEqual(watch.decide(GREEN, runs, 30), "none")
                self.assertEqual(watch.decide(RED, runs, 30, 2, "failure"), "failure")
        in_progress = [run(40, None, status="in_progress"), run(20, "failure")]
        self.assertEqual(watch.decide(GREEN, in_progress, 30), "recovery")


class CommentTests(unittest.TestCase):
    def test_failure_names_the_failed_jobs_with_a_marker(self):
        jobs = [{"name": "Artifacts / macOS x64", "conclusion": "failure"},
                {"name": "Artifacts / macOS arm64", "conclusion": "success"},
                {"name": "Artifacts / Select platforms", "conclusion": "timed_out"},
                {"name": "Watch", "conclusion": None}]
        names = watch.failed_job_names(jobs, RED)
        self.assertEqual(names, ["Artifacts / macOS x64", "Artifacts / Select platforms"])
        text = watch.body("failure", run_url=RUN_URL, sha=SHA, failed_jobs=names)
        self.assertEqual(text.splitlines(), [
            "<!-- nightly-macos-release -->",
            "The macOS release builds failed on `main` at 0123456789ab.",
            f"Run: {RUN_URL}",
            "Failed jobs: Artifacts / macOS x64, Artifacts / Select platforms",
        ])

    def test_failed_jobs_fall_back_to_the_needed_jobs(self):
        self.assertEqual(watch.failed_job_names([], RED), ["rust-artifacts"])

    def test_recovery_is_short(self):
        text = watch.body("recovery", run_url=RUN_URL, sha=SHA, failed_jobs=[])
        self.assertEqual(text.splitlines(), [
            "<!-- nightly-macos-release -->",
            "The macOS release builds recovered on `main` at 0123456789ab.",
            f"Run: {RUN_URL}",
        ])


class CommandTests(unittest.TestCase):
    def setUp(self):
        self.dir = Path(tempfile.mkdtemp(prefix="nightly-watch-"))
        self.addCleanup(subprocess.run, ["rm", "-rf", str(self.dir)], check=False)
        self.body = self.dir / "comment.md"

    def call(self, needs_json, runs=None, jobs=None, extra=()):
        paths = {}
        for name, data in [("runs", runs), ("jobs", jobs)]:
            paths[name] = self.dir / f"{name}.json"
            if data is not None:
                paths[name].write_text(data if isinstance(data, str) else json.dumps(data))
        return subprocess.run(
            [sys.executable, "-B", str(SCRIPT), "--needs", needs_json,
             "--runs", str(paths["runs"]), "--jobs", str(paths["jobs"]), "--run-id", "30",
             "--run-attempt", "1", "--run-url", RUN_URL, "--sha", SHA,
             "--body-out", str(self.body), *extra],
            capture_output=True, text=True)

    def test_prints_the_action_and_writes_a_body_only_to_post(self):
        result = self.call(json.dumps(GREEN), runs={"workflow_runs": [run(20, "success")]})
        self.assertEqual((result.returncode, result.stdout), (0, "action=none\n"))
        self.assertFalse(self.body.exists())
        result = self.call(json.dumps(GREEN), runs={"workflow_runs": [run(20, "failure")]})
        self.assertEqual(result.stdout, "action=recovery\n")
        self.assertIn("recovered", self.body.read_text())

    def test_an_unreadable_history_still_reports_a_failure(self):
        for runs in [None, "{", {"workflow_runs": "x"}, []]:
            with self.subTest(runs=runs):
                result = self.call(json.dumps(RED), runs=runs, jobs="not json")
                self.assertEqual(result.stdout, "action=failure\n")
                self.assertIn("Failed jobs: rust-artifacts", self.body.read_text())
                self.assertEqual(self.call(json.dumps(GREEN), runs=runs).stdout, "action=none\n")

    def test_bad_needs_fail_without_an_action(self):
        for value in ["", "{", "[]"]:
            with self.subTest(value=value):
                result = self.call(value)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.stdout, "")

    def test_a_re_run_reads_its_previous_attempt(self):
        runs = {"workflow_runs": [run(20, "success")]}
        result = self.call(json.dumps(GREEN), runs=runs,
                           extra=["--run-attempt", "2", "--previous-attempt", "failure"])
        self.assertEqual(result.stdout, "action=recovery\n")
        result = self.call(json.dumps(GREEN), runs=runs, extra=["--run-attempt", "x"])
        self.assertEqual((result.returncode, result.stdout), (2, ""))


class WorkflowTests(unittest.TestCase):
    def setUp(self):
        self.workflow = WORKFLOW.read_text()
        body = self.workflow.split("\njobs:\n", 1)[1]
        self.jobs = dict(re.findall(r"^  ([\w-]+):\n(.*?)(?=^  [\w-]+:|\Z)", body, re.M | re.S))

    def test_the_watch_job_waits_for_every_other_job(self):
        others = sorted(name for name in self.jobs if name != "watch")
        needed = re.search(r"(?m)^    needs: \[(.*)\]$", self.jobs["watch"]).group(1)
        self.assertEqual(sorted(name.strip() for name in needed.split(",")), others)
        self.assertIn("    if: ${{ !cancelled() && github.ref == 'refs/heads/main' && "
                      "github.repository == 'peters/horizon' }}\n", self.jobs["watch"])

    def test_only_the_watch_job_can_write_issues(self):
        self.assertEqual(self.workflow.count("issues: write"), 1)
        self.assertIn("    permissions:\n      contents: read\n      actions: read\n"
                      "      issues: write\n", self.jobs["watch"])
        top = re.search(r"(?ms)^permissions:\n(.*?)^\S", self.workflow).group(1)
        self.assertEqual(top, "  contents: read\n  actions: read\n\n")
        self.assertIn("      WATCH_ISSUE: 1534\n", self.jobs["watch"])
        self.assertIn("        if: steps.decide.outputs.action != 'none'\n", self.jobs["watch"])
        self.assertIn('NEEDS: ${{ toJSON(needs) }}', self.jobs["watch"])

    def test_the_history_query_and_the_post_are_scoped(self):
        job = self.jobs["watch"]
        self.assertIn('/runs?branch=main&status=completed&per_page=30" > "$RUNNER_TEMP/runs.json"',
                      job)
        self.assertIn('/attempts/$GITHUB_RUN_ATTEMPT/jobs?per_page=100" > "$RUNNER_TEMP/jobs.json"',
                      job)
        self.assertIn('/attempts/$((GITHUB_RUN_ATTEMPT - 1))" --jq .conclusion', job)
        for option in ['--run-id "$GITHUB_RUN_ID"', '--run-attempt "$GITHUB_RUN_ATTEMPT"',
                       '--previous-attempt "$previous_attempt"', '--sha "$GITHUB_SHA"',
                       '--body-out "$RUNNER_TEMP/comment.md" >> "$GITHUB_OUTPUT"']:
            self.assertIn(option, job)
        self.assertIn('gh api "repos/$GITHUB_REPOSITORY/issues/$WATCH_ISSUE/comments" '
                      '-F "body=@$RUNNER_TEMP/comment.md"', job)

    def gate(self, runs):
        """Runs the skip gate's jq filter on one page of run history."""
        jq_filter = re.search(r"--jq '(.*?)'\)", self.jobs["changes"]).group(1)
        result = subprocess.run(["jq", "-r", jq_filter], input=json.dumps({"workflow_runs": runs}),
                                capture_output=True, text=True, check=True)
        return result.stdout.strip()

    def test_the_skip_gate_reads_the_newest_finished_run_of_the_commit(self):
        self.assertIn("/runs?branch=main&status=completed&head_sha=$GITHUB_SHA&per_page=100",
                      self.jobs["changes"])
        self.assertIn("            success) build=false;", self.jobs["changes"])
        cases = [
            ([], "none"),
            ([run(20, "success"), run(10, "failure")], "success"),
            ([run(10, "success"), run(20, "failure")], "failure"),
            ([run(20, "success"), run(30, "cancelled"), run(40, "skipped"),
              run(50, None)], "success"),
            ([run(20, "failure"), run(30, "timed_out")], "timed_out"),
            ([run(30, "cancelled")], "none"),
        ]
        for runs, expected in cases:
            with self.subTest(runs=runs):
                self.assertEqual(self.gate(runs), expected)

    def test_a_simulated_failure_fails_without_a_build(self):
        self.assertRegex(self.workflow, r"(?m)^      simulate_failure:\n(?:        .*\n)*"
                                        r"        type: boolean\n        default: false\n")
        self.assertIn("    if: inputs.simulate_failure\n", self.jobs["simulate-failure"])
        self.assertIn("&& !inputs.simulate_failure }}", self.jobs["rust-artifacts"])

    def test_repo_checks_run_these_tests(self):
        ci = (ROOT / ".github" / "workflows" / "ci.yml").read_text()
        self.assertIn("python3 -B scripts/test_nightly_release_watch.py -v", ci)


if __name__ == "__main__":
    unittest.main()
