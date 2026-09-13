#!/usr/bin/env python3
"""Azure client-off acceptance harness for #474 / #475 (Azure lane).

Topology: client VM A runs the exact Horizon client with a persistent home; a separate
worker B is created through the product path (or, until that is wired for Azure, the
adapter's live driver, in which case the run is an adapter-only rehearsal, never the
product pass); observer C is this controller. The harness deallocates A only, verifies
`PowerState/deallocated`, observes B's identity and task progress read-only for the
declared interval, starts A only, and cleans up exactly the journaled resources.

Observer C's only reach into B is a pinned SSH session with a key that sshd restricts
(`restrict,command=`) to a forced reader of the progress and checkpoint files; the key
is installed through the ARM run-command channel and accepted only once the forced
reader answers a session that asked for another command.

Nothing here logs in, registers a provider or installs an extension. Every `az` call is
an argument list without a shell, bounded in time. The observer never renews a lease,
reconnects a terminal, checkpoints or replays a task. Counter progress is recorded as
counter progress, never labelled a checkpoint.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from typing import Any, List, Optional

# The harness is a script, run from any directory; its modules live beside it.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from clientoff.az import Az  # noqa: E402
from clientoff.cleanup import identity_record, journal_records, phase_cleanup  # noqa: E402
from clientoff.manifest import TOOL_VERSION, validate_manifest  # noqa: E402
from clientoff.observer import observer_authorized_line  # noqa: E402
from clientoff.phases import phase_install_observer, phase_off, phase_remove_observer, phase_return  # noqa: E402
from clientoff.verdict import verdict_from_records  # noqa: E402

def load_json(path: str) -> Any:
    """A file's JSON, or a sentinel object that no validator accepts."""
    try:
        with open(path, encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, json.JSONDecodeError) as error:
        return {"__unreadable__": type(error).__name__}


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--manifest", required=True, help="JSON manifest frozen before renting")
    parser.add_argument("--journal", default="client-off-journal.ndjson")
    parser.add_argument("--dry-run", action="store_true", help="never issue a mutating az call")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("validate", help="check the manifest; nothing is called")
    line = sub.add_parser("observer-key-line", help="print the restricted authorized_keys line for the observer key")
    line.add_argument("--public-key", required=True, help="the observer's Ed25519 public key file")
    line.add_argument("--progress-path", required=True)
    line.add_argument("--checkpoint-path")
    journal = sub.add_parser("journal-group", help="append a group's ARM identity and tags to the creation journal")
    journal.add_argument("--group", required=True)
    journal.add_argument("--created", required=True, help="JSON array file to append to (created if absent)")
    off = sub.add_parser("off", help="deallocate A only and observe B for the declared interval")
    off.add_argument("--client", required=True, help="provision-client.sh output JSON (exact A identity)")
    off.add_argument("--worker", required=True,
                     help="JSON with vm_name, port, host_key, observer_key_path (the restricted key), progress_path, "
                          "optional checkpoint_path, and the baseline identity group_id, vm_id, host")
    install = sub.add_parser("install-observer-key", help="install the restricted observer key on B via run-command")
    install.add_argument("--worker", required=True, help="the same worker JSON the off phase reads")
    install.add_argument("--public-key", required=True, help="the observer's Ed25519 public key file")
    remove = sub.add_parser("remove-observer-key", help="remove the observer key from B once the run is over")
    remove.add_argument("--worker", required=True, help="the same worker JSON the off phase reads")
    remove.add_argument("--public-key", required=True, help="the observer's Ed25519 public key file")
    ret = sub.add_parser("return", help="start A only and require running")
    ret.add_argument("--client", required=True, help="provision-client.sh output JSON (exact A identity)")
    cleanup = sub.add_parser("cleanup", help="delete exactly the groups this run created")
    cleanup.add_argument("--groups-before", required=True, help="JSON list of group names recorded before the run")
    cleanup.add_argument("--resources-before", required=True,
                         help="JSON list of ARM resource IDs recorded before the run (az resource list --query '[].id')")
    cleanup.add_argument("--created", required=True,
                         help="creation journal: JSON array of {name, id, tags} records written at creation "
                              "(provision-client.sh for A, journal-group for B)")
    verdict = sub.add_parser("verdict", help="evaluate a recorded journal without any provider call")
    verdict.add_argument("--journal-in", required=True)
    args = parser.parse_args(argv)
    try:
        with open(args.manifest, encoding="utf-8") as handle:
            manifest = json.load(handle)
    except (OSError, json.JSONDecodeError) as error:
        print(json.dumps({"runnable": False, "problems": [f"manifest unreadable: {type(error).__name__}"]}, indent=2))
        return 2
    # Only the phases that rent or keep compute alive need a future deadline.
    # Phases that rent, keep compute alive or mutate a live VM need a deadline that
    # outlasts them; verdict and cleanup also run after it.
    problems = validate_manifest(manifest, renting=args.command in ("validate", "off", "install-observer-key", "return"),
                                 phase=args.command)
    if problems:
        print(json.dumps({"runnable": False, "problems": problems}, indent=2))
        return 2
    if args.command == "validate":
        print(json.dumps({"runnable": True, "tool_version": TOOL_VERSION}, indent=2))
        return 0
    if args.command == "observer-key-line":
        try:
            with open(args.public_key, encoding="utf-8") as handle:
                public_key = handle.read().strip().split("\n")[0]
            print(observer_authorized_line(" ".join(public_key.split()[:2]), args.progress_path, args.checkpoint_path))
        except (OSError, ValueError, IndexError) as error:
            print(json.dumps({"passed": False, "findings": [f"observer key line: {error}"]}, indent=2))
            return 2
        return 0
    if args.command == "verdict":
        try:
            with open(args.journal_in, encoding="utf-8") as handle:
                records = [json.loads(line) for line in handle if line.strip()]
        except (OSError, json.JSONDecodeError) as error:
            result = {"passed": False, "findings": [f"journal unreadable: {type(error).__name__}"]}
        else:
            result = verdict_from_records(records, manifest)
        print(json.dumps(result, indent=2))
        return 0 if result["passed"] else 1
    az = Az(manifest["subscription_id"], dry_run=args.dry_run)
    if args.command == "journal-group":
        record = identity_record(az, args.group)
        if record is None:
            print(json.dumps({"passed": False, "findings": [f"group {args.group} could not be read"]}, indent=2))
            return 1
        existing: Any = []
        if os.path.exists(args.created):
            existing = load_json(args.created)
        records = journal_records(existing)
        if records is None:
            print(json.dumps({"passed": False, "findings": ["created is not a JSON array of identity records"]}, indent=2))
            return 1
        if record["name"].casefold() in records:
            print(json.dumps({"passed": False, "findings": [f"{record['name']} is already journaled; the journal is append-only"]}, indent=2))
            return 1
        existing = [*existing, record]
        # Atomic replace: the only cleanup authorization is never left half-written.
        temporary = f"{args.created}.tmp"
        with open(temporary, "w", encoding="utf-8") as handle:
            json.dump(existing, handle, indent=2, sort_keys=True)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, args.created)
        print(json.dumps({"passed": True, "journaled": record}, indent=2))
        return 0
    if args.command in ("off", "return"):
        client = load_json(args.client)
    if args.command == "install-observer-key":
        result = phase_install_observer(az, manifest, load_json(args.worker), args.public_key,
                                        os.path.dirname(os.path.abspath(args.journal)) or ".")
    elif args.command == "remove-observer-key":
        result = phase_remove_observer(az, manifest, load_json(args.worker), args.public_key,
                                       os.path.dirname(os.path.abspath(args.journal)) or ".")
    elif args.command == "off":
        worker = load_json(args.worker)
        result = phase_off(az, manifest, worker, client, args.journal)
    elif args.command == "return":
        result = phase_return(az, manifest, client)
    else:
        try:
            with open(args.groups_before, encoding="utf-8") as handle:
                before = json.load(handle)
            with open(args.created, encoding="utf-8") as handle:
                created = json.load(handle)
            with open(args.resources_before, encoding="utf-8") as handle:
                resources_before = json.load(handle)
        except (OSError, json.JSONDecodeError) as error:
            result = {"passed": False, "deleted": [],
                      "findings": [f"groups-before, created or resources-before unreadable: {type(error).__name__}; nothing deleted"]}
        else:
            result = phase_cleanup(az, manifest, before, created, resources_before)
    result["az_calls"] = az.journal
    print(json.dumps(result, indent=2))
    return 0 if result.get("passed") else 1


if __name__ == "__main__":
    sys.exit(main())
