#!/usr/bin/env python3
"""Read-only Azure readiness preflight for persistent CPU workspace candidates (#474).

Plans a fixed allowlist of documented read-only Azure CLI operations, runs them without a
shell under bounded time and output, and interprets the responses into a redacted report.
Nothing here logs in, registers a provider, changes CLI defaults or touches resource
lifecycle, and a successful read never claims readiness; see UNVERIFIED_GATES.
"""
from __future__ import annotations

import argparse
import contextlib
import dataclasses
import datetime as _dt
import json
import os
import re
import selectors
import shutil
import signal
import subprocess
import sys
import time
from typing import Any, Callable, Dict, List, Optional

TOOL_VERSION = "0.1.0"
REPORT_SCHEMA = "horizon.azure-workspace-preflight.report"
REPORT_SCHEMA_VERSION = 1
ARM = "https://management.azure.com"
API_LOCATIONS = "2022-12-01"
API_ACI = "2026-07-01"
API_APP = "2025-07-01"
DEFAULT_TIMEOUT_SECONDS = 150  # az vm list-skus filters client-side and needs about a minute
MAX_TIMEOUT_SECONDS = 600
OUTPUT_LIMIT_BYTES = 4 * 1024 * 1024

UUID_RE = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
REGION_RE = re.compile(r"^[a-z][a-z0-9]{1,63}$")
VM_SIZE_RE = re.compile(r"^Standard_[A-Za-z0-9_-]{1,40}$")
ERROR_CODE_RE = re.compile(r"^[A-Za-z0-9_.-]{1,80}$")
# The only ARM read endpoints az rest may target; the planner and the allowlist share this definition.
REST_URL_RE = re.compile(re.escape(ARM) + r"/subscriptions/[0-9a-f-]{36}(?:/locations|/providers/Microsoft\."
                         r"(?:ContainerInstance/locations/[a-z0-9]+/(?:usages|capabilities)|App/locations/[a-z0-9]+/usages))"
                         r"\?api-version=\d{4}-\d{2}-\d{2}$")

# Operations the planner may emit. Anything else is a programming error.
ALLOWED_OPERATIONS = (("account", "show"), ("provider", "show"), ("vm", "list-usage"), ("vm", "list-skus"), ("rest",))
FORBIDDEN_TOKENS = frozenset({"create", "delete", "start", "stop", "restart", "register", "unregister", "login", "logout",
                              "set", "extension", "configure", "deployment", "update", "exec", "attach", "run-command"})
