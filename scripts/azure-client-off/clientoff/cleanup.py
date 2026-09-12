"""Cleanup authorization: delete exactly the groups this run journaled, proven owned again immediately before the delete."""
from __future__ import annotations

import time
from typing import Any, Dict, List, Optional
from .az import Az
from .manifest import ARM, GROUP_RE, RUN_ID_RE, UUID_RE, same_id


def group_list(value: Any) -> Optional[List[str]]:
    """An inventory is a JSON array of group names; anything else is refused."""
    if isinstance(value, list) and all(isinstance(item, str) and GROUP_RE.fullmatch(item) for item in value):
        return value
    return None


def journal_records(value: Any) -> Optional[Dict[str, Dict[str, Any]]]:
    """The creation journal is a JSON array of {name, id, tags} records taken from ARM
    when the group was created; anything else is refused. Keyed by case-folded name."""
    if not isinstance(value, list):
        return None
    records: Dict[str, Dict[str, Any]] = {}
    for item in value:
        if (not isinstance(item, dict) or not isinstance(item.get("name"), str) or not GROUP_RE.fullmatch(item["name"])
                or not isinstance(item.get("id"), str) or not item["id"] or not isinstance(item.get("tags"), dict)
                or not all(isinstance(k, str) and isinstance(v, str) for k, v in item["tags"].items())):
            return None
        if item["name"].casefold() in records:
            return None  # conflicting identities for one name: the journal is not trustworthy
        records[item["name"].casefold()] = item
    return records


def run_bound(record: Dict[str, Any], manifest: Dict[str, Any]) -> bool:
    """Whether a journaled record is bound to this run and nothing else. ARM group IDs
    are name-based and every other tag is reproducible, so the tags must carry the
    identity the name itself is derived from: A's group (`horizon-client-<run_id>`)
    must carry exactly the manifest's `run_id`; B's group (`horizon-ws-<workflow>-<job>`)
    must carry the adapter's workflow and job identities."""
    tags, name = record.get("tags"), str(record.get("name", ""))
    run_id = str(manifest.get("run_id", ""))
    if not isinstance(tags, dict):
        return False
    if name.casefold() == str(manifest["client_group"]).casefold():
        return bool(RUN_ID_RE.fullmatch(run_id)) and tags.get("run_id") == run_id \
            and name.casefold() == f"horizon-client-{run_id}"
    if name.casefold() == str(manifest["worker_group"]).casefold():
        workflow, job = str(tags.get("horizon-workflow-id", "")), str(tags.get("horizon-job-id", ""))
        return bool(UUID_RE.fullmatch(workflow) and UUID_RE.fullmatch(job)) and name.casefold() == f"horizon-ws-{workflow}-{job}"
    return False


def owned_now(az: Az, record: Dict[str, Any], manifest: Dict[str, Any]) -> Optional[bool]:
    """Immediately before a delete: is the group still the exact resource that was
    journaled at creation (same ARM ID, identical tag set) and bound to this run? None
    when it cannot be read; an absent, recreated or retagged group is not ours, and a
    record not bound to this run never authorizes a delete, nor even a read."""
    if not run_bound(record, manifest):
        return False
    shown = az.run(["group", "show", "-n", record["name"]])
    if not isinstance(shown, dict) or not isinstance(shown.get("tags"), dict):
        return None
    return same_id(shown.get("id"), record["id"]) and shown["tags"] == record["tags"]


def identity_record(az: Az, group: str) -> Optional[Dict[str, Any]]:
    """Read a group's ARM identity and tags into a journal record."""
    shown = az.run(["group", "show", "-n", group])
    if not isinstance(shown, dict) or not isinstance(shown.get("id"), str) or not isinstance(shown.get("name"), str):
        return None
    if not isinstance(shown.get("tags"), dict):
        return None
    return {"name": shown["name"], "id": shown["id"], "tags": shown["tags"]}


def cleanup_targets(manifest: Dict[str, Any], before: Any, created: Any) -> Dict[str, Any]:
    """Which groups may be deleted: only those this run journaled as created, bound to
    this run, and never one that existed before the run. Malformed inputs refuse
    everything. Pure, so the refusal is testable; the returned records carry the
    identity to re-check."""
    wanted = [manifest["client_group"], manifest["worker_group"]]
    before, records = group_list(before), journal_records(created)
    if before is None or records is None:
        return {"delete": [], "refused": wanted, "malformed": True}
    pre_existing = {name.casefold() for name in before}
    refused = [group for group in wanted if group.casefold() in pre_existing or group.casefold() not in records
               or not run_bound(records[group.casefold()], manifest)]
    return {"delete": [records[group.casefold()] for group in wanted if group not in refused], "refused": refused}


def phase_cleanup(az: Az, manifest: Dict[str, Any], before: List[str], created: List[str]) -> Dict[str, Any]:
    """Delete only the exact groups this run created; verify absence and untouched peers.
    A dry run walks the same authorization (the reads happen, the delete is journaled
    and suppressed by `Az.run`) and stops before waiting for an absence it never caused."""
    targets = cleanup_targets(manifest, before, created)
    if targets.get("malformed"):
        return {"passed": False, "deleted": [],
                "findings": ["groups-before is not a JSON array of names or created is not a JSON array of identity records"]}
    findings = [f"refusing to delete {group}: pre-existing, not journaled as created by this run, or journaled "
                "without this run's identity in its tags" for group in targets["refused"]]
    deleting = []
    for record in targets["delete"]:
        group = record["name"]
        # A same-named group recreated or retagged since the journal entry is not ours.
        owned = owned_now(az, record, manifest)
        if owned is None:
            findings.append(f"refusing to delete {group}: ownership could not be read")
        elif not owned:
            findings.append(f"refusing to delete {group}: absent, replaced or retagged since it was journaled")
        else:
            deleting.append(group)
            # Delete the resource ID that was journaled and re-attested a moment ago. ARM
            # offers no conditional delete for resource groups and the ID is name-based,
            # so the guarantee rests on the name: unique to this run, so nothing else can
            # legitimately stand at this path in the window between the re-read and the
            # delete reaching ARM.
            az.run(["rest", "--method", "delete", "--url", f"{ARM}{record['id']}?api-version=2022-09-01"], mutating=True)
    targets["delete"] = deleting
    if az.dry_run:
        return {"passed": False, "dry_run": True, "findings": findings, "deleted": [], "would_delete": deleting}
    deadline = time.monotonic() + 1_500
    unresolved: Dict[str, str] = {}
    while time.monotonic() < deadline:
        # False is proven absence; True is presence; None (timeout, CLI error, malformed
        # answer) is unknown and never counts as absence.
        unresolved = {}
        for group in targets["delete"]:
            exists = az.run(["group", "exists", "-n", group])
            if exists is True:
                unresolved[group] = "present"
            elif exists is not False:
                unresolved[group] = "unknown"
        if not unresolved:
            break
        time.sleep(15)
    for group, state in sorted(unresolved.items()):
        findings.append(f"group {group} {state} at the bound: absence not proven")
    inventory = az.run(["group", "list"])
    names = group_list([group.get("name") for group in inventory]
                       if isinstance(inventory, list) and all(isinstance(group, dict) for group in inventory) else None)
    if names is None:
        findings.append("final resource-group inventory unreadable: unchanged pre-existing resources not proven")
    elif sorted(name.casefold() for name in names) != sorted(name.casefold() for name in before):
        findings.append("pre-existing resource groups changed during the run")
    return {"passed": not findings, "findings": findings, "deleted": targets["delete"]}
