"""Manifest schema, constants and the small pure helpers every other module shares."""
from __future__ import annotations

import datetime as _dt
import hashlib
import ipaddress
import re
from typing import Any, Dict, List, Optional

TOOL_VERSION = "0.1.0"
ARM = "https://management.azure.com"
CLI_STEP_SECONDS = 90
SAMPLE_SECONDS = 15
SAMPLE_JITTER_SECONDS = 5
MIN_OFF_MINUTES = 10
REAPER_TAGS = {"purpose": "horizon-azure-vm-spike"}
CLIENT_VM_NAME = "client"
PUBLIC_IP_NAME = "worker-pip"
WORKER_CONTAINER = "horizon-worker"
RUN_COMMAND_SECONDS = 600
UUID_RE = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
GROUP_RE = re.compile(r"^[A-Za-z0-9_.()-]{1,90}$")
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
DIGEST_RE = re.compile(r"^[a-z0-9]+(?:[.-][a-z0-9]+)+(?::[0-9]{1,5})?/[a-z0-9]+(?:[._-][a-z0-9]+)*(?:/[a-z0-9]+(?:[._-][a-z0-9]+)*)*@sha256:[0-9a-f]{64}$")
MANIFEST_FIELDS = ("subscription_id", "location", "client_group", "client_vm_size", "client_sha",
                   "client_binary_sha256", "worker_group", "worker_image", "hourly_cost_micros", "budget_micros",
                   "cleanup_deadline_utc", "off_minutes", "lease_seconds")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
VM_SIZE_RE = re.compile(r"^Standard_[A-Za-z0-9_-]{2,40}$")
# Plain components only: no `.` or `..`, so two spellings can never alias one file.
PROGRESS_PATH_RE = re.compile(r"^/workspace(?:/(?!\.\.?(?:/|$))[A-Za-z0-9_.-]+)+$")
PROGRESS_READ_LIMIT = 256
TAG_IMAGE_REF = "horizon-worker-image-ref-sha256"
HOST_KEY_RE = re.compile(r"^ssh-ed25519 [A-Za-z0-9+/]{68}$")
RUN_ID_RE = re.compile(r"[0-9a-f]{32}")
INSTANCE_ID_RE = re.compile(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}")


def utc_now() -> _dt.datetime:
    return _dt.datetime.now(_dt.timezone.utc)


def parse_utc(value: str) -> _dt.datetime:
    """Parse an ISO-8601 instant; a value without an offset is rejected, never guessed."""
    parsed = _dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        raise ValueError("instant has no UTC offset")
    return parsed.astimezone(_dt.timezone.utc)


def same_group(left: Any, right: Any) -> bool:
    """Azure resource-group names are case-insensitive; compare them that way."""
    return isinstance(left, str) and isinstance(right, str) and left.casefold() == right.casefold()


# Time the manifest deadline must still cover when a phase starts: provisioning runs
# under a 30-minute bound, the off interval is declared, and return plus cleanup need
# a margin. A deadline that the reaper could reach mid-run is not runnable.
PROVISION_MINUTES = 30
CLEANUP_MARGIN_MINUTES = 30
RETURN_MARGIN_MINUTES = 15


def required_minutes(manifest: Dict[str, Any], phase: str) -> int:
    """How many minutes past `now` the deadline must lie for `phase` to start."""
    off = manifest.get("off_minutes") if type(manifest.get("off_minutes")) is int else MIN_OFF_MINUTES  # noqa: E721
    return {"validate": PROVISION_MINUTES + off + CLEANUP_MARGIN_MINUTES, "off": off + CLEANUP_MARGIN_MINUTES,
            "return": RETURN_MARGIN_MINUTES}.get(phase, 0)


