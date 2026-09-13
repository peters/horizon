#!/usr/bin/env python3
"""Read-only preflight for running a Horizon Linux worker on an existing host.

Initial slice of #604. Probes a fixed set of read-only capabilities, issues
only fixed argument vectors (no shell, no interpolation of host-provided
values into probe arguments), redacts anything outside the fixed report
fields, and classifies each prerequisite as supported, unsupported or
unverified.

Read-only guarantees:
- subprocess calls use fixed argv vectors from PROBE_ARGS only; never shell
- direct file reads are limited to fixed procfs/sysfs paths under the given roots
- no writes, no installation, no image pull/run, no daemon or socket changes,
  no Tailscale state changes, no privilege escalation, no cleanup
"""

import argparse
import json
import math
import os
import re
import select
import subprocess
import sys
import time
from datetime import datetime, timezone

SCHEMA = 1
TOOL = "remote-host-preflight"

SUPPORTED_ARCHS = ("x86_64", "aarch64")
MIN_CORES = 4
MIN_MEM_KB = 16 * 1024 * 1024  # 16 GiB: reference CPU worker baseline
MIN_FREE_KB = 20 * 1024 * 1024  # 20 GiB free on the workspace filesystem
DEFAULT_WORKSPACE_PATH = "/var/lib/horizon-workers"
DEFAULT_TIMEOUT = 10.0
MAX_PROBE_OUTPUT_BYTES = 65536

# Status values, kept separate on purpose per the issue contract.
SUPPORTED = "supported"
UNSUPPORTED = "unsupported"
UNVERIFIED = "unverified"
ERROR = "error"

# Fixed argv vectors. The workspace path is substituted verbatim as one argv
# element (never through a shell), so no interpolation is possible.
PROBE_ARGS = {
    "os": ["uname", "-srm"],
    "cores": ["nproc"],
    "docker_version": ["docker", "version", "--format", "json"],
    "docker_info": ["docker", "info", "--format", "{{.Driver}}"],
    "docker_context": ["docker", "context", "inspect", "--format",
                       "{{.Endpoints.docker.Host}}"],
    "podman_info": ["podman", "--remote=false", "info", "--format",
                    "{{.Version.Version}}"],
    "workspace_dir": [sys.executable, "-B", "-c",
                      "import os,sys\n"
                      "p=os.path.normpath(os.path.realpath(sys.argv[1]))\n"
                      "c=p\n"
                      "while True:\n"
                      "    if os.path.isdir(c):\n"
                      "        st=os.stat(c)\n"
                      "        sys.stdout.write('%s\\t%d\\t%d\\n'%(c,os.major(st.st_dev),os.minor(st.st_dev)))\n"
                      "        sys.exit(0)\n"
                      "    if os.path.lexists(c):\n"
                      "        sys.stderr.write('not-a-directory\\n'); sys.exit(2)\n"
                      "    n=os.path.dirname(c)\n"
                      "    if n==c:\n"
                      "        sys.stderr.write('missing\\n'); sys.exit(3)\n"
                      "    c=n\n"],
    "disk": ["df", "-kP"],
    "tailscale_version": ["tailscale", "version"],
    "tailscale_status": ["tailscale", "status", "--json", "--peers=false"],
}

# Probes whose absence is a finding, not a crash: the engine pair and tailscale.
OPTIONAL_PROBES = {"docker_version", "docker_info", "docker_context", "podman_info",
                   "tailscale_version", "tailscale_status"}

# Endpoint-selection variables inspected for locality. Presence only — never
# an environment dump. Tests clear this set so developer shells cannot leak.
DOCKER_ENDPOINT_VARS = ("DOCKER_HOST", "DOCKER_CONTEXT")
PODMAN_ENDPOINT_VARS = ("PODMAN_CONNECTION", "PODMAN_HOST",
                        "CONTAINER_HOST", "CONTAINER_CONNECTION")
ENDPOINT_VARS = DOCKER_ENDPOINT_VARS + PODMAN_ENDPOINT_VARS