_IDENTITY = ["Microsoft.ContainerRegistry", "Microsoft.ManagedIdentity"]
CANDIDATES: Dict[str, Dict[str, Any]] = {
    "aci": {"name": "Azure Container Instances container group (not selected 2026-09-10)",
            "required": ["Microsoft.ContainerInstance"],
            "supporting": _IDENTITY + ["Microsoft.Storage", "Microsoft.Network"]},
    "vm": {"name": "Azure Linux VM with managed disk (revalidated candidate 2026-09-10; acceptance open)",
           "required": ["Microsoft.Compute", "Microsoft.Network"], "supporting": _IDENTITY + ["Microsoft.DevTestLab"]},
    "container-apps": {"name": "Azure Container Apps long-running app (not selected 2026-09-10; Jobs excluded)",
                       "required": ["Microsoft.App", "Microsoft.Network"],
                       "supporting": _IDENTITY + ["Microsoft.Storage", "Microsoft.OperationalInsights"]},
}
# Structured ARM/CLI error codes take precedence over free-text matching.
_AUTH, _PERM = ("blocked", "authentication_required"), ("blocked", "insufficient_permission")
_SUB, _UNSUPPORTED = ("blocked", "subscription_not_visible"), ("unknown", "unsupported_query")
ERROR_CODES = {
    "AuthorizationFailed": _PERM, "InvalidAuthenticationToken": _AUTH, "InvalidAuthenticationTokenTenant": _AUTH,
    "ExpiredAuthenticationToken": _AUTH, "SubscriptionNotFound": _SUB, "InvalidSubscriptionId": _SUB,
    "MissingSubscriptionRegistration": ("blocked", "unregistered_provider"), "TooManyRequests": ("unknown", "throttled"),
    "NoRegisteredProviderFound": _UNSUPPORTED, "InvalidApiVersionParameter": _UNSUPPORTED,
    "InvalidResourceType": _UNSUPPORTED, "ResourceNotFound": _UNSUPPORTED,
    "LocationNotAvailableForResourceType": ("blocked", "region_unavailable"),
}
UNVERIFIED_GATES = [
    "create permission for the candidate resource type (a successful read is not a write grant)",
    "actual regional capacity at creation time (quota headroom is not capacity; Container Apps core quota is per environment)",
    "direct key-only SSH reachability and host-key pinning on a real worker",
    "compact worker image pull through a user-assigned managed identity without registry passwords",
    "on-worker storage qualification: healthy journaled ext4 accepted by the Rust qualifier, not a disk SKU or share",
    "measured create, pull, endpoint, SSH-ready and delete timing against the 180-second boundary",
    "independent sessions continuing while Horizon is closed and the PC is off",
    "explicit Stop with retained data, restart/replacement behavior and exact deletion proof",
    "no Container Apps Job is created anywhere in this lane",
]
ENV_PASSTHROUGH = ("PATH", "HOME", "AZURE_CONFIG_DIR", "LANG", "LC_ALL", "TMPDIR", "TERM",
                   "SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", "HTTPS_PROXY", "HTTP_PROXY", "NO_PROXY")


class InputError(ValueError):
    """Rejected before any Azure command is planned or executed."""  # exit code 3


@dataclasses.dataclass(frozen=True)
class Request:
    candidate: str
    subscription: str
    region: str
    mode: str  # plan | fixture | live
    cpu_cores: int
    memory_gb: int
    vm_size: Optional[str]
    timeout_seconds: int
    az_path: str


@dataclasses.dataclass(frozen=True)
class PlannedCheck:
    id: str
    title: str
    role: str  # required | supporting
    argv: List[str]
    interpret: "Callable[[Any, Dict[str, Outcome]], Outcome]"
    depends_on: tuple = ()


@dataclasses.dataclass(frozen=True)
class CommandResult:
    exit_code: Optional[int]
    stdout: str
    stderr: str
    timed_out: bool = False
    oversized: bool = False
    executed: bool = True
    launch_failed: bool = False


@dataclasses.dataclass(frozen=True)
class Outcome:
    outcome: str  # observed_ok | blocked | unknown | planned
    reason: Optional[str] = None
    details: Dict[str, Any] = dataclasses.field(default_factory=dict)


Executor = Callable[[List[str], int], CommandResult]


def validate_request(args: argparse.Namespace) -> Request:
    candidate = args.candidate
    subscription = (args.subscription or "").strip().lower()
    if not UUID_RE.match(subscription):
        raise InputError("--subscription must be the exact subscription UUID, not a name or default")
    region = (args.region or "").strip()
    if not REGION_RE.match(region):
        raise InputError("--region must be a lowercase Azure region name such as northeurope")
    if args.live and args.fixture is not None:
        raise InputError("--live and --fixture are mutually exclusive")
    if args.fixture is not None and not args.fixture:
        raise InputError("--fixture requires a path")
    mode = "live" if args.live else ("fixture" if args.fixture is not None else "plan")
    if not (1 <= args.cpu_cores <= 31 and 1 <= args.memory_gb <= 240):
        raise InputError("--cpu-cores must be between 1 and 31 and --memory-gb between 1 and 240")
    vm_size = args.vm_size
    if candidate == "vm":
        if not vm_size or not VM_SIZE_RE.match(vm_size):
            raise InputError("--vm-size is required for the vm candidate, for example Standard_D4s_v3")
    elif vm_size:
        raise InputError("--vm-size applies only to the vm candidate")
    if not 1 <= args.timeout_seconds <= MAX_TIMEOUT_SECONDS:
        raise InputError(f"--timeout-seconds must be between 1 and {MAX_TIMEOUT_SECONDS}")
    az_path = args.az_path or "az"
    if mode == "live":
        if os.name != "posix":
            raise InputError("--live needs a POSIX host (Linux, macOS or WSL) for process-group timeouts; "
                             "use plan or --fixture here")
        resolved = shutil.which(az_path)
        if not resolved:
            raise InputError(f"Azure CLI executable {az_path!r} was not found; install it or pass --az-path")
        az_path = resolved
    return Request(candidate, subscription, region, mode, args.cpu_cores, args.memory_gb, vm_size,
                   args.timeout_seconds, az_path)