def validate_manifest(manifest: Dict[str, Any], now: Optional[_dt.datetime] = None, renting: bool = True,
                      phase: str = "validate") -> List[str]:
    """Problems that must be fixed before anything is rented; empty means runnable.

    With `renting=False` (verdict and cleanup) the deadline must still be a valid
    instant but may lie in the past: a late cleanup is exactly the case that must run.
    """
    now = now or utc_now()
    if not isinstance(manifest, dict):
        return ["manifest is not a JSON object"]
    problems = [f"missing {field}" for field in MANIFEST_FIELDS if field not in manifest]
    if problems:
        return problems
    def text(field: str, pattern: "re.Pattern[str]", what: str, search: bool = False) -> None:
        value = manifest[field]
        matched = isinstance(value, str) and (pattern.search(value) if search else pattern.fullmatch(value))
        if not matched:
            problems.append(f"{field} is not {what}")

    text("subscription_id", UUID_RE, "an exact UUID")
    text("client_vm_size", VM_SIZE_RE, "an Azure VM size name")
    text("client_group", GROUP_RE, "a valid resource group name")
    text("worker_group", GROUP_RE, "a valid resource group name")
    if same_group(manifest["client_group"], manifest["worker_group"]):
        problems.append("client and worker must live in different exact groups")
    text("client_sha", SHA_RE, "a full commit SHA")
    text("client_binary_sha256", SHA256_RE, "a SHA-256 digest")
    text("worker_image", DIGEST_RE, "a complete registry/repository@sha256 digest reference")
    if not isinstance(manifest["location"], str) or not manifest["location"]:
        problems.append("location is not a region name")
    for field in ("hourly_cost_micros", "budget_micros"):
        # bool is an int subclass in JSON decoding; only an exact integer counts.
        if type(manifest[field]) is not int or manifest[field] <= 0:  # noqa: E721
            problems.append(f"{field} must be a positive integer")
    try:
        deadline = parse_utc(str(manifest["cleanup_deadline_utc"]))
    except ValueError:
        problems.append("cleanup_deadline_utc is not an ISO-8601 instant")
    else:
        needed = _dt.timedelta(minutes=required_minutes(manifest, phase)) if renting else _dt.timedelta(0)
        if renting and deadline <= now:
            problems.append("cleanup_deadline_utc is not in the future")
        elif renting and deadline - now < needed:
            problems.append(f"cleanup_deadline_utc must lie at least {int(needed.total_seconds() // 60)} minutes "
                            f"ahead for the {phase} phase (provisioning bound, off interval and margins)")
        elif deadline - now > _dt.timedelta(hours=24):
            problems.append("cleanup_deadline_utc is more than 24 hours away")
    if type(manifest["off_minutes"]) is not int or manifest["off_minutes"] < MIN_OFF_MINUTES:  # noqa: E721
        problems.append(f"off_minutes must be at least {MIN_OFF_MINUTES}")
    if type(manifest["lease_seconds"]) is not int or manifest["lease_seconds"] < 0:  # noqa: E721
        problems.append("lease_seconds must be a non-negative integer (0 when no lease applies)")
    elif type(manifest["off_minutes"]) is int and manifest["off_minutes"] * 60 <= manifest["lease_seconds"]:  # noqa: E721
        problems.append("off_minutes must exceed lease_seconds so the interval crosses the lease boundary")
    return problems


def image_ref_digest(image: str) -> str:
    """The adapter tags each worker with the SHA-256 of its full image reference."""
    return hashlib.sha256(image.encode()).hexdigest()


def routable(address: Any) -> bool:
    """Only a public, globally routable IP address can be worker B's endpoint."""
    if not isinstance(address, str):
        return False
    try:
        parsed = ipaddress.ip_address(address)
    except ValueError:
        return False
    return parsed.is_global and not parsed.is_multicast


def same_id(left: Any, right: Any) -> bool:
    """ARM resource IDs compare case-insensitively; addresses compare exactly."""
    return isinstance(left, str) and isinstance(right, str) and left.casefold() == right.casefold()
