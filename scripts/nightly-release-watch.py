#!/usr/bin/env python3
"""Decide what the macOS release builds watcher posts for one run.

Used by the Watch job of .github/workflows/macos-release-builds.yml.

Usage: nightly-release-watch.py --needs <json> --runs <file> --jobs <file>
           --run-id <id> --run-attempt <n> [--previous-attempt <conclusion>]
           --run-url <url> --sha <sha> --body-out <file>

--needs is the `toJSON(needs)` of the Watch job. --runs is the
`workflow_runs` list of this workflow, newest first, and --jobs is the `jobs`
list of the current run attempt. An unreadable --runs file makes the history
unknown, and an unreadable --jobs file names the failed jobs from --needs. Prints `action=failure`, `action=recovery` or `action=none` in the
form a workflow step appends to $GITHUB_OUTPUT, and writes the comment to
--body-out unless the action is `none`.

- failure: a job this run needs failed. Every failed run posts.
- recovery: no needed job failed, and the run before this one failed. For a
  re-run that is the previous attempt of this run (--previous-attempt) when
  that attempt finished; otherwise it is the newest older finished scheduled
  or dispatched run.
- none: everything else, routine success included. A green run also stays
  silent when a newer run has finished, because that run already reported
  the current state. An unknown history never posts a recovery, not even
  after a failed previous attempt: a newer run can be red.

A run that failed only because the Watch job could not post still counts as
a failed run, so the next green run posts a recovery for it.
"""

import argparse
import json
import sys

MARKER = "<!-- nightly-macos-release -->"
WATCHED_EVENTS = {"schedule", "workflow_dispatch"}
# A cancelled or skipped run says nothing about the build, so the search
# looks past it.
FINISHED = {"success", "failure", "timed_out", "startup_failure"}


def failed_needs(needs):
    return sorted(name for name, need in needs.items() if need.get("result") == "failure")


def finished_runs(runs):
    """(id, conclusion) of each finished scheduled or dispatched run, newest first."""
    finished = []
    for run in runs:
        try:
            run_id = int(run.get("id"))
        except (TypeError, ValueError):
            continue
        if (run.get("event") in WATCHED_EVENTS and run.get("status") == "completed"
                and run.get("conclusion") in FINISHED):
            finished.append((run_id, run["conclusion"]))
    return sorted(finished, reverse=True)


def previous_conclusion(runs, run_id, run_attempt=1, previous_attempt=None):
    """The conclusion of the run before this one, or None when it is unknown.

    `runs` is None when the history could not be read.
    """
    if runs is None:
        return None
    finished = finished_runs(runs)
    if any(other > int(run_id) for other, _ in finished):
        return None
    if int(run_attempt) > 1 and previous_attempt in FINISHED:
        return previous_attempt
    return next((conclusion for other, conclusion in finished if other < int(run_id)), None)


def decide(needs, runs, run_id, run_attempt=1, previous_attempt=None):
    if failed_needs(needs):
        return "failure"
    previous = previous_conclusion(runs, run_id, run_attempt, previous_attempt)
    if previous is not None and previous != "success":
        return "recovery"
    return "none"


def failed_job_names(jobs, needs):
    names = [job["name"] for job in jobs
             if job.get("conclusion") in {"failure", "timed_out"} and job.get("name")]
    return names or failed_needs(needs)


def body(action, *, run_url, sha, failed_jobs):
    commit = sha[:12]
    if action == "failure":
        lines = [MARKER, f"The macOS release builds failed on `main` at {commit}.",
                 f"Run: {run_url}", f"Failed jobs: {', '.join(failed_jobs) or 'unknown'}"]
    else:
        lines = [MARKER, f"The macOS release builds recovered on `main` at {commit}.",
                 f"Run: {run_url}"]
    return "\n".join(lines) + "\n"


def read_list(path, key):
    """The `key` list of a JSON file, or None when it is unreadable."""
    try:
        with open(path, encoding="utf-8") as handle:
            data = json.load(handle)
    except (OSError, ValueError):
        return None
    items = data.get(key) if isinstance(data, dict) else data
    return [item for item in items if isinstance(item, dict)] if isinstance(items, list) else None


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    for option in ["--needs", "--runs", "--jobs", "--run-id", "--run-attempt", "--run-url",
                   "--sha", "--body-out"]:
        parser.add_argument(option, required=True)
    parser.add_argument("--previous-attempt", default="")
    args = parser.parse_args(argv)
    try:
        needs = json.loads(args.needs)
    except ValueError as error:
        parser.error(f"--needs is not JSON: {error}")
    if not isinstance(needs, dict):
        parser.error("--needs must be a JSON object")
    if not (args.run_id.isdigit() and args.run_attempt.isdigit()):
        parser.error("--run-id and --run-attempt must be numbers")
    action = decide(needs, read_list(args.runs, "workflow_runs"), args.run_id,
                    args.run_attempt, args.previous_attempt)
    if action != "none":
        failed_jobs = failed_job_names(read_list(args.jobs, "jobs") or [], needs)
        with open(args.body_out, "w", encoding="utf-8") as handle:
            handle.write(body(action, run_url=args.run_url, sha=args.sha, failed_jobs=failed_jobs))
    print(f"action={action}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