def _az(request: Request, *tokens: str) -> List[str]:
    argv = [request.az_path, *tokens, "--output", "json", "--only-show-errors"]
    assert_allowlisted(argv)
    return argv


def assert_allowlisted(argv: List[str]) -> None:
    tokens = tuple(argv[1:])
    command = tuple(token for token in tokens if not token.startswith("-"))[:2]
    if not any(command[: len(op)] == op for op in ALLOWED_OPERATIONS):
        raise AssertionError(f"operation not in read-only allowlist: {command}")
    if FORBIDDEN_TOKENS.intersection(command):
        raise AssertionError("forbidden lifecycle token in planned command")
    if tokens[0] == "rest":
        try:
            rest_ok = tokens[tokens.index("--method") + 1] == "get" and REST_URL_RE.match(tokens[tokens.index("--url") + 1])
        except (ValueError, IndexError):
            rest_ok = False
        if not rest_ok:
            raise AssertionError("az rest must be a pinned GET against an allowlisted ARM read endpoint")


def _rest(request: Request, path: str, api_version: str, query: str) -> List[str]:
    url = f"{ARM}/subscriptions/{request.subscription}{path}?api-version={api_version}"
    return _az(request, "rest", "--method", "get", "--url", url, "--query", query)


def plan_checks(request: Request) -> List[PlannedCheck]:
    sub = ("--subscription", request.subscription)
    account, region = "account_context", "region_available"
    checks = [
        PlannedCheck(account, "Authenticated CLI context resolves the requested subscription", "required",
                     _az(request, "account", "show", *sub), lambda payload, _: interpret_account(payload, request)),
        PlannedCheck(region, "Requested region is a physical region of the subscription", "required",
                     _rest(request, "/locations", API_LOCATIONS,
                           "value[].{name:name,type:type,regionType:metadata.regionType}"),
                     lambda payload, _: interpret_locations(payload, request), (account,)),
    ]
    spec = CANDIDATES[request.candidate]
    for namespace in spec["required"] + spec["supporting"]:
        role = "required" if namespace in spec["required"] else "supporting"
        checks.append(PlannedCheck(
            f"provider_{namespace.split('.', 1)[1].lower()}", f"Resource provider {namespace} registration", role,
            _az(request, "provider", "show", "--namespace", namespace, *sub,
                "--query", "{namespace:namespace,registrationState:registrationState}"),
            lambda payload, _, ns=namespace: interpret_provider(payload, ns), (account,)))
    loc, regional = f"/locations/{request.region}", (account, region, f"provider_{spec['required'][0].split('.', 1)[1].lower()}")
    if request.candidate == "aci":
        checks.append(PlannedCheck(
            "aci_regional_quota", "Container Instances regional usage versus quota", "required",
            _rest(request, f"/providers/Microsoft.ContainerInstance{loc}/usages", API_ACI, "value"),
            lambda payload, _: interpret_usage(payload, [("ContainerGroups", 1), ("StandardCores", request.cpu_cores)]),
            regional))
        checks.append(PlannedCheck(
            "aci_regional_capabilities", "Container Instances Linux public-IP capability", "required",
            _rest(request, f"/providers/Microsoft.ContainerInstance{loc}/capabilities", API_ACI, "value"),
            lambda payload, _: interpret_aci_capabilities(payload, request), regional))
    elif request.candidate == "vm":
        checks.append(PlannedCheck(
            "vm_sku_availability", f"Compute SKU {request.vm_size} availability in region", "required",
            _az(request, "vm", "list-skus", "--location", request.region, "--size", request.vm_size or "",
                "--resource-type", "virtualMachines", "--all", *sub),
            lambda payload, _: interpret_vm_sku(payload, request), regional))
        checks.append(PlannedCheck(
            "vm_regional_quota", "Compute regional vCPU and family usage versus quota", "required",
            _az(request, "vm", "list-usage", "--location", request.region, *sub),
            interpret_vm_usage, (*regional, "vm_sku_availability")))
    else:
        checks.append(PlannedCheck(
            "container_apps_regional_quota", "Container Apps regional usage versus quota", "required",
            _rest(request, f"/providers/Microsoft.App{loc}/usages", API_APP, "value"),
            lambda payload, _: interpret_usage(payload, [("ManagedEnvironmentCount", 1)]), regional))
    return checks


