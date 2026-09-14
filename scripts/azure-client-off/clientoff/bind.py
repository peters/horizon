"""Bind the product-created worker group into the frozen manifest, exactly once.

The product draws B's workflow and job identities when setup is submitted on A, after A
was provisioned from the manifest. `bind-worker` closes that gap without a hand edit: it
reads the group the operator names, requires the adapter tags its name is derived from,
requires it to be absent from the pre-run inventory, requires the provisioning descriptor
to carry the digest of the very manifest being bound, journals the group (the only
cleanup authorization) before it touches anything, puts B's VM under the deadline
reaper with a read-back, and only then writes `worker_group`. Every refusal happens
before any mutation, and the mutation happens only after B is deletable."""
from __future__ import annotations

from typing import Any, Callable, Dict, List, Optional

from .az import Az
from .cleanup import group_list, identity_record, journal_records, worker_record_bound
from .manifest import (REAPER_TAGS, RUN_ID_RE, UUID_RE, WORKER_GROUP_RE, WORKER_VM_NAME, is_bound, manifest_digest,
                       same_id)

DESCRIPTOR_DIGEST = "manifest_sha256"


def adapter_identity(group: str) -> Optional[str]:
    """The problem with the group name, or None when it is the adapter's with two exact UUIDs."""
    parts = WORKER_GROUP_RE.fullmatch(group)
    if not parts or not all(UUID_RE.fullmatch(part) for part in parts.groups()):
        return f"{group} is not the adapter's horizon-ws-<workflow>-<job> name"
    return None


def bind_checks(manifest: Dict[str, Any], group: str, before: Any, created: Any, client: Any) -> Dict[str, Any]:
    """Every refusal that needs no ARM read. Pure, so it is testable on its own; returns
    the problems and the journal record already present for the group, if any."""
    problems: List[str] = []
    if is_bound(manifest):
        if same_id(str(manifest["worker_group"]), group):
            problems.append(f"the manifest is already bound to {group}")
        else:
            problems.append(f"the manifest is already bound to {manifest['worker_group']}; a run binds one worker")
    shape = adapter_identity(group)
    if shape:
        problems.append(shape)
    inventory, records = group_list(before), journal_records(created)
    if inventory is None:
        problems.append("groups-before is not a JSON array of group names")
    elif any(same_id(name, group) for name in inventory):
        problems.append(f"{group} existed before the run; it is not this run's worker")
    if records is None:
        problems.append("created is not a JSON array of identity records")
    if not isinstance(client, dict) or not isinstance(client.get(DESCRIPTOR_DIGEST), str) \
            or not isinstance(client.get("run_id"), str):
        problems.append(f"the client descriptor carries no {DESCRIPTOR_DIGEST} and run_id; provision A with this harness first")
    else:
        if client["run_id"] != manifest.get("run_id") or not RUN_ID_RE.fullmatch(str(manifest.get("run_id", ""))):
            problems.append("the client descriptor belongs to another run")
        if client[DESCRIPTOR_DIGEST] != manifest_digest(manifest):
            problems.append("the manifest changed since A was provisioned (its unbound digest differs from the descriptor's); "
                            "nothing but worker_group may change between provisioning and binding")
    journaled = records.get(group.casefold()) if records else None
    return {"problems": problems, "journaled": journaled}


def worker_vm_id(group_id: str) -> str:
    """The adapter's worker VM inside its own group: one worker per group, fixed name."""
    return f"{group_id}/providers/Microsoft.Compute/virtualMachines/{WORKER_VM_NAME}"


