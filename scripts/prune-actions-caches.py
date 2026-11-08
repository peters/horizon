#!/usr/bin/env python3
r"""Choose which GitHub Actions cache entries a repository no longer needs.

Used by the Prune job of .github/workflows/cache-prune.yml.

Usage: prune-actions-caches.py --caches <file> [--keep N]
           [--min-age-minutes M] [--now <iso8601>]

--caches is the response of `GET /repos/{owner}/{repo}/actions/caches`, either
the object that holds an `actions_caches` list or the list itself. Prints one
`<id>\t<size_in_bytes>\t<key>` line for each entry to delete and a summary to
standard error.

A cache key from actions/cache ends in a hash of the lockfile, and the key
without that last component is the restore key that a job falls back to. The
entries that share a restore key are generations of one cache, and a run reaches
only the generation whose hash matches its own lockfile. This script reports the
rest, keeping `--keep` per restore key and per ref.

Neither timestamp alone identifies the generation that main uses.

`created_at` does not: the hash is not a sequence, so a lockfile that returns to
an earlier state keeps an older entry live. In this repository the live
`linux-test` entry was created three minutes before the superseded one beside
it, and the newest entry by creation was the superseded one for two of the
heaviest keys, 1.28 GB that a rule anchored on creation would keep for ever.

`last_accessed_at` does not either: a pull request run restores the entries of
its base branch, so a branch still on the old lockfile can take an exact hit on
the old generation after main has already saved the new one. The old generation
then becomes the most recently accessed, and a rule that keeps only that one
would delete the generation main itself restores.

So an entry has to clear both tests to be reported: it is outside the `--keep`
most recently accessed of its group, and nothing has read it for
`--max-idle-days`. A generation that any branch still exact-matches keeps being
read, which holds it above the idle threshold whatever its place in the order.
Measured on this repository, the separation is wide: every live generation had
been read the same day, and all ten superseded ones had been idle 2.3 days.

A ref is part of the group because cache scoping is directional: a run restores
entries of its own branch, the default branch and, for a pull request, the base
branch, but never a sibling branch. Grouping per ref keeps a prune that one
branch starts from reaching generations that only another branch can use.

Two cases are deliberately left alone, because GitHub clears them without help:
it removes an entry that no run has accessed for more than 7 days, and at the
repository cache limit it deletes the least recently accessed entries until the
total fits. So a group with one member reports nothing, even when a toolchain or
lockfile change has made its restore key unreachable.

A lockfile change therefore retires the generation before it only once every
branch has stopped reading it. A branch that comes back to an old lockfile after
that finds no exact match and falls back to the restore key, which serves the
newest entry, so it pays a partial build once rather than keeping a second copy
of every cache alive for ever.

An entry is also never reported when it was created less than --min-age-minutes
ago, because a run can sit between the save of one job and the restore of the
next, or when its timestamps cannot be read, because an entry that cannot be
ordered cannot be shown to be superseded.
"""

import argparse
import json
import sys
from datetime import datetime, timedelta, timezone

# Long enough to cover the gap between one job saving an entry and a later job
# of the same run restoring it.
DEFAULT_MIN_AGE_MINUTES = 60
DEFAULT_KEEP = 1
# A generation that any branch still exact-matches is read by those runs. Two
# days is above every live entry measured here, which had all been read the same
# day, and below the 2.3 days that the superseded ones had been idle.
DEFAULT_MAX_IDLE_DAYS = 2


def restore_key(key):
    """The key without its trailing lockfile hash, which is the restore key."""
    head, separator, _ = key.rpartition("-")
    return head if separator else key


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
                "accessed": parse_time(entry.get("last_accessed_at")),
            }
        )
    return entries


def superseded(
    payload,
    keep=DEFAULT_KEEP,
    min_age_minutes=DEFAULT_MIN_AGE_MINUTES,
    max_idle_days=DEFAULT_MAX_IDLE_DAYS,
    now=None,
):
    """The entries to delete, most recently accessed first."""
    now = now or datetime.now(timezone.utc)
    youngest_allowed = now - timedelta(minutes=min_age_minutes)
    idle_before = now - timedelta(days=max_idle_days)
    groups = {}
    for entry in cache_entries(payload):
        # An entry without a readable access time cannot be placed in the
        # generation order, so it neither survives a slot nor gets reported.
        if entry["accessed"] is None:
            continue
        groups.setdefault((entry["ref"], restore_key(entry["key"])), []).append(entry)
    stale = []
    for group in groups.values():
        group.sort(key=lambda entry: (entry["accessed"], entry["id"]), reverse=True)
        # Two keepers besides the order: an entry that something still reads,
        # and a fresh one, because a run can sit between the save of one job and
        # the restore of the next.
        stale.extend(
            entry
            for entry in group[keep:]
            if entry["accessed"] <= idle_before
            and entry["created"] is not None
            and entry["created"] <= youngest_allowed
        )
    stale.sort(key=lambda entry: (entry["accessed"], entry["id"]), reverse=True)
    return stale


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--caches", required=True, help="the cache list response, as JSON")
    parser.add_argument(
        "--keep",
        type=int,
        default=DEFAULT_KEEP,
        help=f"generations to keep per restore key and ref (default {DEFAULT_KEEP})",
    )
    parser.add_argument(
        "--min-age-minutes",
        type=int,
        default=DEFAULT_MIN_AGE_MINUTES,
        help=f"never report an entry younger than this (default {DEFAULT_MIN_AGE_MINUTES})",
    )
    parser.add_argument(
        "--max-idle-days",
        type=float,
        default=DEFAULT_MAX_IDLE_DAYS,
        help=f"only report an entry nothing has read for this long (default {DEFAULT_MAX_IDLE_DAYS})",
    )
    parser.add_argument("--now", help="the current time, for tests")
    arguments = parser.parse_args(argv)
    if arguments.keep < 1:
        parser.error("--keep must keep at least one generation")
    if arguments.min_age_minutes < 0:
        parser.error("--min-age-minutes cannot be negative")
    if arguments.max_idle_days < 0:
        parser.error("--max-idle-days cannot be negative")
    now = parse_time(arguments.now) if arguments.now else None
    if arguments.now and now is None:
        parser.error("--now must be an ISO 8601 time")

    with open(arguments.caches, encoding="utf-8") as handle:
        payload = json.load(handle)

    stale = superseded(
        payload,
        arguments.keep,
        arguments.min_age_minutes,
        arguments.max_idle_days,
        now,
    )
    for entry in stale:
        print(f"{entry['id']}\t{entry['size']}\t{entry['key']}")
    reclaimed = sum(entry["size"] for entry in stale)
    print(
        f"{len(stale)} superseded cache entries, {reclaimed / 2**30:.2f} GB",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