def subprocess_executor(argv: List[str], timeout_seconds: int, limit: int = OUTPUT_LIMIT_BYTES) -> CommandResult:
    """Run argv without a shell in its own POSIX process group; kill the group on timeout or once a stream passes limit."""
    env = {key: os.environ[key] for key in ENV_PASSTHROUGH if key in os.environ}
    env.update({"AZURE_CORE_NO_COLOR": "true", "AZURE_CORE_DISABLE_PROGRESS_BAR": "true",
                "AZURE_EXTENSION_USE_DYNAMIC_INSTALL": "no", "AZURE_CORE_COLLECT_TELEMETRY": "false"})
    try:
        proc = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                env=env, shell=False, start_new_session=True)
    except OSError:
        return CommandResult(None, "", "", launch_failed=True)
    buffers = {proc.stdout.fileno(): bytearray(), proc.stderr.fileno(): bytearray()}
    deadline, timed_out, oversized = time.monotonic() + timeout_seconds, False, False
    with selectors.DefaultSelector() as selector:
        for fd in buffers:
            selector.register(fd, selectors.EVENT_READ)
        while selector.get_map() and not (timed_out or oversized):
            ready = selector.select(timeout=max(0.0, deadline - time.monotonic()))
            timed_out = not ready and time.monotonic() >= deadline
            for key, _ in ready:
                chunk = os.read(key.fd, 65536)
                buffers[key.fd].extend(chunk) if chunk else selector.unregister(key.fd)
                oversized = oversized or len(buffers[key.fd]) > limit
    if timed_out or oversized:
        with contextlib.suppress(ProcessLookupError):  # the group may exit between the check and the kill
            os.killpg(proc.pid, signal.SIGKILL)  # the installed az is a shell wrapper; kill its children too
    proc.communicate()  # reap; after a kill the remaining pipe contents are bounded and discarded
    stdout, stderr = (bytes(buffers[fd][:limit]).decode("utf-8", "replace") for fd in buffers)
    return CommandResult(None if (timed_out or oversized) else proc.returncode, stdout, stderr, timed_out, oversized)


def fixture_executor(fixture: Dict[str, Any], plan: List[PlannedCheck]) -> Executor:
    by_argv = {tuple(check.argv): check.id for check in plan}

    def run(argv: List[str], _timeout: int) -> CommandResult:
        entry = fixture.get(by_argv[tuple(argv)]) if tuple(argv) in by_argv else None
        if not isinstance(entry, dict):
            return CommandResult(None, "", "", executed=False)
        stdout = entry.get("stdout", "")
        stdout = stdout if isinstance(stdout, str) else json.dumps(stdout)
        return CommandResult(entry.get("exit_code", 0), stdout, str(entry.get("stderr", "")),
                             timed_out=bool(entry.get("timed_out")), oversized=bool(entry.get("oversized")))

    return run


