"""The mutation phases: attest the exact A and B from ARM, then off, observer install and return."""
from __future__ import annotations

import datetime as _dt
import json
import os
import time
from typing import Any, Callable, Dict, List, Optional, Tuple
from .az import Az
from .manifest import (AFTER_OFF_MINUTES, CLI_STEP_SECONDS, CLIENT_VM_NAME, INSTANCE_ID_RE, OFF_SETUP_MINUTES,
                       RETURN_RESERVE_MINUTES,
                       RUN_ID_RE, SAMPLE_SECONDS, client_tags, image_ref_digest, parse_utc, routable, same_group, same_id,
                       utc_now)
from .observer import OBSERVATION_MIN_SECONDS, OBSERVATION_SECONDS, read_observations, validate_worker
from .verdict import evaluate_samples


CLIENT_FIELDS = ("client_group", "client_vm_id", "client_group_id", "client_instance_id", "run_id", "client_sha",
                 "client_binary_sha256")


def validate_client(client: Any) -> List[str]:
    """Fail-closed schema for the provisioning output that identifies the exact A."""
    if not isinstance(client, dict):
        return ["client descriptor is not an object"]
    problems = [f"missing {field}" for field in CLIENT_FIELDS if field not in client]
    if problems:
        return problems
    problems = [f"{field} is not a string" for field in CLIENT_FIELDS if not isinstance(client[field], str) or not client[field]]
    if not problems and not RUN_ID_RE.fullmatch(client["run_id"]):
        problems.append("run_id is not the 32-hex-digit per-run identity")
    if not problems and not INSTANCE_ID_RE.fullmatch(client["client_instance_id"]):
        problems.append("client_instance_id is not a VM instance identity (vmId)")
    return problems


# A power call (deallocate or start) and the poll that verifies it share this bound.
POWER_CALL_SECONDS = 600


def arm_phase_deadline(az: Az, manifest: Dict[str, Any], reserve_minutes: int) -> Optional[str]:
    """Bind the phase to the manifest deadline minus the minutes reserved for what must
    still follow it; every ARM call, probe and poll the phase makes is cut off there.
    Returns the problem when nothing is left."""
    try:
        until = parse_utc(str(manifest["cleanup_deadline_utc"]))
    except ValueError:
        return "cleanup_deadline_utc is not an ISO-8601 instant"
    left = (until - utc_now()).total_seconds() - reserve_minutes * 60
    if left < 60:
        return f"less than a minute left before the reserved margin ({reserve_minutes} min) of the manifest deadline"
    az.deadline = time.monotonic() + left
    return None


def observation_budget(az: Az) -> float:
    """What an observation may take now: its own bound, or less when the phase deadline
    is nearer; below the observation minimum the caller must not start one."""
    left = az.left()
    return float(OBSERVATION_SECONDS) if left is None else min(float(OBSERVATION_SECONDS), left)


def observe_client(az: Az, manifest: Dict[str, Any], client: Dict[str, Any],
                   timeout: int = CLI_STEP_SECONDS) -> Tuple[Dict[str, Any], Optional[str]]:
    """Read A from ARM and attest it: same group ID, same VM ID, same instance identity
    and exactly this run's tags on group and VM. Returns the evidence (what ARM showed,
    for the journal) and the problem, if any."""
    evidence: Dict[str, Any] = {"a_group_id": None, "a_vm_id": None, "a_instance_id": None, "a_tags": None,
                                "a_group_tags": None}
    if (not same_group(client["client_group"], manifest["client_group"]) or client["client_sha"] != manifest["client_sha"]
            or client["client_binary_sha256"] != manifest["client_binary_sha256"] or client["run_id"] != manifest["run_id"]):
        return evidence, "client descriptor does not belong to this manifest"
    group = az.run(["group", "show", "-n", manifest["client_group"]], timeout=timeout)
    vm = az.run(["vm", "show", "-g", manifest["client_group"], "-n", CLIENT_VM_NAME], timeout=timeout)
    if not isinstance(group, dict) or not isinstance(vm, dict):
        return evidence, "client group or VM could not be read"
    evidence.update({"a_group_id": group.get("id"), "a_vm_id": vm.get("id"), "a_instance_id": vm.get("vmId"),
                     "a_tags": vm.get("tags"), "a_group_tags": group.get("tags")})
    if not same_id(group.get("id"), client["client_group_id"]) or not same_id(vm.get("id"), client["client_vm_id"]):
        return evidence, "client VM or group is not the resource provisioned for this run"
    # ARM IDs are name-based paths and survive a same-name recreation; the VM's
    # instance identity and the run identity in the tags do not.
    if vm.get("vmId") != client["client_instance_id"]:
        return evidence, "client VM is not the instance provisioned for this run"
    expected = client_tags(manifest)
    if group.get("tags") != expected or vm.get("tags") != expected:
        return evidence, "client group or VM does not carry exactly this run's tags"
    return evidence, None


