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

Generations are ordered by `last_accessed_at`, never by `created_at`. The hash
is not a sequence: a lockfile can return to an earlier state, so the generation
that runs restore today can be the oldest one by creation. In this repository
the live `linux-test` entry was created three minutes before the superseded one
beside it and was still the only one any run had touched in two days. Access
time is also the currency GitHub itself uses, both for the 7-day removal and for
the eviction order at the cache limit.

A ref is part of the group because cache scoping is directional: a run restores
entries of its own branch, the default branch and, for a pull request, the base
branch, but never a sibling branch. Grouping per ref keeps a prune that one
branch starts from reaching generations that only another branch can use.

Two cases are deliberately left alone, because GitHub clears them without help:
it removes an entry that no run has accessed for more than 7 days, and at the
repository cache limit it deletes the least recently accessed entries until the
total fits. So a group with one member reports nothing, even when a toolchain or
lockfile change has made its restore key unreachable.

With the default `--keep 1`, a lockfile change retires the generation before it
on the next prune. A pull request still on the old lockfile then finds no exact
match and falls back to the restore key, which serves the new entry, so it pays
a partial build once rather than keeping a second copy of every cache.

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


def superseded(payload, keep=DEFAULT_KEEP, min_age_minutes=DEFAULT_MIN_AGE_MINUTES, now=None):
    """The entries to delete, most recently accessed first."""
    now = now or datetime.now(timezone.utc)
    youngest_allowed = now - timedelta(minutes=min_age_minutes)
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
        # A fresh entry is kept whatever its place in the order: a run can sit
        # between the save of one job and the restore of the next.
        stale.extend(
            entry
            for entry in group[keep:]
            if entry["created"] is not None and entry["created"] <= youngest_allowed
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
    parser.add_argument("--now", help="the current time, for tests")
    arguments = parser.parse_args(argv)
    if arguments.keep < 1:
        parser.error("--keep must keep at least one generation")
    if arguments.min_age_minutes < 0:
        parser.error("--min-age-minutes cannot be negative")
    now = parse_time(arguments.now) if arguments.now else None
    if arguments.now and now is None:
        parser.error("--now must be an ISO 8601 time")

    with open(arguments.caches, encoding="utf-8") as handle:
        payload = json.load(handle)

    stale = superseded(payload, arguments.keep, arguments.min_age_minutes, now)
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