# Credential-shaped material that must never survive into the report.
# Redactors redact the complete value (to end of line) for the assignment and
# header forms: a partial redaction of "Authorization: Bearer <token>" would
# leak the credential, and whitespace-containing values are otherwise only
# partially removed.
REDACTED_PATTERNS = (
    re.compile(r"eyJ[A-Za-z0-9_-]{4,}\.[A-Za-z0-9_-]{4,}(?:\.[A-Za-z0-9_-]{4,})?"),  # JWT-like
    re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*"),
    re.compile(
        r"(?i)[A-Za-z0-9_-]*(password|passwd|secret|token|api[_-]?key)\s*[:=]\s*[^\n]*"
    ),
    re.compile(r"(?i)\bauthorization\s*:\s*[^\n]*"),
    re.compile(r"\b(?:ghp|gho|ghu|ghs|ghr|github_pat)_[A-Za-z0-9_]{8,}"),
)
URI_USERINFO = re.compile(r"(?i)([a-z][a-z0-9+.-]*://)[^/@\s]+:[^/@\s]+@")

# Precomputed read-only facts that cannot be proven from host metadata alone.
ALWAYS_UNVERIFIED = (
    ("storage_durability",
     "ext4 options do not prove retention across power loss, reboot or "
     "container removal; requires the later on-worker acceptance"),
    ("effective_isolation",
     "host metadata does not prove the worker's container isolation "
     "boundaries; requires the later on-worker acceptance"),
    ("worker_startup",
     "read-only probes never start a worker; startup and reconnect remain "
     "unverified until the later slices"),
)


def redact(text):
    """Remove credential-shaped material from any host-provided text."""
    if not text:
        return ""
    out = str(text)
    out = URI_USERINFO.sub(r"\1<redacted>@", out)
    for pattern in REDACTED_PATTERNS:
        out = pattern.sub("<redacted>", out)
    return out[:400]


def run_probe(executor, key, timeout, extra_argv=None):
    """Run one fixed probe; returns (result_dict, error_string_or_None).

    `extra_argv` is appended as additional argv elements (never through a
    shell). The disk probe uses this for the workspace path so `df` does
    not enumerate unrelated mounts.
    """
    argv = list(PROBE_ARGS[key])
    if extra_argv:
        argv.extend(extra_argv)
    try:
        result = executor(argv, timeout)
    except subprocess.TimeoutExpired:
        return None, "probe timed out after %ss" % format_seconds(timeout)
    except FileNotFoundError:
        if key in OPTIONAL_PROBES:
            return None, "tool not present"
        return None, "required tool missing"
    except OSError as exc:
        return None, "probe could not run: %s" % redact(exc)
    if not isinstance(result, dict):
        return None, "probe returned malformed result"
    exit_code = result.get("exit_code")
    if result.get("output_exceeded"):
        return None, "probe output exceeded %d bytes" % MAX_PROBE_OUTPUT_BYTES
    if not isinstance(exit_code, int):
        return None, "probe returned malformed exit code"
    return result, None


def parse_json_output(result):
    try:
        return json.loads(result.get("stdout", ""))
    except (ValueError, TypeError):
        return None


def check_os(executor, timeout):
    result, error = run_probe(executor, "os", timeout)
    if error:
        return {"id": "os_linux", "status": ERROR, "value": None, "detail": error}
    if result["exit_code"] != 0:
        return {"id": "os_linux", "status": ERROR, "value": None,
                "detail": redact(result.get("stderr", "")) or "uname failed"}
    fields = redact(result.get("stdout", "")).strip().split()
    if len(fields) < 3:
        return {"id": "os_linux", "status": ERROR, "value": None,
                "detail": "uname output truncated"}
    if fields[0] != "Linux":
        value = " ".join(fields[:3])
        return {"id": "os_linux", "status": UNSUPPORTED, "value": value,
                "detail": redact("kernel reports %s" % value)}
    machine = fields[2]
    release = fields[1]
    if machine in SUPPORTED_ARCHS:
        return {"id": "os_linux", "status": SUPPORTED, "value": " ".join(fields[:3]),
                "detail": "Linux %s on %s (supported architecture)" % (release, machine)}
    return {"id": "os_linux", "status": UNSUPPORTED, "value": " ".join(fields[:3]),
            "detail": "Linux %s: architecture %s is outside %s"
                      % (release, machine, "/".join(SUPPORTED_ARCHS))}


