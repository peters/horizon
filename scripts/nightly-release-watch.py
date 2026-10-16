#!/usr/bin/env python3
"""Decide what the macOS release builds watcher posts for one run.

Used by the Watch job of .github/workflows/macos-release-builds.yml.

Usage: nightly-release-watch.py --needs <json> --runs <file> --jobs <file>
           --run-id <id> --run-url <url> --sha <sha> --body-out <file>

--needs is the `toJSON(needs)` of the Watch job. --runs is the
`workflow_runs` list of this workflow, newest first, and --jobs is the `jobs`
list of the current run attempt; an empty or unreadable file counts as
unknown. Prints `action=failure`, `action=recovery` or `action=none` in the
form a workflow step appends to $GITHUB_OUTPUT, and writes the comment to
--body-out unless the action is `none`.

- failure: a job this run needs failed. Every failed run posts.
- recovery: no needed job failed, and the newest earlier finished scheduled
  or dispatched run failed.
- none: everything else, routine success included. An unknown run history
  never posts a recovery.
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


def previous_conclusion(runs, run_id):
    for run in runs:
        if str(run.get("id")) == str(run_id) or run.get("event") not in WATCHED_EVENTS:
            continue
        if run.get("status") == "completed" and run.get("conclusion") in FINISHED:
            return run["conclusion"]
    return None


def decide(needs, runs, run_id):
    if failed_needs(needs):
        return "failure"
    previous = previous_conclusion(runs, run_id)
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
    """The `key` list of a JSON file, or an empty list when it is unreadable."""
    try:
        with open(path, encoding="utf-8") as handle:
            data = json.load(handle)
    except (OSError, ValueError):
        return []
    items = data.get(key) if isinstance(data, dict) else data
    return [item for item in items if isinstance(item, dict)] if isinstance(items, list) else []


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    for option in ["--needs", "--runs", "--jobs", "--run-id", "--run-url", "--sha", "--body-out"]:
        parser.add_argument(option, required=True)
    args = parser.parse_args(argv)
    try:
        needs = json.loads(args.needs)
    except ValueError as error:
        parser.error(f"--needs is not JSON: {error}")
    if not isinstance(needs, dict):
        parser.error("--needs must be a JSON object")
    action = decide(needs, read_list(args.runs, "workflow_runs"), args.run_id)
    if action != "none":
        failed_jobs = failed_job_names(read_list(args.jobs, "jobs"), needs)
        with open(args.body_out, "w", encoding="utf-8") as handle:
            handle.write(body(action, run_url=args.run_url, sha=args.sha, failed_jobs=failed_jobs))
    print(f"action={action}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
