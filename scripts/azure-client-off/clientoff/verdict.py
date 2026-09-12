"""Pure verdict over recorded observer samples; no provider call is ever made here."""
from __future__ import annotations

from typing import Any, Dict, List, Optional
from .manifest import INSTANCE_ID_RE, SAMPLE_JITTER_SECONDS, SAMPLE_SECONDS, image_ref_digest, parse_utc, routable, same_id


def evaluate_samples(samples: List[Dict[str, Any]], off_minutes: int, lease_seconds: int,
                     expected_image_ref: Optional[str] = None,
                     expected_identity: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
    """Pure verdict over observer samples taken while A was meant to be off.

    Each sample: {"at": ISO instant, "a_power": str|None, "b_group_id": str|None,
    "b_vm_id": str|None, "b_instance_id": str|None (the VM's vmId, which a same-name
    recreation does not keep), "b_host": str|None, "b_image_ref": str|None (the worker's
    image-reference tag), "b_power": str|None, "progress": int|None, "checkpoint":
    int|None}. Missing fields are misses, never passes. Counters and checkpoints are
    judged apart. With `expected_image_ref` (the manifest's image reference) every
    sample must carry the adapter's tag for exactly that image; with
    `expected_identity` (the group ID, VM ID, instance identity and address recorded
    at baseline, before A was stopped) the first sample must be that very worker.
    """
    findings: List[str] = []
    if len(samples) < 2:
        return {"passed": False, "findings": ["fewer than two samples"], "samples": len(samples)}
    text_fields = ("a_power", "b_group_id", "b_vm_id", "b_instance_id", "b_host", "b_image_ref", "b_power")
    if any(not isinstance(sample, dict) or any(sample.get(field) is not None and not isinstance(sample.get(field), str)
                                                for field in text_fields) for sample in samples):
        return {"passed": False, "findings": ["a sample has a field of the wrong shape"], "samples": len(samples)}
    try:
        times = [parse_utc(str(sample["at"])) for sample in samples]
    except (KeyError, ValueError, TypeError):
        return {"passed": False, "findings": ["a sample has a missing or invalid timestamp"], "samples": len(samples)}
    if expected_identity is not None:
        first = samples[0]
        mismatched = [field for field in ("b_group_id", "b_vm_id", "b_instance_id", "b_host")
                      if not same_id(first.get(field), expected_identity.get(field))]
        if mismatched:
            findings.append(f"first sample does not match the worker recorded at baseline: {mismatched}")
    if any(later < earlier for earlier, later in zip(times, times[1:])):
        return {"passed": False, "findings": ["sample timestamps are not monotonic"], "samples": len(samples)}
    span = (times[-1] - times[0]).total_seconds()
    if span < off_minutes * 60:
        findings.append(f"observed {span:.0f}s, declared interval {off_minutes * 60}s")
    if span <= lease_seconds:
        findings.append(f"observed {span:.0f}s did not exceed the lease of {lease_seconds}s")
    # Cadence: #475 wants a sample at least every 15 s, with scheduled and actual times
    # recorded. With scheduled instants present, a sample is late when it lands more
    # than the jitter allowance after its slot, and a jump in scheduled slots is that
    # many missed observations; without them, the inter-sample gaps stand in.
    # Every sample carries its scheduled instant; there is no inference from gaps.
    scheduled = [sample.get("scheduled_at") for sample in samples]
    if not all(isinstance(value, str) for value in scheduled):
        return {"passed": False, "findings": ["a sample has no scheduled instant"], "samples": len(samples)}
    try:
        slots = [parse_utc(str(value)) for value in scheduled]
    except ValueError:
        return {"passed": False, "findings": ["a sample has an invalid scheduled instant"], "samples": len(samples)}
    deltas = [(later - earlier).total_seconds() for earlier, later in zip(slots, slots[1:])]
    # Slots are computed instants: strictly increasing exact multiples of the cadence.
    if any(delta <= 0 or abs(delta / SAMPLE_SECONDS - round(delta / SAMPLE_SECONDS)) > 1e-6 for delta in deltas):
        return {"passed": False, "findings": ["scheduled instants are not increasing 15-second slots"], "samples": len(samples)}
    if any(actual < slot for slot, actual in zip(slots, times)):
        return {"passed": False, "findings": ["a sample was recorded before its scheduled slot"], "samples": len(samples)}
    late = sum(1 for slot, actual in zip(slots, times) if (actual - slot).total_seconds() > SAMPLE_JITTER_SECONDS)
    misses = sum(round(delta / SAMPLE_SECONDS) - 1 for delta in deltas)
    # The actual instants must also keep the cadence on their own.
    wide = sum(1 for earlier, later in zip(times, times[1:])
               if (later - earlier).total_seconds() > SAMPLE_SECONDS + SAMPLE_JITTER_SECONDS)
    if misses or late or wide:
        findings.append(f"cadence not met: {misses} missed {SAMPLE_SECONDS}s observations, {late} late samples, "
                        f"{wide} gaps over {SAMPLE_SECONDS + SAMPLE_JITTER_SECONDS}s")
    not_off = [index for index, sample in enumerate(samples) if sample.get("a_power") != "PowerState/deallocated"]
    if not_off:
        findings.append(f"client A not deallocated in {len(not_off)} samples (first at index {not_off[0]})")
    def identity(sample: Dict[str, Any], field: str) -> Optional[str]:
        value = sample.get(field)
        return value.casefold() if isinstance(value, str) and value.strip() else None

    identities = {(identity(sample, "b_group_id"), identity(sample, "b_vm_id"), identity(sample, "b_instance_id"))
                  for sample in samples}
    if len(identities) != 1 or any(None in pair for pair in identities):
        findings.append("worker B identity changed or was unreadable during the interval")
    b_states = {sample.get("b_power") for sample in samples}
    if b_states != {"PowerState/running"}:
        findings.append(f"worker B power states during the interval: {sorted(str(s) for s in b_states)}")
    counters = [sample.get("progress") for sample in samples]
    if any(type(counter) is not int for counter in counters):  # noqa: E721
        findings.append("progress counter unreadable or not an integer in at least one sample")
    elif any(later < earlier for earlier, later in zip(counters, counters[1:])):
        findings.append("progress counter went backwards (task replay or a different task)")
    elif any(later <= earlier for earlier, later in zip(counters, counters[1:])):
        stalls = sum(1 for earlier, later in zip(counters, counters[1:]) if later <= earlier)
        findings.append(f"progress counter did not advance between {stalls} consecutive samples")
    hosts = {sample.get("b_host") for sample in samples}
    if len(hosts) != 1 or not all(routable(host) for host in hosts):
        findings.append("worker B endpoint changed, was unreadable or was not a public address during the interval")
    if expected_image_ref is not None:
        wanted = image_ref_digest(expected_image_ref)
        if any(sample.get("b_image_ref") != wanted for sample in samples):
            findings.append("worker B did not carry the manifest image's reference tag in every sample")
    checkpoints = [sample.get("checkpoint") for sample in samples]
    checkpoint_proof = (all(type(value) is int for value in checkpoints) and checkpoints[-1] > checkpoints[0]  # noqa: E721
                        and all(later >= earlier for earlier, later in zip(checkpoints, checkpoints[1:])))
    return {
        "passed": not findings,
        "findings": findings,
        "samples": len(samples),
        "observed_seconds": span,
        "missed_samples": misses,
        "late_samples": late,
        "counter_progress": counters[0] if counters and counters[0] is not None else None,
        "counter_progress_end": counters[-1] if counters and counters[-1] is not None else None,
        # Recorded separately: counter progress is never labelled checkpoint evidence.
        "worker_checkpoint_progress": checkpoint_proof,
    }


def baseline_problems(baseline: Any, manifest: Dict[str, Any]) -> List[str]:
    """The header's identity must be an Azure worker in the manifest's worker group:
    ARM group and VM ID paths under the manifest subscription, a VM instance identity,
    a routable address. Stable but meaningless strings never identify a worker."""
    if not isinstance(baseline, dict):
        return ["baseline is not an object"]
    problems = []
    group = f"/subscriptions/{manifest['subscription_id']}/resourceGroups/{manifest['worker_group']}"
    if not same_id(baseline.get("b_group_id"), group):
        problems.append("baseline group ID is not the manifest worker group under the manifest subscription")
    if not same_id(baseline.get("b_vm_id"), f"{group}/providers/Microsoft.Compute/virtualMachines/worker"):
        problems.append("baseline VM ID is not the worker VM in that group")
    if not isinstance(baseline.get("b_instance_id"), str) or not INSTANCE_ID_RE.fullmatch(baseline["b_instance_id"]):
        problems.append("baseline instance identity is not a VM instance identity")
    if not routable(baseline.get("b_host")):
        problems.append("baseline address is not a public IP address")
    return problems


def verdict_from_records(records: List[Any], manifest: Dict[str, Any]) -> Dict[str, Any]:
    """Offline verdict over a journal: the first record must be the baseline header the
    off phase wrote, naming an Azure worker in the manifest's group, so a journal from
    another worker, or a fabricated one, can never pass."""
    if not records or not isinstance(records[0], dict) or not isinstance(records[0].get("baseline"), dict):
        return {"passed": False, "findings": ["journal has no baseline header; not produced by the off phase"]}
    header, samples = records[0], records[1:]
    problems = baseline_problems(header["baseline"], manifest)
    if problems:
        return {"passed": False, "findings": [f"baseline: {problem}" for problem in problems]}
    if header.get("worker_image") != manifest["worker_image"]:
        return {"passed": False, "findings": ["journal was recorded for a different worker image"]}
    if header.get("observed_image_ref") != image_ref_digest(manifest["worker_image"]):
        return {"passed": False, "findings": ["baseline observation did not carry the manifest image's reference tag"]}
    return evaluate_samples(samples, manifest["off_minutes"], manifest["lease_seconds"], manifest["worker_image"],
                            header["baseline"])