def engine_failure_bit(name, probe, error, timeout):
    """One reason bit for an engine that was not usable, classified as
    absent / timeout / nonzero-exit / malformed-or-unreachable."""
    if error is not None:
        if "timed out" in error:
            return "%s: probe timed out after %ss" % (name, format_seconds(timeout))
        if "not present" in error:
            return "%s: tool not present" % name
        return "%s: %s" % (name, redact(error))
    if probe is None:
        return "%s: probe could not run" % name
    if probe["exit_code"] != 0:
        return "%s present but probe failed (%s)" % (name, redact(probe.get("stderr", "")) or "exit %s" % probe["exit_code"])
    return "%s present but response payload unusable" % name


def parse_engine_version(probe, name):
    """Version string from a successful engine probe, or None."""
    if probe is None or probe["exit_code"] != 0:
        return None
    if name == "podman":
        # `podman info --format '{{.Version.Version}}'` returns one field.
        text = redact(str(probe.get("stdout", "")).strip())
        return text or None
    payload = parse_json_output(probe)
    if not isinstance(payload, dict):
        return None
    server = payload.get("Server")
    if isinstance(server, dict) and server.get("Version"):
        return redact(str(server["Version"]))
    return None


def is_local_unix_endpoint(host):
    """True for a unix socket URL or an absolute socket path."""
    if not host:
        return False
    text = str(host)
    return text.startswith("unix://") or (len(text) >= 2 and text[0] == "/")


def docker_context_host(probe):
    """Active-context docker Host from the `--format` template, or None."""
    if probe is None or probe["exit_code"] != 0:
        return None
    host = redact(str(probe.get("stdout", "")).strip())
    return host or None


def docker_endpoint_reason(executor, timeout):
    """None when the selected docker endpoint is a local unix socket.

    Both `DOCKER_HOST` and the active context (selected by `DOCKER_CONTEXT`)
    must be local unix sockets. A local socket env var must not skip a
    remote context, and missing inspect output fails closed.
    """
    host_env = os.environ.get("DOCKER_HOST")
    if host_env and not is_local_unix_endpoint(host_env):
        return "docker endpoint is remote (DOCKER_HOST=%s)" % redact(host_env)
    ctx, err = run_probe(executor, "docker_context", timeout)
    if err is not None:
        return "docker context inspect failed (%s)" % redact(err)
    ctx_host = docker_context_host(ctx)
    if not ctx_host:
        return "docker context endpoint missing"
    if not is_local_unix_endpoint(ctx_host):
        return "docker endpoint is remote (context Host=%s)" % ctx_host
    return None


def engine_endpoint_note(engine):
    """None when a podman client has no named/remote connection variables set.

    Only those endpoint variables are inspected — never an env dump.
    """
    for var in PODMAN_ENDPOINT_VARS:
        if os.environ.get(var):
            return "%s endpoint may be remote (%s is set)" % (engine, var)
    return None


def docker_server_ostype(probe):
    """Linux/OS name from `docker version` JSON.

    The documented Go field is `Server.OSType`; the live `docker version
    --format json` payload on Engine 29 exposes `Server.Os` instead. Either
    value establishes the server OS; absence of both is missing.
    """
    if probe is None or probe["exit_code"] != 0:
        return None
    payload = parse_json_output(probe)
    server = payload.get("Server") if isinstance(payload, dict) else None
    if not isinstance(server, dict):
        return None
    for key in ("OSType", "Os", "os"):
        raw = server.get(key)
        if raw:
            return redact(str(raw)).strip().lower()
    return None