def bind_worker(az: Az, manifest: Dict[str, Any], group: str, before: Any, created: Any, client: Any,
                persist_journal: Optional[Callable[[List[Dict[str, Any]]], None]] = None) -> Dict[str, Any]:
    """Read, check, journal, tag with read-back, then hand back the bound manifest for
    the caller to write. `persist_journal` is called with the complete journal once the
    group and its VM are proven and before the first mutation, so a crash or a failing
    write can never leave a tagged worker without the record that authorizes deleting
    it; a raising callback aborts the binding untouched. A dry run walks every read and
    journals nothing. Nothing here writes the manifest."""
    checks = bind_checks(manifest, group, before, created, client)
    if checks["problems"]:
        return {"passed": False, "bound": False, "findings": checks["problems"]}
    record = identity_record(az, group)
    if record is None:
        return {"passed": False, "bound": False, "findings": [f"group {group} could not be read; nothing bound"]}
    if not same_id(record["name"], group) or not worker_record_bound(record):
        return {"passed": False, "bound": False,
                "findings": [f"group {group} does not carry the adapter tags its name is derived from; nothing bound"]}
    journaled = checks["journaled"]
    if journaled is not None and (not same_id(journaled["id"], record["id"]) or journaled["tags"] != record["tags"]):
        return {"passed": False, "bound": False,
                "findings": [f"group {group} is journaled under another identity or tag set; nothing bound"]}
    vm_id = worker_vm_id(record["id"])
    vm = az.run(["vm", "show", "--ids", vm_id])

    if not isinstance(vm, dict) or not same_id(vm.get("id"), vm_id) or not isinstance(vm.get("tags"), dict):
        return {"passed": False, "bound": False,
                "findings": [f"the worker VM in {group} could not be read yet (the deployment may still be in flight); "
                             "nothing bound; journal-group is still available"]}
    if any(vm["tags"].get(key) != record["tags"].get(key) for key in ("horizon-workflow-id", "horizon-job-id")):
        return {"passed": False, "bound": False,
                "findings": [f"the worker VM in {group} does not carry the group's adapter identity; nothing bound"]}
    journal = list(created) if journaled is not None else [*created, record]
    if az.dry_run:
        return {"passed": False, "dry_run": True, "bound": False, "would_bind": record["name"],
                "findings": ["dry run: the group would be journaled, the VM tagged for the reaper and the manifest bound now"],
                "plan": az.journal}
    # The journal is the only authorization cleanup has for deleting B, so it is
    # persisted before anything is written to Azure: a crash, a kill or a failing write
    # after this point still leaves a deletable worker, never a tagged orphan.
    if persist_journal is not None:
        try:
            persist_journal(journal)
        except OSError as error:
            return {"passed": False, "bound": False,
                    "findings": [f"the creation journal could not be written ({type(error).__name__}); nothing tagged or bound"]}
    # The product's deployment writes worker identity tags only, and the subscription's
    # deadline reaper selects on `purpose` and `deadline`: without them a controller that
    # dies between the product's create and the run's end leaves B running unreaped.
    deadline = str(manifest["cleanup_deadline_utc"])
    az.run(["tag", "update", "--resource-id", vm_id, "--operation", "merge", "--tags",
            f"purpose={REAPER_TAGS['purpose']}", f"deadline={deadline}"], mutating=True)
    shown = az.run(["vm", "show", "--ids", vm_id, "--query", "tags"])
    if not isinstance(shown, dict) or shown.get("purpose") != REAPER_TAGS["purpose"] or shown.get("deadline") != deadline:
        return {"passed": False, "bound": False, "journaled": record,
                "findings": ["the reaper tags could not be read back from the worker VM; nothing bound, retry "
                             "(the group is journaled, so cleanup can still delete it)"]}
    # The tags go on the VM only: the product checks its own tags as a subset, so extra
    # VM tags are tolerated, while cleanup refuses a group whose tag set changed since it
    # was journaled. The journal record is therefore ARM's group identity, untouched.
    bound = dict(manifest, worker_group=record["name"])
    return {"passed": True, "bound": True, "worker_group": record["name"], "worker_vm_id": vm_id,
            "journaled": record, "journal": journal, "manifest": bound,
            "bound_manifest_sha256": manifest_digest(bound, bound=True), "findings": []}