_FAILURE_PATTERNS = [
    ("blocked", "authentication_required", r"az login|AADSTS|Interactive authentication|Not logged in"),
    ("blocked", "subscription_not_visible", r"doesn't exist in cloud"), ("blocked", "insufficient_permission", r"Forbidden"),
    ("unknown", "unsupported_query", r"unrecognized arguments|Command not found|not a valid value|NotFound")]


def _error_code(stderr: str) -> Optional[str]:
    match = (re.search(r'"code"\s*:\s*"([^"]{1,80})"', stderr)
             or re.search(r"^ERROR: \(([A-Za-z][A-Za-z0-9_.-]{2,79})\)", stderr, re.MULTILINE))
    code = match.group(1) if match else None
    return code if code and ERROR_CODE_RE.match(code) else None


def classify_failure(result: CommandResult) -> Outcome:
    if not result.executed:
        return Outcome("unknown", "not_executed")
    if result.timed_out:
        return Outcome("unknown", "timeout")
    if result.oversized:
        return Outcome("unknown", "oversized_output")
    if result.launch_failed:
        return Outcome("unknown", "launch_failed")
    code = _error_code(result.stderr)
    details = {"error_code": code}
    if code in ERROR_CODES:
        return Outcome(*ERROR_CODES[code], details)
    for outcome, reason, pattern in _FAILURE_PATTERNS:
        if re.search(pattern, result.stderr):
            return Outcome(outcome, reason, details)
    return Outcome("unknown", "command_failed", details)


def _number(value: Any) -> Optional[float]:
    numeric = isinstance(value, (int, float)) and not isinstance(value, bool)  # az vm list-usage emits numeric strings
    return float(value) if numeric or (isinstance(value, str) and re.fullmatch(r"-?\d+(?:\.\d+)?", value)) else None


def _usage_entries(payload: Any) -> Optional[Dict[str, Dict[str, float]]]:
    if not isinstance(payload, list):
        return None
    entries: Dict[str, Dict[str, float]] = {}
    for item in payload:
        if not isinstance(item, dict) or not isinstance(item.get("name"), dict):
            return None
        name, current, limit = item["name"].get("value"), _number(item.get("currentValue")), _number(item.get("limit"))
        if not isinstance(name, str) or current is None or limit is None:
            return None
        entries[name.lower()] = {"current": current, "limit": limit}
    return entries


def interpret_usage(payload: Any, requirements: List[tuple]) -> Outcome:
    """Headroom per required entry; blocked beats unknown, first reason wins. Headroom is not capacity."""
    entries = _usage_entries(payload)
    if entries is None:
        return Outcome("unknown", "malformed_response")
    details: Dict[str, Any] = {"capacity": "unverified"}
    problems = []
    for name, needed in requirements:
        entry = entries.get(name.lower())
        if entry is None:
            details.setdefault("missing_quota_entries", []).append(name)
            problems.append(("unknown", "quota_entry_missing"))
            continue
        headroom = "unlimited" if entry["limit"] < 0 else entry["limit"] - entry["current"]
        details[name] = {**entry, "headroom": headroom}
        if headroom != "unlimited" and headroom < 0:
            problems.append(("unknown", "contradictory_response"))
        elif headroom != "unlimited" and headroom < needed:
            problems.append(("blocked", "quota_exhausted"))
    worst = next((p for p in problems if p[0] == "blocked"), problems[0] if problems else ("observed_ok", None))
    return Outcome(worst[0], worst[1], details)


def interpret_account(payload: Any, request: Request) -> Outcome:
    if not isinstance(payload, dict) or not isinstance(payload.get("id"), str):
        return Outcome("unknown", "malformed_response")
    user = payload.get("user") if isinstance(payload.get("user"), dict) else {}
    details = {"state": payload.get("state"), "principal_type": user.get("type"), "tenant": "<redacted>"}
    if payload["id"].lower() != request.subscription:
        return Outcome("blocked", "subscription_mismatch", details)
    if payload.get("state") != "Enabled":
        return Outcome("blocked", "subscription_not_enabled", details)
    return Outcome("observed_ok", None, details)