def check_container_engine(executor, timeout):
    """Locality first, then daemon probes. A remote endpoint is never contacted."""
    reasons = {}
    docker_version, dv_err, docker_server = None, None, None
    podman_info, pi_err, podman_version = None, None, None

    docker_note = docker_endpoint_reason(executor, timeout)
    if docker_note is not None:
        reasons["docker"] = docker_note
    else:
        docker_version, dv_err = run_probe(executor, "docker_version", timeout)
        docker_server = parse_engine_version(docker_version, "docker")
        if docker_server is not None:
            ostype = docker_server_ostype(docker_version)
            if ostype != "linux":
                reasons["docker"] = ("docker server OS is %s, not linux" % ostype
                                     if ostype else "docker server OSType missing")

    podman_note = engine_endpoint_note("podman")
    if podman_note is not None:
        reasons["podman"] = podman_note
    else:
        podman_info, pi_err = run_probe(executor, "podman_info", timeout)
        podman_version = parse_engine_version(podman_info, "podman")

    engine_ok = None
    if docker_server is not None and "docker" not in reasons:
        engine_ok = "docker"
    elif podman_version is not None and "podman" not in reasons:
        engine_ok = "podman"

    if engine_ok is None:
        detail_bits = []
        for name, probe, err, version, reason in (
                ("docker", docker_version, dv_err, docker_server, reasons.get("docker")),
                ("podman", podman_info, pi_err, podman_version, reasons.get("podman"))):
            if reason:
                detail_bits.append("%s: %s" % (name, reason))
            elif version is None:
                detail_bits.append(engine_failure_bit(name, probe, err, timeout))
            else:
                detail_bits.append("%s: %s" % (name, reason))
        return {"id": "container_engine", "status": UNSUPPORTED, "value": None,
                "detail": "no usable container engine: %s" % "; ".join(detail_bits)}

    engine_version = docker_server if engine_ok == "docker" else podman_version
    detail = "usable engine: %s %s" % (engine_ok, engine_version)
    found = {"docker": docker_server, "podman": podman_version}
    others = ["%s %s" % (n, v) for n, v in found.items() if v and n != engine_ok]
    if others:
        detail += " (also found %s)" % ", ".join(others)
    if engine_ok == "docker":
        info, err = run_probe(executor, "docker_info", timeout)
        driver = None
        if err is not None:
            reason = "probe timed out" if "timed out" in err else "probe failed"
        elif info["exit_code"] != 0:
            reason = "probe failed (%s)" % (redact(info.get("stderr", "")) or "exit %s" % info["exit_code"])
        else:
            driver = redact(str(info.get("stdout", "")).strip()) or None
            if driver is None:
                reason = "docker info --format Driver was empty"
        if driver:
            detail += "; storage driver %s" % driver
        else:
            detail += "; storage driver unverified (docker info: %s)" % reason
    return {"id": "container_engine", "status": SUPPORTED, "value": "%s %s" % (engine_ok, engine_version),
            "detail": detail}


def read_procfs(procfs_root, name):
    path = os.path.join(procfs_root, name)
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as handle:
            return handle.read()
    except OSError:
        return None


def check_capacity(executor, timeout, procfs_root):
    cores = None
    probe_failed = False
    result, error = run_probe(executor, "cores", timeout)
    if error or result["exit_code"] != 0:
        probe_failed = True
    else:
        try:
            cores = int(result.get("stdout", "").strip())
        except ValueError:
            probe_failed = True
    if cores is None:
        cpuinfo = read_procfs(procfs_root, "cpuinfo")
        if cpuinfo:
            parsed = sum(1 for line in cpuinfo.splitlines() if line.startswith("processor"))
            cores = parsed if parsed > 0 else None
    if cores is None:
        status = ERROR if probe_failed else UNSUPPORTED
        detail = "cpu count unreadable (nproc probe failed)" if probe_failed \
            else "cpu count unreadable"
        return {"id": "cpu_capacity", "status": status, "value": None, "detail": detail}
    status = SUPPORTED if cores >= MIN_CORES else UNSUPPORTED
    return {"id": "cpu_capacity", "status": status, "value": cores,
            "detail": "%d cores (reference baseline %d)" % (cores, MIN_CORES)}


