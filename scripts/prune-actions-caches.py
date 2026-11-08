#!/usr/bin/env python3
r"""Choose which GitHub Actions cache entries a repository no longer needs.

Used by the Prune job of .github/workflows/cache-prune.yml.

Usage: prune-actions-caches.py --caches <file> --log <file> --ref <ref>
           [--min-age-minutes M] [--now <iso8601>]

--caches is the response of `GET /repos/{owner}/{repo}/actions/caches`, either
the object that holds an `actions_caches` list or the list itself. --log is the
log of the workflow run that saved the entries, and --ref is the ref that run
used. Prints one `<id>\t<size_in_bytes>\t<key>` line for each entry to delete
and a summary to standard error.

A cache key ends in a hash of the lockfile, and the key without that last
component is the restore key that a job falls back to. Each lockfile change adds
a generation beside the one before it, and only the generation whose hash a run
computes is reachable by its exact key.

Which generation that is cannot be worked out from the timestamps the API
reports. Two rules that look obvious are both wrong here:

- Newest by `created_at` is wrong, because the hash is not a sequence. A lockfile
  that returns to an earlier state keeps an older entry live. In this repository
  the live `linux-test` entry was created three minutes before the superseded one
  beside it, and for two of the heaviest keys the newest entry by creation was
  the superseded one, 1.28 GB that such a rule would keep for ever.
- Newest by `last_accessed_at` is wrong, because a pull request run restores the
  entries of its base branch. A branch still on the old lockfile takes an exact
  hit on the old generation, which makes that one the most recently read while
  main's own generation sits quiet. Waiting for an idle period does not fix it
  either: a docs-only merge skips the Rust jobs, so main's generation can stay
  quiet for days while a stale branch keeps reading the old one.

So this script does not infer. The log of the run names the key each job used, on
the line after `Cache Key:`, and those keys are the live set. An entry is
reported only when another generation of its own restore key is in that set.

A restore key the live set does not mention is left alone entirely. That is what
keeps a run which skipped a job from retiring that job's cache: a docs-only merge
names no Rust key, so no Rust generation is reported. If the log cannot be read,
or the action changes the wording this parser looks for, the live set comes out
empty and nothing is reported, which is the safe direction.

An entry created less than --min-age-minutes ago is never reported either,
because a run can sit between the save of one job and the restore of the next.

The two cases GitHub clears on its own are still left to it: it removes an entry
that no run has accessed for more than 7 days, and at the repository cache limit
it deletes the least recently accessed entries until the total fits.
"""

import argparse
import json
import re
import sys
from datetime import datetime, timedelta, timezone

# Long enough to cover the gap between one job saving an entry and a later job
# of the same run restoring it.
DEFAULT_MIN_AGE_MINUTES = 60
# Swatinem/rust-cache prints `Cache Key:` and then the key, indented, on the
# next line. `gh run view --log` puts the job, the step and a timestamp in front
# of both, so the marker is matched at the end of the line.
CACHE_KEY_MARKER = re.compile(r"Cache Key:\s*$")
# A key is the only token on its line: the action prints nothing else there.
CACHE_KEY_VALUE = re.compile(r"^(?:[^\t]*\t)*(?:\S+Z )?\s*(?P<key>[\w.-]+)\s*$")


def restore_key(key):
    """The key without its trailing lockfile hash, which is the restore key."""
    head, separator, _ = key.rpartition("-")
    return head if separator else key


def live_keys(log):
    """The cache key that each job of the run used, from the run's log."""
    keys = set()
    lines = log.splitlines()
    for index, line in enumerate(lines[:-1]):
        if not CACHE_KEY_MARKER.search(line):
            continue
        match = CACHE_KEY_VALUE.match(lines[index + 1])
        if match:
            keys.add(match.group("key"))
    return keys


def parse_time(value):
    if not isinstance(value, str):
        return None
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    return parsed if parsed.tzinfo else parsed.replace(tzinfo=timezone.utc)


def cache_entries(payload):
    """The cache list of either response shape."""
    if isinstance(payload, dict):
        payload = payload.get("actions_caches", [])
    if not isinstance(payload, list):
        return []
    entries = []
    for entry in payload:
        if not isinstance(entry, dict):
            continue
        key = entry.get("key")
        try:
            identifier = int(entry.get("id"))
        except (TypeError, ValueError):
            continue
        if not isinstance(key, str) or not key:
            continue
        entries.append(
            {
                "id": identifier,
                "key": key,
                "ref": entry.get("ref") or "",
                "size": entry.get("size_in_bytes") or 0,
                "created": parse_time(entry.get("created_at")),
            }
        )
    return entries


def superseded(payload, live, ref, min_age_minutes=DEFAULT_MIN_AGE_MINUTES, now=None):
    """The entries to delete, largest first."""
    now = now or datetime.now(timezone.utc)
    youngest_allowed = now - timedelta(minutes=min_age_minutes)
    live_restore_keys = {restore_key(key) for key in live}
    stale = [
        entry
        for entry in cache_entries(payload)
        # The live set comes from a run of this ref, so it says nothing about
        # the entries of another one.
        if entry["ref"] == ref
        and entry["key"] not in live
        and restore_key(entry["key"]) in live_restore_keys
        and entry["created"] is not None
        and entry["created"] <= youngest_allowed
    ]
    stale.sort(key=lambda entry: (entry["size"], entry["id"]), reverse=True)
    return stale


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--caches", required=True, help="the cache list response, as JSON")
    parser.add_argument("--log", required=True, help="the log of the run that saved the entries")
    parser.add_argument("--ref", required=True, help="the ref that run used")
    parser.add_argument(
        "--min-age-minutes",
        type=int,
        default=DEFAULT_MIN_AGE_MINUTES,
        help=f"never report an entry younger than this (default {DEFAULT_MIN_AGE_MINUTES})",
    )
    parser.add_argument("--now", help="the current time, for tests")
    arguments = parser.parse_args(argv)
    if arguments.min_age_minutes < 0:
        parser.error("--min-age-minutes cannot be negative")
    now = parse_time(arguments.now) if arguments.now else None
    if arguments.now and now is None:
        parser.error("--now must be an ISO 8601 time")

    with open(arguments.caches, encoding="utf-8") as handle:
        payload = json.load(handle)
    with open(arguments.log, encoding="utf-8", errors="replace") as handle:
        live = live_keys(handle.read())

    stale = superseded(payload, live, arguments.ref, arguments.min_age_minutes, now)
    for entry in stale:
        print(f"{entry['id']}\t{entry['size']}\t{entry['key']}")
    reclaimed = sum(entry["size"] for entry in stale)
    print(
        f"{len(live)} live cache keys, {len(stale)} superseded entries, "
        f"{reclaimed / 2**30:.2f} GB",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