def bound_client_state(az: Az, manifest: Dict[str, Any], client: Dict[str, Any],
                       timeout: int = CLI_STEP_SECONDS) -> Tuple[Dict[str, Any], Optional[str], Optional[str]]:
    """A's power state bound to one attested identity: the identity is attested before
    and after the state read, and the state counts only when both attestations pass on
    the same evidence, so a same-name replacement is a problem, never a state. Returns
    the evidence ARM showed (for the journal), the state, and the problem."""
    before, problem_before = observe_client(az, manifest, client, timeout)
    if problem_before:
        return before, None, problem_before
    state = az.power_state(manifest["client_group"], CLIENT_VM_NAME, timeout=timeout)
    after, problem_after = observe_client(az, manifest, client, timeout)
    if problem_after:
        return before, None, problem_after
    if before != after:
        return before, None, "client A changed identity around the state read"
    return before, state, None


def observe_client_state(az: Az, manifest: Dict[str, Any], client: Dict[str, Any],
                         timeout: int = CLI_STEP_SECONDS) -> Tuple[Dict[str, Any], Optional[str]]:
    """The evidence ARM showed for A together with its bound power state (None unless bound)."""
    evidence, state, _ = bound_client_state(az, manifest, client, timeout)
    return evidence, state


def client_power(az: Az, manifest: Dict[str, Any], client: Dict[str, Any],
                 timeout: int = CLI_STEP_SECONDS) -> Tuple[Optional[str], Optional[str]]:
    """The exact A's bound power state and the problem, for the mutation paths."""
    _, state, problem = bound_client_state(az, manifest, client, timeout)
    return state, problem


def attest_client(az: Az, manifest: Dict[str, Any], client: Dict[str, Any], timeout: int = CLI_STEP_SECONDS) -> Optional[str]:
    """The problem with A, or None when it is the exact provisioned resource right now."""
    return observe_client(az, manifest, client, timeout)[1]


def await_client_state(az: Az, manifest: Dict[str, Any], client: Dict[str, Any], target: str,
                       bound_seconds: int = 600) -> Optional[str]:
    """Wait for the exact A to report `target` under an absolute bound: every ARM
    request is handed what is left of it, the sleep never crosses it, and every poll
    re-attests A's identity, so the answer is only ever about the provisioned instance.
    Returns the problem."""
    deadline = time.monotonic() + bound_seconds
    if az.deadline is not None:
        deadline = min(deadline, az.deadline)
    while True:
        left = deadline - time.monotonic()
        # Five requests per poll share what is left, each cut off at the bound; a poll
        # that cannot fit five one-second requests is not started at all.
        if left < 5:
            return f"client A did not reach {target} within the bound"
        state, problem = client_power(az, manifest, client, timeout=int(left / 5))
        if problem:
            return f"{problem} while waiting for {target}"
        if state == target:
            return None
        left = deadline - time.monotonic()
        if left <= 0:
            return f"client A did not reach {target} within {bound_seconds}s (last {state})"
        time.sleep(min(5.0, left))