def check_memory(procfs_root):
    meminfo = read_procfs(procfs_root, "meminfo")
    if meminfo is None:
        return {"id": "memory_capacity", "status": ERROR, "value": None,
                "detail": "meminfo unreadable"}
    total_kb = None
    for line in meminfo.splitlines():
        if line.startswith("MemTotal:"):
            try:
                total_kb = int(line.split()[1])
            except (ValueError, IndexError):
                total_kb = None
            break
    if total_kb is None:
        # An unreadable MemTotal is a probe failure (incomplete report),
        # not evidence of insufficient memory.
        return {"id": "memory_capacity", "status": ERROR, "value": None,
                "detail": "MemTotal missing or malformed"}
    status = SUPPORTED if total_kb >= MIN_MEM_KB else UNSUPPORTED
    return {"id": "memory_capacity", "status": status, "value": total_kb,
            "detail": "%d MiB total (reference baseline 16 GiB)" % (total_kb // 1024)}


def select_mount_point(mount_paths, workspace_path):
    """Longest mount point that is an ancestor of (or is) the resolved
    workspace path.

    A workspace such as /mnt/workspaces/job lives on the /mnt/workspaces
    mount, not on /; selecting the wrong filesystem produces a wrong verdict.
    The path is resolved first so a symlinked workspace is judged on the
    mount its target actually lives on (the storage qualifier follows the
    same resolution via os.stat). realpath resolves symlinks in the
    existing prefix and preserves a not-yet-created suffix, so this works
    for workspaces that do not exist yet.
    """
    normalized = os.path.normpath(os.path.realpath(workspace_path))
    best = None
    for mount in mount_paths:
        if mount == "/" or mount == normalized or normalized.startswith(mount.rstrip("/") + "/"):
            if best is None or len(mount) > len(best):
                best = mount
    return best


def format_seconds(timeout):
    """Render a timeout without rounding (0.1 stays 0.1, not 0)."""
    return ("%.6f" % timeout).rstrip("0").rstrip(".")


def resolve_workspace_directory(executor, timeout, path):
    """Killable workspace dir + device identity, bounded by `timeout`."""
    result, error = run_probe(executor, "workspace_dir", timeout, extra_argv=[path])
    if error:
        return None, None, None, error
    if result["exit_code"] == 0:
        line = str(result.get("stdout", "")).splitlines()[0] if result.get("stdout") else ""
        parts = line.split("\t")
        if len(parts) != 3:
            return None, None, None, "workspace resolver returned malformed output"
        try:
            return parts[0], int(parts[1]), int(parts[2]), None
        except ValueError:
            return None, None, None, "workspace resolver returned malformed output"
    err = redact(result.get("stderr", "")).strip()
    if result["exit_code"] == 2 or "not-a-directory" in err:
        return None, None, None, "workspace path is not a directory"
    return None, None, None, ("workspace path %s does not exist and no ancestor is stat-able"
                              % redact(path))


def check_disk(executor, timeout, workspace_path):
    target, _major, _minor, problem = resolve_workspace_directory(
        executor, timeout, workspace_path)
    if problem:
        return {"id": "disk_capacity", "status": ERROR, "value": None,
                "detail": problem}
    result, error = run_probe(executor, "disk", timeout, extra_argv=[target])
    if error:
        return {"id": "disk_capacity", "status": ERROR, "value": None, "detail": error}
    if result["exit_code"] != 0:
        return {"id": "disk_capacity", "status": ERROR, "value": None,
                "detail": redact(result.get("stderr", "")) or "df failed"}
    mounts = []
    free_by_mount = {}
    for line in str(result.get("stdout", "")).splitlines()[1:]:
        # POSIX `df -P`: six fields; the mount point may contain spaces, so
        # split at most five times and keep the remainder intact.
        fields = line.split(None, 5)
        if len(fields) >= 6:
            mounts.append(fields[5])
            free_by_mount[fields[5]] = int(fields[3]) if fields[3].isdigit() else None
    mount = select_mount_point(mounts, workspace_path)
    if mount is None:
        return {"id": "disk_capacity", "status": ERROR, "value": None,
                "detail": "df output contains no mount point covering %s" % redact(workspace_path)}
    free_kb = free_by_mount.get(mount)
    if free_kb is None:
        # A malformed free value on the *selected* entry is rejected rather
        # than silently falling back to another filesystem.
        return {"id": "disk_capacity", "status": ERROR, "value": None,
                "detail": "df reported a malformed free value for mount %s" % redact(mount)}
    status = SUPPORTED if free_kb >= MIN_FREE_KB else UNSUPPORTED
    detail = "%d MiB free on %s (reference baseline 20 GiB)" % (
        free_kb // 1024, redact(mount))
    return {"id": "disk_capacity", "status": status, "value": free_kb, "detail": detail}


def workspace_directory(path):
    """Nearest existing directory of the resolved path, or (None, error).

    A workspace that exists as a regular file (or whose first existing
    ancestor is not a directory) cannot host the worker tree.
    """
    resolved = os.path.normpath(os.path.realpath(path))
    current = resolved
    while True:
        if os.path.isdir(current):
            return current, None
        if os.path.lexists(current):
            return None, "workspace path is not a directory"
        parent = os.path.dirname(current)
        if parent == current:
            return None, ("workspace path %s does not exist and no ancestor is stat-able"
                          % redact(path))
        current = parent


def nearest_existing(path):
    """Nearest existing directory of the resolved path, or None."""
    target, _error = workspace_directory(path)
    return target


def read_ext4_options(procfs_root, sysfs_root, dev_major, dev_minor):
    """Resolve the block device name via sysfs, then read its ext4 options.

    Returns (name, options_bytes_or_None, error_or_None). The options are
    returned as raw bytes: the gate below enforces the authoritative
    byte-level contract (4096-byte cap before decoding, trailing newline,
    exact tokens) and must see exactly what the kernel wrote.
    """
    block_id = "%d:%d" % (dev_major, dev_minor)
    block_path = os.path.join(sysfs_root, "dev", "block", block_id)
    if not os.path.lexists(block_path):
        return None, None, "block device %s not resolvable via sysfs" % block_id
    try:
        name = os.path.basename(os.path.realpath(block_path))
    except OSError:
        return None, None, "block device %s not resolvable via sysfs" % block_id
    if not name or name in (".", "..") or os.sep in name:
        return None, None, "sysfs reported an unsafe device name"
    options_path = os.path.join(procfs_root, "fs", "ext4", name, "options")
    try:
        with open(options_path, "rb") as handle:
            raw = handle.read(MAX_OPTIONS_BYTES + 1)
    except OSError:
        return name, None, "no /proc/fs/ext4/%s entry (not a mounted ext4 filesystem)" % name
    return name, raw, None


# Mirrors crates/horizon-core/src/repository_overlay/storage.rs
# `journaled_options`: exact tokens ("rwx" or a space-padded line must not
# pass), no duplicates, no empty lines, no ASCII control bytes, no spaces,
# exactly one data= line, 4096-byte cap, trailing newline required.
MAX_OPTIONS_BYTES = 4096


def ext4_qualifier_problems(raw):
    """Returns the list of gate violations for a raw options payload."""
    if len(raw) > MAX_OPTIONS_BYTES:
        return ["options longer than %d bytes" % MAX_OPTIONS_BYTES]
    try:
        options = raw.decode("utf-8")
    except UnicodeDecodeError:
        return ["options are not valid UTF-8"]
    if not options.endswith("\n"):
        return ["options missing trailing newline"]
    lines = options.split("\n")[:-1]
    problems = []
    if len(set(lines)) != len(lines):
        problems.append("duplicate option lines")
    for line in lines:
        if line == "":
            problems.append("empty option line")
            break
        for byte in line.encode("utf-8"):
            if byte < 0x20 or byte == 0x7F or byte == 0x20:
                problems.append("control character or space in option line")
                break
        if problems:
            break
    if "rw" not in lines:
        problems.append("missing exact token rw")
    if "barrier" not in lines:
        problems.append("missing exact token barrier")
    if "ro" in lines:
        problems.append("forbidden token ro")
    if "nobarrier" in lines:
        problems.append("forbidden token nobarrier")
    data_lines = [line for line in lines if line.startswith("data=")]
    if len(data_lines) != 1 or data_lines[0] not in ("data=ordered", "data=journal"):
        problems.append("needs exactly one data=ordered or data=journal line")
    return problems


def check_storage_qualifier(procfs_root, sysfs_root, workspace_path, executor, timeout):
    _target, major, minor, problem = resolve_workspace_directory(
        executor, timeout, workspace_path)
    if problem:
        status = ERROR if "timed out" in problem else UNSUPPORTED
        return {"id": "storage_ext4_qualifier", "status": status, "value": None,
                "detail": problem}
    name, options, error = read_ext4_options(procfs_root, sysfs_root, major, minor)
    if error is not None or options is None:
        status = UNSUPPORTED
        value = name
        detail = error or "ext4 options unreadable"
    else:
        problems = ext4_qualifier_problems(options)
        if not problems:
            status, value = SUPPORTED, name
            detail = "device %s meets the on-worker ext4 qualifier" % name
        else:
            status, value = UNSUPPORTED, name
            detail = "device %s fails the qualifier: %s" % (name, "; ".join(problems))
    return {"id": "storage_ext4_qualifier", "status": status, "value": value,
            "detail": redact(detail)}


def check_tailscale(executor, timeout):
    version, verr = run_probe(executor, "tailscale_version", timeout)
    # Tailscale is optional: issue #604 explicitly permits ordinary pinned
    # SSH, so absence keeps the verdict out of the prerequisite gate.
    if verr:
        return {"id": "tailscale", "status": UNVERIFIED, "value": None,
                "detail": "tailscale not present (%s); ordinary pinned SSH remains usable; "
                          "client-side reachability remains unverified" % redact(verr)}
    if version["exit_code"] != 0:
        return {"id": "tailscale", "status": UNVERIFIED, "value": None,
                "detail": redact(version.get("stderr", "")) or "tailscale version failed; "
                          "client-side reachability remains unverified"}
    self_name = None
    online = None
    status, serr = run_probe(executor, "tailscale_status", timeout)
    if serr is None and status["exit_code"] == 0:
        payload = parse_json_output(status)
        if isinstance(payload, dict) and isinstance(payload.get("Self"), dict):
            if payload["Self"].get("DNSName"):
                # Host-provided: redact before it can reach the report, then
                # drop the tailnet DNS trailing dot.
                self_name = redact(str(payload["Self"]["DNSName"]))
                if self_name.endswith("<redacted>."):
                    self_name = self_name[:-1]
                elif self_name.endswith("."):
                    self_name = self_name[:-1]
            raw_online = payload["Self"].get("Online")
            online = raw_online if isinstance(raw_online, bool) else None
    stdout_lines = [line for line in str(version.get("stdout", "")).splitlines() if line.strip()]
    version_line = redact(stdout_lines[0] if stdout_lines else "unknown")
    value = self_name or version_line.split()[0]
    detail = "tailscale %s" % version_line
    if self_name:
        detail += " as %s (online=%s)" % (self_name, "unknown" if online is None else str(online).lower())
    return {"id": "tailscale", "status": SUPPORTED, "value": value,
            "detail": detail + "; client-side reachability remains unverified"}


def build_report(procfs_root, sysfs_root, workspace_path, timeout, executor, now):
    checks = [
        check_os(executor, timeout),
        check_container_engine(executor, timeout),
        check_capacity(executor, timeout, procfs_root),
        check_memory(procfs_root),
        check_disk(executor, timeout, workspace_path),
        check_storage_qualifier(procfs_root, sysfs_root, workspace_path, executor, timeout),
        check_tailscale(executor, timeout),
    ]
    checks.extend({"id": check_id, "status": UNVERIFIED, "value": None, "detail": detail}
                  for check_id, detail in ALWAYS_UNVERIFIED)
    counts = {state: 0 for state in (SUPPORTED, UNSUPPORTED, UNVERIFIED, ERROR)}
    for check in checks:
        counts[check["status"]] += 1
    if counts[ERROR] > 0:
        verdict = "incomplete"
    elif counts[UNSUPPORTED] > 0:
        verdict = "unsupported"
    else:
        verdict = "supported"
    generated = now if now is not None else datetime.now(timezone.utc).isoformat()
    return {
        "tool": TOOL,
        "schema": SCHEMA,
        "generated_at": generated,
        "checks": checks,
        "summary": dict(counts, verdict=verdict),
    }


def printable_line(text):
    """One printable line: drop C0/DEL controls, collapse newlines to spaces."""
    chars = []
    for char in str(text or ""):
        code = ord(char)
        if char in "\n\r":
            chars.append(" ")
        elif code < 32 or code == 127:
            continue
        else:
            chars.append(char)
    return " ".join("".join(chars).split())


def render_text(report):
    lines = ["%s report (schema %d)" % (report["tool"], report["schema"]), ""]
    for check in report["checks"]:
        detail = printable_line(check.get("detail", ""))
        lines.append("%-24s %-11s %s" % (check["id"], check["status"], detail))
    summary = report["summary"]
    lines.append("")
    lines.append("verdict: %s (supported %d, unsupported %d, unverified %d, errors %d)"
                 % (summary["verdict"], summary["supported"], summary["unsupported"],
                    summary["unverified"], summary["error"]))
    return "\n".join(lines)


def decode_probe_output(data):
    """Decode probe bytes with replacement so non-UTF-8 cannot crash preflight."""
    if data is None:
        return ""
    if isinstance(data, bytes):
        return data.decode("utf-8", errors="replace")
    return str(data)


def bounded_communicate(proc, timeout, max_bytes):
    """Read stdout/stderr up to max_bytes. Kill the probe if it exceeds that.

    Returns (stdout, stderr, overflow_or_None). Raises TimeoutExpired.
    """
    deadline = time.monotonic() + timeout
    buckets = {proc.stdout: bytearray(), proc.stderr: bytearray()}
    open_fds = [proc.stdout, proc.stderr]
    for fd in open_fds:
        os.set_blocking(fd.fileno(), False)
    overflow = False
    try:
        while open_fds:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                proc.kill()
                try:
                    proc.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    proc.kill()
                raise subprocess.TimeoutExpired(proc.args, timeout)
            ready, _, _ = select.select(open_fds, [], [], remaining)
            for fd in ready:
                chunk = fd.read(4096)
                if chunk is None:
                    continue
                if chunk == b"":
                    open_fds.remove(fd)
                    continue
                buckets[fd].extend(chunk)
                if len(buckets[proc.stdout]) + len(buckets[proc.stderr]) > max_bytes:
                    overflow = True
                    proc.kill()
                    try:
                        proc.wait(timeout=1)
                    except subprocess.TimeoutExpired:
                        proc.kill()
                    open_fds = []
                    break
        if proc.poll() is None:
            proc.wait()
        stdout = bytes(buckets[proc.stdout])
        stderr = bytes(buckets[proc.stderr])
        if overflow:
            return stdout, stderr, "probe output exceeded %d bytes" % max_bytes
        return stdout, stderr, None
    finally:
        for fd in (proc.stdout, proc.stderr):
            if fd is not None:
                try:
                    fd.close()
                except OSError:
                    pass


def default_executor(argv, timeout):
    proc = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            shell=False)
    stdout, stderr, overflow = bounded_communicate(proc, timeout, MAX_PROBE_OUTPUT_BYTES)
    if overflow:
        return {"exit_code": 1, "stdout": "", "stderr": overflow, "output_exceeded": True}
    return {"exit_code": proc.returncode,
            "stdout": decode_probe_output(stdout),
            "stderr": decode_probe_output(stderr)}


def parse_timeout(value):
    """Positive finite seconds. Rejects negatives, zero, NaN and infinities."""
    try:
        timeout = float(value)
    except (TypeError, ValueError) as exc:
        raise argparse.ArgumentTypeError(
            "timeout must be a positive finite number") from exc
    if not math.isfinite(timeout) or timeout <= 0:
        raise argparse.ArgumentTypeError("timeout must be a positive finite number")
    return timeout


def main(argv=None, executor=None, now=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace-path", default=DEFAULT_WORKSPACE_PATH,
                        help="intended workspace directory (default: %(default)s)")
    parser.add_argument("--procfs-root", default="/proc",
                        help="procfs root for synthetic testing (default: /proc)")
    parser.add_argument("--sysfs-root", default="/sys",
                        help="sysfs root for synthetic testing (default: /sys)")
    parser.add_argument("--now", default=None,
                        help="fixed generated_at timestamp for deterministic output")
    parser.add_argument("--timeout", type=parse_timeout, default=DEFAULT_TIMEOUT,
                        help="per-probe timeout in seconds (default: %(default)s)")
    parser.add_argument("--json", action="store_true", help="emit the JSON report")
    args = parser.parse_args(argv)

    executor = executor or default_executor
    report = build_report(args.procfs_root, args.sysfs_root, args.workspace_path,
                          args.timeout, executor, args.now)
    if args.json:
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        print(render_text(report))
    if report["summary"]["error"] > 0:
        return 2
    if report["summary"]["unsupported"] > 0:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