def interpret_locations(payload: Any, request: Request) -> Outcome:
    if not isinstance(payload, list) or not payload:
        return Outcome("unknown", "malformed_response")
    for item in payload:
        if not isinstance(item, dict) or not isinstance(item.get("name"), str):
            return Outcome("unknown", "malformed_response")
        if item["name"].lower() == request.region:
            details = {"type": item.get("type"), "region_type": item.get("regionType")}
            if (item.get("type"), item.get("regionType")) == ("Region", "Physical"):
                return Outcome("observed_ok", None, details)
            return Outcome("blocked", "region_unavailable", details)
    return Outcome("blocked", "region_unavailable", {"listed_regions": len(payload)})


def interpret_provider(payload: Any, namespace: str) -> Outcome:
    if not isinstance(payload, dict) or not isinstance(payload.get("registrationState"), str):
        return Outcome("unknown", "malformed_response")
    if str(payload.get("namespace", "")).lower() != namespace.lower():
        return Outcome("unknown", "contradictory_response")
    state = payload["registrationState"]
    details = {"registration_state": state}
    if state == "Registered":
        return Outcome("observed_ok", None, details)
    if state in ("NotRegistered", "Unregistered"):
        return Outcome("blocked", "unregistered_provider", {**details, "note": "registration needs separate approval"})
    if state in ("Registering", "Unregistering"):
        return Outcome("unknown", "registration_in_progress", details)
    return Outcome("unknown", "malformed_response", details)


def interpret_aci_capabilities(payload: Any, request: Request) -> Outcome:
    if not isinstance(payload, list):
        return Outcome("unknown", "malformed_response")
    details: Dict[str, Any] = {"linux_public_entries": 0, "capacity": "unverified"}
    for item in payload:
        if not isinstance(item, dict):
            return Outcome("unknown", "malformed_response")
        caps = item.get("capabilities") if isinstance(item.get("capabilities"), dict) else {}
        if (str(item.get("osType", "")).lower(), str(item.get("ipAddressType", "")).lower(),
                str(item.get("gpu", "None")).lower()) != ("linux", "public", "none"):
            continue
        max_cpu, max_mem = _number(caps.get("maxCpu")), _number(caps.get("maxMemoryInGB"))
        if max_cpu is None or max_mem is None:
            return Outcome("unknown", "malformed_response")
        details["linux_public_entries"] += 1
        if max_cpu >= request.cpu_cores and max_mem >= request.memory_gb:
            return Outcome("observed_ok", None, {**details, "max_cpu": max_cpu, "max_memory_gb": max_mem})
    reason = "request_exceeds_regional_maximum" if details["linux_public_entries"] else "no_linux_public_capability"
    return Outcome("blocked", reason, details)


def interpret_vm_sku(payload: Any, request: Request) -> Outcome:
    if not isinstance(payload, list):
        return Outcome("unknown", "malformed_response")
    for item in payload:
        if (not isinstance(item, dict) or item.get("resourceType") != "virtualMachines"
                or str(item.get("name", "")).lower() != str(request.vm_size).lower()):
            continue
        capabilities, restrictions, family = item.get("capabilities"), item.get("restrictions") or [], item.get("family")
        if not isinstance(capabilities, list) or not isinstance(restrictions, list):
            return Outcome("unknown", "malformed_response")
        caps = {c.get("name"): c.get("value") for c in capabilities if isinstance(c, dict)}
        vcpus = str(caps.get("vCPUs", "")).strip()
        if not isinstance(family, str) or not vcpus.isdigit() or int(vcpus) == 0:
            return Outcome("unknown", "malformed_response")
        details: Dict[str, Any] = {"family": family, "vcpus": int(vcpus), "capacity": "unverified"}
        for restriction in restrictions:
            if isinstance(restriction, dict) and restriction.get("type") == "Location":
                return Outcome("blocked", "sku_restricted", {**details, "reason_code": restriction.get("reasonCode")})
        zones = [r.get("reasonCode") for r in restrictions if isinstance(r, dict) and r.get("type") == "Zone"]
        if zones:
            details["zone_restrictions"] = zones
        return Outcome("observed_ok", None, details)
    return Outcome("blocked", "sku_unavailable_in_region", {"listed_skus": len(payload)})