def attest_worker(az: Az, manifest: Dict[str, Any], worker: Dict[str, Any]) -> Tuple[Dict[str, Optional[str]], Optional[str]]:
    """B must be the worker recorded at baseline (group, VM and address as ARM reports
    them now), carry the manifest image's tag and be running, before any phase touches
    it or A. Returns the ARM observation and the first problem, if any."""
    expected = {"b_group_id": worker["group_id"], "b_vm_id": worker["vm_id"], "b_instance_id": worker["instance_id"], "b_host": worker["host"]}
    baseline = az.vm_identity(manifest["worker_group"], worker["vm_name"])
    if any(not same_id(baseline.get(field), expected[field]) for field in expected):
        return baseline, "worker B does not match the identity recorded at baseline"
    if baseline.get("b_image_ref") != image_ref_digest(manifest["worker_image"]):
        return baseline, "worker B does not carry the manifest image's reference tag"
    if az.power_state(manifest["worker_group"], worker["vm_name"]) != "PowerState/running":
        return baseline, "worker B is not running"
    return baseline, None


def phase_off(az: Az, manifest: Dict[str, Any], worker: Dict[str, Any], client: Dict[str, Any], journal_path: str,
              sample_seconds: int = SAMPLE_SECONDS, interval_seconds: Optional[float] = None,
              reader: Optional[Callable[..., Dict[str, Optional[int]]]] = None) -> Dict[str, Any]:
    """Deallocate A only, require deallocated, observe for the declared interval.

    `interval_seconds` and `reader` exist for deterministic tests of the mutation
    boundary; production callers leave them unset.
    """
    reader = reader or read_observations
    sample_seconds = max(1, sample_seconds)
    client_group = manifest["client_group"]
    problems = validate_worker(worker) + [f"client: {p}" for p in validate_client(client)]
    if problems:
        return {"passed": False, "findings": [f"descriptor: {problem}" for problem in problems]}
    # One absolute deadline for the whole phase: the manifest deadline minus what the
    # return phase and the cleanup window need after the interval, so A is deallocated
    # only when it can still be brought back. The declared interval plus the
    # pre-sampling work must fit inside it before anything is stopped.
    problem = arm_phase_deadline(az, manifest, AFTER_OFF_MINUTES) or attest_client(az, manifest, client)
    if problem:
        return {"passed": False, "findings": [f"{problem}; nothing stopped"]}
    interval = interval_seconds if interval_seconds is not None else manifest["off_minutes"] * 60
    if az.left() is not None and az.left() < OFF_SETUP_MINUTES * 60 + interval:
        return {"passed": False, "findings": ["the declared off interval and its setup do not fit before the phase deadline; "
                                              "nothing stopped"]}
    directory = os.path.dirname(os.path.abspath(journal_path))
    expected_identity = {"b_group_id": worker["group_id"], "b_vm_id": worker["vm_id"], "b_instance_id": worker["instance_id"],
                         "b_host": worker["host"], "a_group_id": client["client_group_id"], "a_vm_id": client["client_vm_id"],
                         "a_instance_id": client["client_instance_id"]}
    # The observed image tag is what the header records.
    baseline, problem = attest_worker(az, manifest, worker)
    if problem:
        return {"passed": False, "findings": [f"{problem}; nothing stopped"]}
    # The observer channel must already be the restricted one: the forced reader must
    # answer (and the sent command must not) before A is touched.
    if observation_budget(az) < OBSERVATION_MIN_SECONDS:
        return {"passed": False, "findings": ["phase deadline too near for the observer probe; nothing stopped"]}
    probe = reader(worker["host"], worker["port"], worker["host_key"], worker["observer_key_path"], directory,
                   observation_budget(az))
    if probe.get("channel") == "unrestricted":
        return {"passed": False, "findings": ["observer key runs the forced reader but is not `restrict`ed on B (a pty was "
                                              "granted); nothing stopped"]}
    if probe.get("channel") != "answered" or probe.get("progress") is None:
        return {"passed": False, "findings": ["observer key is not a working restricted read channel on B, or the counter "
                                              "is not readable yet; nothing stopped"]}
    # The client must actually be on, and must still be the exact A in the same read
    # that precedes the mutation: ARM offers no conditional power operation, so the
    # attestation is repeated immediately before the call and on every poll after it,
    # and A's group name is unique to this run, so nothing else can stand at the path.
    state, problem = client_power(az, manifest, client)
    if problem:
        return {"passed": False, "findings": [f"{problem}; nothing stopped"]}
    if state != "PowerState/running":
        return {"passed": False, "findings": ["client A is not running before the off interval; nothing stopped"]}
    # The last check before the irreversible call, with the remaining budget as it is
    # now: the deallocation and its poll share one bound, and the declared interval must
    # still fit after it.
    if az.left() is not None and az.left() < POWER_CALL_SECONDS + interval:
        return {"passed": False, "findings": ["the deallocation bound and the declared interval no longer fit before the "
                                              "phase deadline; nothing stopped"]}
    if az.dry_run:
        # The plan stays in memory: a dry run must not consume the production journal
        # path that the real run will need to create exclusively.
        if os.path.exists(journal_path):
            return {"passed": False, "findings": [f"journal {journal_path} already exists; the real run would refuse it"]}
        az.run(["vm", "deallocate", "-g", client_group, "-n", CLIENT_VM_NAME], mutating=True, timeout=POWER_CALL_SECONDS)
        return {"passed": False, "dry_run": True, "findings": ["dry run: A would be deallocated now; no sample taken"],
                "plan": az.journal}
    # The journal is created only now, once every check that could still refuse has
    # passed: a refusal before this point leaves no header behind, so the same path can
    # be retried; from here on the deallocation is attempted and the header must exist.
    try:
        # Exclusive creation proves the path is fresh and writable before A is touched;
        # the first line records the baseline the offline verdict must enforce.
        with open(journal_path, "x", encoding="utf-8") as handle:
            handle.write(json.dumps({"baseline": expected_identity, "worker_image": manifest["worker_image"],
                                     "observed_image_ref": baseline["b_image_ref"]}, sort_keys=True) + "\n")
            # Durable before the irreversible call: a crash right after the deallocate
            # must still leave the header naming what was stopped.
            handle.flush()
            os.fsync(handle.fileno())
    except FileExistsError:
        return {"passed": False, "findings": [f"journal {journal_path} already exists; each run needs a fresh path"]}
    except OSError as error:
        # Created by this call but not written through: remove it, so the path stays
        # retryable; nothing has been mutated.
        try:
            os.unlink(journal_path)
        except OSError:
            pass
        return {"passed": False, "findings": [f"journal {journal_path} cannot be written: {type(error).__name__}"]}
    call_started = time.monotonic()
    az.run(["vm", "deallocate", "-g", client_group, "-n", CLIENT_VM_NAME], mutating=True, timeout=POWER_CALL_SECONDS)
    problem = await_client_state(az, manifest, client, "PowerState/deallocated",
                                 bound_seconds=int(POWER_CALL_SECONDS - (time.monotonic() - call_started)))
    if problem:
        return {"passed": False, "findings": [problem]}
    samples: List[Dict[str, Any]] = []
    first_slot = utc_now()
    origin = time.monotonic()
    end = origin + interval
    if az.deadline is not None:
        end = min(end, az.deadline)
    index = 0
    while True:
        # Fixed schedule from the first slot; the actual instant is recorded beside it.
        slot_offset = index * sample_seconds
        scheduled_at = (first_slot + _dt.timedelta(seconds=slot_offset)).isoformat()
        # The observation instant is the start of the sample: acquisition latency belongs
        # to the sample, never to the slot.
        at = utc_now().isoformat()
        # A first: its state, bracketed by its attestation, is what the sample's instant
        # attributes; B's reads and the SSH observation follow, so a transition of A
        # during them can never be back-dated to `at`.
        a_evidence, a_power = observe_client_state(az, manifest, client)
        identity = az.vm_identity(manifest["worker_group"], worker["vm_name"])
        host = identity.get("b_host")
        b_power = az.power_state(manifest["worker_group"], worker["vm_name"])
        # Read only through the endpoint ARM reports for B right now, pinned to the
        # attested key, with the restricted observer key whose forced reader returns
        # the counter and, when the task exposes one, the worker-owned checkpoint.
        # The reading gets what is left of the phase, never more than its own bound.
        observed = (reader(host, worker["port"], worker["host_key"], worker["observer_key_path"], directory,
                           observation_budget(az))
                    if routable(host) else {"progress": None, "checkpoint": None})
        # B's state and reading count only when B's identity (group, VM, instance, image
        # tag, endpoint) is the same before and after them; a replacement or repointed
        # endpoint in between leaves an unattested sample, never a stale identity next to
        # a replacement's state.
        if az.vm_identity(manifest["worker_group"], worker["vm_name"]) != identity or observed.get("channel") != "answered":
            # A replaced B, or a channel that is not the restricted reader any more
            # (refused, unavailable, or answering with a pty granted), leaves an
            # unreadable sample: its state and reading are never evidence.
            b_power, observed = None, {"progress": None, "checkpoint": None}
        # A's evidence (group and VM IDs, instance identity, the tag maps ARM showed) is
        # journaled verbatim so the offline verdict re-derives that the exact provisioned
        # A was off in every sample, not merely a VM of that name.
        sample = {"scheduled_at": scheduled_at, "at": at,
                  "a_power": a_power,
                  **a_evidence,
                  "b_power": b_power,
                  **identity,
                  "progress": observed.get("progress"),
                  "checkpoint": observed.get("checkpoint") if worker.get("checkpoint_path") else None}
        samples.append(sample)
        try:
            with open(journal_path, "a", encoding="utf-8") as handle:
                handle.write(json.dumps(sample, sort_keys=True) + "\n")
                handle.flush()
                os.fsync(handle.fileno())
        except OSError as error:
            return {"passed": False, "findings": [f"journal became unwritable during the interval: {type(error).__name__}"],
                    "samples": len(samples)}
        if time.monotonic() > end:
            break
        # Sleep to the next slot; a slow sample skips slots, which the journal shows. A
        # slot that falls after the end, or leaves no room for one observation before
        # the phase deadline, is not started: the sleep never crosses the end.
        index += 1
        while origin + index * sample_seconds < time.monotonic():
            index += 1
        next_slot = origin + index * sample_seconds
        # The slot at the endpoint is still sampled: the interval is closed on its end.
        if next_slot > end or (az.deadline is not None and az.deadline - next_slot < OBSERVATION_MIN_SECONDS):
            break
        time.sleep(max(0.0, next_slot - time.monotonic()))
    return evaluate_samples(samples, manifest["off_minutes"], manifest["lease_seconds"], manifest["worker_image"],
                            expected_identity, client_tags(manifest))