def interpret_vm_usage(payload: Any, outcomes: Dict[str, Outcome]) -> Outcome:
    sku = outcomes["vm_sku_availability"].details
    return interpret_usage(payload, [("cores", sku["vcpus"]), (sku["family"], sku["vcpus"])])


def evaluate(check: PlannedCheck, result: CommandResult, outcomes: Dict[str, Outcome]) -> Outcome:
    if not result.executed or result.launch_failed or result.timed_out or result.oversized or result.exit_code != 0:
        return classify_failure(result)
    try:
        return check.interpret(json.loads(result.stdout), outcomes)
    except Exception:  # noqa: BLE001 - any unexpected payload shape fails closed as missing evidence
        return Outcome("unknown", "malformed_response")


def redact(value: Any, subscription: str) -> Any:
    if isinstance(value, dict):
        return {redact(k, subscription): redact(v, subscription) for k, v in value.items()}
    if isinstance(value, list):
        return [redact(v, subscription) for v in value]
    if not isinstance(value, str):
        return value
    value = value if len(value) <= 400 else value[:400] + "<truncated>"
    text = re.sub(re.escape(subscription), "<subscription>", value, flags=re.IGNORECASE) if subscription else value
    text = re.sub(r"eyJ[A-Za-z0-9_-]{8,}(?:\.[A-Za-z0-9_-]+){0,2}", "<token>", text)
    text = re.sub(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}", "<uuid>", text)
    text = re.sub(r"[\w.+-]+@[\w-]+(?:\.[\w-]+)+", "<email>", text)
    return re.sub(r"/resource[Gg]roups/[^/\s\"']+", "/resourceGroups/<redacted>", text)