def phase_return(az: Az, manifest: Dict[str, Any], client: Dict[str, Any]) -> Dict[str, Any]:
    """Start A only and require running; the client reconnect step follows on A."""
    client_group = manifest["client_group"]
    problems = validate_client(client)
    if problems:
        return {"passed": False, "findings": [f"client descriptor: {problem}" for problem in problems]}
    # The return keeps the cleanup window untouched after itself.
    problem = arm_phase_deadline(az, manifest, RETURN_RESERVE_MINUTES) or attest_client(az, manifest, client)
    if problem:
        return {"passed": False, "findings": [f"{problem}; nothing started"]}
    # Only the exact A, and only when it is actually off, can be brought back: identity
    # and state are read together immediately before the start and on every poll after.
    state, problem = client_power(az, manifest, client)
    if problem:
        return {"passed": False, "findings": [f"{problem}; nothing started"]}
    if state != "PowerState/deallocated":
        return {"passed": False, "findings": ["client A is not deallocated before the return; nothing started"]}
    # The start and its poll share one bound, which must fit before the phase deadline.
    if az.left() is not None and az.left() < POWER_CALL_SECONDS:
        return {"passed": False, "findings": ["the start bound no longer fits before the phase deadline; nothing started"]}
    call_started = time.monotonic()
    az.run(["vm", "start", "-g", client_group, "-n", CLIENT_VM_NAME], mutating=True, timeout=POWER_CALL_SECONDS)
    if az.dry_run:
        return {"passed": False, "dry_run": True, "findings": ["dry run: A would be started now"], "plan": az.journal}
    problem = await_client_state(az, manifest, client, "PowerState/running",
                                 bound_seconds=int(POWER_CALL_SECONDS - (time.monotonic() - call_started)))
    if problem:
        return {"passed": False, "findings": [problem]}
    return {"passed": True, "findings": []}