def run_preflight(request: Request, executor: Optional[Executor]) -> Dict[str, Any]:
    plan = plan_checks(request)
    planning = request.mode == "plan"
    if planning != (executor is None):
        raise InputError("plan mode takes no executor; fixture and live modes require one")
    outcomes: Dict[str, Outcome] = {}
    for check in plan:
        if planning:
            outcomes[check.id] = Outcome("planned")
        elif any(outcomes[dep].outcome != "observed_ok" for dep in check.depends_on):
            outcomes[check.id] = Outcome("unknown", "prerequisite_failed",
                                         {"depends_on": [d for d in check.depends_on if outcomes[d].outcome != "observed_ok"]})
        else:
            outcomes[check.id] = evaluate(check, executor(check.argv, request.timeout_seconds), outcomes)
    states = {o.outcome for o in outcomes.values()}
    status = ("planned" if planning else "blocked" if "blocked" in states
              else "unknown" if "unknown" in states else "no_blockers_observed")
    report = {
        "schema": REPORT_SCHEMA, "schema_version": REPORT_SCHEMA_VERSION, "tool_version": TOOL_VERSION,
        "mode": request.mode,
        "observed_at": None if planning else _dt.datetime.now(_dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "candidate": {"id": request.candidate, "name": CANDIDATES[request.candidate]["name"],
                      "vm_size": request.vm_size, "cpu_cores": request.cpu_cores, "memory_gb": request.memory_gb},
        "region": request.region, "subscription": "<subscription>", "status": status,
        "checks": [{"id": c.id, "title": c.title, "role": c.role, "operation": " ".join(c.argv[1:]),
                    **dataclasses.asdict(outcomes[c.id])} for c in plan],
        "blockers": [f"{c.id}: {outcomes[c.id].reason}" for c in plan if outcomes[c.id].outcome == "blocked"],
        "unverified_gates": list(UNVERIFIED_GATES),
        "claims": "Read-only observation only. This report never asserts readiness, capacity or acceptance.",
    }
    return redact(report, request.subscription)


def render_text(report: Dict[str, Any]) -> str:
    lines = [f"Azure workspace preflight {report['tool_version']} ({report['mode']} mode) status={report['status']}",
             f"candidate={report['candidate']['id']} region={report['region']} subscription={report['subscription']}"
             f" observed_at={report['observed_at']}"]
    for check in report["checks"]:
        reason = f" ({check['reason']})" if check.get("reason") else ""
        lines.append(f"  [{check['outcome']}] {check['id']}: {check['title']}{reason}")
        if report["mode"] == "plan":
            lines.append(f"      az {check['operation']}")
    lines += ["blockers: " + (", ".join(report["blockers"]) or "none observed"),
              "still unverified (live qualification gates):", *(f"  - {g}" for g in report["unverified_gates"]),
              report["claims"]]
    return "\n".join(lines)


def write_report(path: str, report: Dict[str, Any]) -> None:
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w", encoding="utf-8") as handle:
        handle.write(json.dumps(report, indent=2, sort_keys=True) + "\n")


class _Parser(argparse.ArgumentParser):
    def error(self, message: str) -> None:  # type: ignore[override]  # exit 3 instead of argparse's 2
        raise InputError(message)


def build_parser() -> argparse.ArgumentParser:
    parser = _Parser(description=__doc__.splitlines()[0])
    parser.add_argument("--candidate", required=True, choices=sorted(CANDIDATES))
    parser.add_argument("--subscription", required=True, help="exact subscription UUID (never the CLI default)")
    parser.add_argument("--region", required=True, help="lowercase region name, for example northeurope")
    parser.add_argument("--live", action="store_true", help="execute the read-only Azure CLI operations")
    parser.add_argument("--fixture", help="evaluate synthetic responses from this JSON file instead of Azure")
    parser.add_argument("--vm-size", help="exact VM size for the vm candidate, for example Standard_D4s_v3")
    for flag, default in (("--cpu-cores", 2), ("--memory-gb", 4), ("--timeout-seconds", DEFAULT_TIMEOUT_SECONDS)):
        parser.add_argument(flag, type=int, default=default)
    parser.add_argument("--az-path", default="az")
    parser.add_argument("--report", help="write the JSON report to this new private file (never overwrites)")
    parser.add_argument("--json", action="store_true", help="print the JSON report instead of the text summary")
    return parser


def main(argv: Optional[List[str]] = None, stdout=None, stderr=None) -> int:
    stdout, stderr = stdout or sys.stdout, stderr or sys.stderr
    subscription = ""
    try:
        args = build_parser().parse_args(argv)
        subscription = (args.subscription or "").lower()
        request = validate_request(args)
        executor: Optional[Executor] = subprocess_executor if request.mode == "live" else None
        if request.mode == "fixture":
            with open(args.fixture, encoding="utf-8") as handle:
                fixture = json.load(handle)
            if not isinstance(fixture, dict):
                raise InputError("fixture must be a JSON object keyed by check id")
            executor = fixture_executor(fixture, plan_checks(request))
        if args.report and os.path.lexists(args.report):
            raise InputError(f"refusing to overwrite existing report file {args.report!r}")
    except (InputError, OSError, ValueError) as error:
        print(f"error: {redact(str(error), subscription)}", file=stderr)
        return 3
    report = run_preflight(request, executor)
    print(json.dumps(report, indent=2, sort_keys=True) if args.json else render_text(report), file=stdout)
    if args.report:
        try:
            write_report(args.report, report)
        except OSError as error:
            print(f"error: report not written: {error.__class__.__name__}", file=stderr)
            return 4
    return {"planned": 0, "no_blockers_observed": 0, "blocked": 1, "unknown": 2}[report["status"]]


if __name__ == "__main__":
    sys.exit(main())
