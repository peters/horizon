#!/usr/bin/env python3
"""Read-only preflight for running a Horizon Linux worker on an existing host.

Initial slice of #604. Probes a fixed set of read-only capabilities using
fixed command *shapes* from PROBE_ARGS (never a shell). Validated host
values (docker `--host`, workspace path, XDG runtime dir, podman socket
URL) are passed as extra argv words, not interpolated into a command
string. Report fields are redacted. Each prerequisite is supported,
unsupported or unverified.

Read-only guarantees:
- subprocess argv is a PROBE_ARGS shape plus optional extra words; never shell
- host-fact file reads are limited to `--procfs-root` / `--sysfs-root`
- no writes, no installation, no image pull/run, no daemon or socket changes,
  no Tailscale state changes, no privilege escalation, no cleanup
"""

import argparse
import errno
import json
import math
import os
import re
import select
import signal
import subprocess
import sys
import time
import unicodedata
from datetime import datetime, timezone

SCHEMA = 1
TOOL = "remote-host-preflight"

SUPPORTED_ARCHS = ("x86_64", "aarch64")
MIN_CORES = 4
# 16 GiB installed RAM minus kernel-reserved pages that never appear in
# MemTotal, so a nominal 16 GiB host is not rejected.
MEM_KERNEL_RESERVE_KB = 512 * 1024
MIN_MEM_KB = 16 * 1024 * 1024 - MEM_KERNEL_RESERVE_KB
MIN_FREE_KB = 20 * 1024 * 1024  # 20 GiB free on the workspace filesystem
DEFAULT_WORKSPACE_PATH = "/var/lib/horizon-workers"
DEFAULT_TIMEOUT = 10.0
MAX_TIMEOUT = 3600.0  # select/wait cannot represent 1e300-class values
MAX_PROBE_OUTPUT_BYTES = 65536
# json.dumps can expand one byte to `\u00XX` (6 chars). stdout+stderr plus
# the watchdog wrapper must fit this cap or a completed probe looks like a timeout.
MAX_WATCHDOG_PAYLOAD = MAX_PROBE_OUTPUT_BYTES * 12 + 4096
# Explicit ASCII digit cap so oversized `df` fields are rejected even when
# Python's int-string conversion limit is disabled (`PYTHONINTMAXSTRDIGITS=0`).
MAX_NONNEG_INT_DIGITS = 20  # uint64 decimal width

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
    "podman_info": ["podman", "--remote=true", "--url"],
    "podman_info_tail": ["info", "--format", "{{.Version.Version}}"],
    "podman_socket": [sys.executable, "-B", "-c",
                      "import os,sys\n"
                      "paths=['/run/podman/podman.sock']\n"
                      "if len(sys.argv)>1 and sys.argv[1]:\n"
                      "    paths.insert(0, os.path.join(sys.argv[1],'podman','podman.sock'))\n"
                      "for p in paths:\n"
                      "    if os.path.exists(p):\n"
                      "        sys.stdout.write(p+'\\n'); sys.exit(0)\n"
                      "sys.exit(1)\n"],
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
    re.compile(r"eyJ[A-Za-z0-9_-]{4,4096}\.[A-Za-z0-9_-]{4,4096}(?:\.[A-Za-z0-9_-]{4,4096})?"),
    re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*"),
    re.compile(
        r'(?i)(?:^|[^A-Za-z0-9_-])"?[A-Za-z0-9_-]{0,64}'
        r'(?:password|passwd|secret|token|api[_-]?key)"?\s*[:=]\s*[^\n]*'
    ),
    re.compile(r'(?i)"?authorization"?\s*[:=]\s*[^\n]*'),
    re.compile(r"\b(?:ghp|gho|ghu|ghs|ghr|github_pat)_[A-Za-z0-9_]{8,255}"),
)
URI_USERINFO = re.compile(
    r"(?i)([a-z][a-z0-9+.-]{0,32}://)[^/@\s]{1,256}:[^/@\s]{1,256}@"
)

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


def run_probe(executor, key, timeout, extra_argv=None, after_binary=None):
    """Run one fixed probe; returns (result_dict, error_string_or_None).

    `extra_argv` is appended as additional argv elements (never through a
    shell). The disk probe uses this for the workspace path so `df` does
    not enumerate unrelated mounts. `after_binary` is inserted immediately
    after argv[0] so a validated docker `--host` cannot be re-resolved.
    """
    argv = list(PROBE_ARGS[key])
    if after_binary:
        argv = [argv[0]] + list(after_binary) + argv[1:]
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
    except (ValueError, TypeError, RecursionError):
        return None


def parse_nonneg_int(text):
    """Parse a non-negative decimal integer, or None if malformed.

    Reject overlong digit strings before `int()` so the result does not
    depend on Python's optional conversion limit.
    """
    if not text:
        return None
    raw = str(text)
    if not raw.isdigit() or len(raw) > MAX_NONNEG_INT_DIGITS:
        return None
    try:
        return int(raw)
    except ValueError:
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
        # Multiline stdout (warnings mixed in) is not a version.
        lines = [line.strip() for line in str(probe.get("stdout", "")).splitlines()
                 if line.strip()]
        if len(lines) != 1:
            return None
        text = redact(lines[0])
        return text or None
    payload = parse_json_output(probe)
    if not isinstance(payload, dict):
        return None
    server = payload.get("Server")
    if not isinstance(server, dict):
        return None
    version = server.get("Version")
    if not isinstance(version, str) or not version.strip():
        return None
    return redact(version.strip())


def is_local_unix_endpoint(host):
    """True for a unix socket URL or an absolute socket path."""
    if not host:
        return False
    text = str(host)
    return text.startswith("unix://") or (len(text) >= 2 and text[0] == "/")


def docker_context_host(probe):
    """Raw active-context docker Host from the `--format` template, or None.

    Keep this unredacted: `redact()` can rewrite a valid socket path such as
    `unix:///tmp/client_secret=value/docker.sock`. Redact only in diagnostics.
    """
    if probe is None or probe["exit_code"] != 0:
        return None
    host = str(probe.get("stdout", "")).strip()
    return host or None


def normalize_unix_endpoint(host):
    """`unix://` URL for a local socket path, or None if it is not pin-able."""
    if not host:
        return None
    text = str(host).strip()
    if not text or "\n" in text or "\r" in text or "\0" in text:
        return None
    if text.startswith("unix://"):
        return text
    if text.startswith("/"):
        return "unix://" + text
    return None


def docker_endpoint_reason(executor, timeout):
    """(pinned unix endpoint, None) or (None, reason).

    Both `DOCKER_HOST` and the active context Host must be local unix
    sockets. The returned endpoint is the one Docker would use (`DOCKER_HOST`
    overrides the context) and is pinned on later `--host` probes so a
    context change cannot contact a remote daemon.
    """
    host_env = os.environ.get("DOCKER_HOST")
    if host_env and not is_local_unix_endpoint(host_env):
        return None, "docker endpoint is remote (DOCKER_HOST=%s)" % redact(host_env)
    ctx, err = run_probe(executor, "docker_context", timeout)
    if err is not None:
        return None, "docker context inspect failed (%s)" % redact(err)
    if ctx["exit_code"] != 0:
        return None, "docker context inspect failed (%s)" % (
            redact(ctx.get("stderr", "")) or "exit %s" % ctx["exit_code"])
    ctx_host = docker_context_host(ctx)
    if not ctx_host:
        return None, "docker context endpoint missing"
    if not is_local_unix_endpoint(ctx_host):
        return None, "docker endpoint is remote (context Host=%s)" % redact(ctx_host)
    pinned = normalize_unix_endpoint(host_env if host_env else ctx_host)
    if not pinned:
        return None, "docker endpoint is remote"
    return pinned, None


def candidate_podman_sockets():
    """Fixed local socket paths. String join only — no filesystem probes."""
    paths = ["/run/podman/podman.sock"]
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if runtime:
        paths.insert(0, os.path.join(runtime, "podman", "podman.sock"))
    return paths


def resolve_podman_socket(executor, timeout):
    """Existing local podman API socket via a killable helper.

    Returns (path, None) when a candidate socket exists, (None, None) when
    none does, or (None, error) when the helper fails or times out.
    `XDG_RUNTIME_DIR` may name a stale FUSE/NFS path, so `exists` stays
    inside the timeout-bounded subprocess.
    """
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    extra = [runtime] if runtime else None
    result, error = run_probe(executor, "podman_socket", timeout, extra_argv=extra)
    if error:
        return None, error
    if result["exit_code"] != 0:
        return None, None
    path = str(result.get("stdout", "")).splitlines()[0].strip() if result.get("stdout") else ""
    if path not in candidate_podman_sockets():
        return None, "podman socket helper returned an unexpected path"
    return path, None


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

    docker_host, docker_note = docker_endpoint_reason(executor, timeout)
    if docker_note is not None:
        reasons["docker"] = docker_note
    else:
        pin = ["--host", docker_host]
        docker_version, dv_err = run_probe(executor, "docker_version", timeout,
                                           after_binary=pin)
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
        socket, sock_err = resolve_podman_socket(executor, timeout)
        if sock_err:
            reasons["podman"] = sock_err
        elif not socket:
            reasons["podman"] = "podman local service is not running"
        else:
            extra = ["unix://" + socket] + list(PROBE_ARGS["podman_info_tail"])
            podman_info, pi_err = run_probe(executor, "podman_info", timeout, extra_argv=extra)
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
        return (
            {"id": "container_engine", "status": UNSUPPORTED, "value": None,
             "detail": "no usable container engine: %s" % "; ".join(detail_bits)},
            {"id": "container_storage_driver", "status": UNVERIFIED, "value": None,
             "detail": "no usable container engine"},
        )

    engine_version = docker_server if engine_ok == "docker" else podman_version
    detail = "usable engine: %s %s" % (engine_ok, engine_version)
    found = {"docker": docker_server, "podman": podman_version}
    others = ["%s %s" % (n, v) for n, v in found.items() if v and n != engine_ok]
    if others:
        detail += " (also found %s)" % ", ".join(others)
    if engine_ok == "docker":
        info, err = run_probe(executor, "docker_info", timeout,
                              after_binary=["--host", docker_host])
        driver = None
        if err is not None:
            reason = "probe timed out" if "timed out" in err else "probe failed"
        elif info["exit_code"] != 0:
            reason = "probe failed (%s)" % (redact(info.get("stderr", "")) or "exit %s" % info["exit_code"])
        else:
            lines = [line.strip() for line in str(info.get("stdout", "")).splitlines()
                     if line.strip()]
            if len(lines) != 1:
                driver = None
                reason = "docker info --format Driver was not a single line"
            else:
                driver = redact(lines[0]) or None
                if driver is None:
                    reason = "docker info --format Driver was empty"
        if driver:
            driver_check = {"id": "container_storage_driver", "status": SUPPORTED,
                            "value": driver, "detail": "docker storage driver %s" % driver}
        else:
            driver_check = {"id": "container_storage_driver", "status": UNVERIFIED,
                            "value": None, "detail": "docker info: %s" % reason}
    else:
        driver_check = {"id": "container_storage_driver", "status": UNVERIFIED,
                        "value": None, "detail": "podman storage driver is not probed"}
    return (
        {"id": "container_engine", "status": SUPPORTED,
         "value": "%s %s" % (engine_ok, engine_version), "detail": detail},
        driver_check,
    )


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
        parsed = parse_nonneg_int(result.get("stdout", "").strip())
        if parsed is None or parsed < 1:
            probe_failed = True
        else:
            cores = parsed
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
            parts = line.split()
            if len(parts) >= 3 and parts[2] == "kB":
                parsed = parse_nonneg_int(parts[1])
                if parsed is not None and parsed >= 1:
                    total_kb = parsed
            break
    if total_kb is None:
        # An unreadable MemTotal is a probe failure (incomplete report),
        # not evidence of insufficient memory.
        return {"id": "memory_capacity", "status": ERROR, "value": None,
                "detail": "MemTotal missing or malformed"}
    status = SUPPORTED if total_kb >= MIN_MEM_KB else UNSUPPORTED
    return {"id": "memory_capacity", "status": status, "value": total_kb,
            "detail": "%d MiB total (reference baseline 16 GiB)" % (total_kb // 1024)}


def select_mount_point(mount_paths, resolved_path):
    """Longest mount point that is an ancestor of the already-resolved path.

    Callers must pass the helper-resolved workspace directory so this does
    not `realpath` in the main process (stale NFS/FUSE can block).
    """
    normalized = os.path.normpath(resolved_path)
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
            free_by_mount[fields[5]] = parse_nonneg_int(fields[3])
    mount = select_mount_point(mounts, target)
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
    except OSError as exc:
        if getattr(exc, "errno", None) in (errno.ENOENT, errno.ENOTDIR):
            return None, None, "block device %s not resolvable via sysfs" % block_id
        return None, None, "block device %s unreadable: %s" % (block_id, redact(exc))
    if not sysfs_device_name_ok(name):
        return None, None, "sysfs reported an unsafe device name"
    options_path = os.path.join(procfs_root, "fs", "ext4", name, "options")
    try:
        with open(options_path, "rb") as handle:
            raw = handle.read(MAX_OPTIONS_BYTES + 1)
    except OSError as exc:
        if getattr(exc, "errno", None) in (errno.ENOENT, errno.ENOTDIR):
            return name, None, "no /proc/fs/ext4/%s entry (not a mounted ext4 filesystem)" % name
        return name, None, "ext4 options unreadable: %s" % redact(exc)
    return name, raw, None


# Mirrors crates/horizon-core/src/repository_overlay/storage.rs
# `journaled_options`: exact tokens ("rwx" or a space-padded line must not
# pass), no duplicates, no empty lines, no ASCII control bytes, no spaces,
# exactly one data= line, 4096-byte cap, trailing newline required.
MAX_OPTIONS_BYTES = 4096
# storage.rs also requires name.len() <= 255 and paths::validate(name).
MAX_DEVICE_NAME_BYTES = 255
EXCLUDED_DEVICE_COMPONENTS = frozenset((
    ".env", ".envrc", ".git", ".hg", ".svn", ".ssh", ".gnupg", ".aws",
    ".azure", ".kube", ".docker", ".config", ".codex", ".claude",
    ".claude.json", ".netrc", ".npmrc", ".pypirc", ".git-credentials",
    "credentials.json", "auth.json", "id_rsa", "id_ed25519", ".cache",
    "__pycache__", "node_modules", "target", ".venv", "venv",
    ".pytest_cache", ".mypy_cache",
))


def sysfs_device_name_ok(name):
    """True when a sysfs basename would pass the worker storage name gate."""
    if not name or os.sep in name or name in (".", ".."):
        return False
    try:
        encoded = name.encode("utf-8")
    except UnicodeEncodeError:
        return False
    if len(encoded) > MAX_DEVICE_NAME_BYTES:
        return False
    if any(unicodedata.category(char) == "Cc" or char in "\\:" for char in name):
        return False
    if name.endswith(".") or name.endswith(" "):
        return False
    lower = name.lower()
    if lower == ".env" or lower.startswith(".env.") or lower.endswith(".env"):
        return False
    if lower in EXCLUDED_DEVICE_COMPONENTS:
        return False
    return True


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
        invalid_path = (
            problem == "workspace path is not a directory"
            or "does not exist and no ancestor is stat-able" in problem
        )
        status = UNSUPPORTED if invalid_path else ERROR
        return {"id": "storage_ext4_qualifier", "status": status, "value": None,
                "detail": problem}
    name, options, error = read_ext4_options(procfs_root, sysfs_root, major, minor)
    if error is not None or options is None:
        status = ERROR if error and "unreadable" in error else UNSUPPORTED
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
    return {"id": "storage_ext4_qualifier", "status": status,
            "value": None if value is None else redact(value),
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


def parsed_non_linux(os_check):
    """True when uname was parsed and the kernel is not Linux."""
    if os_check.get("status") != UNSUPPORTED:
        return False
    value = os_check.get("value")
    return bool(value) and not str(value).startswith("Linux ")


LINUX_ONLY_UNVERIFIED = (
    ("container_engine", "skipped because the kernel is not Linux"),
    ("container_storage_driver", "skipped because the kernel is not Linux"),
    ("cpu_capacity", "skipped because the kernel is not Linux"),
    ("memory_capacity", "skipped because the kernel is not Linux"),
    ("disk_capacity", "skipped because the kernel is not Linux"),
    ("storage_ext4_qualifier", "skipped because the kernel is not Linux"),
)


def build_report(procfs_root, sysfs_root, workspace_path, timeout, executor, now):
    os_check = check_os(executor, timeout)
    if parsed_non_linux(os_check):
        checks = [os_check]
        checks.extend({"id": check_id, "status": UNVERIFIED, "value": None, "detail": detail}
                      for check_id, detail in LINUX_ONLY_UNVERIFIED)
        checks.append(check_tailscale(executor, timeout))
    else:
        engine_check, driver_check = check_container_engine(executor, timeout)
        checks = [
            os_check,
            engine_check,
            driver_check,
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


def _kill_descendants(pid):
    """SIGKILL children recorded in Linux procfs, depth-first."""
    children = []
    try:
        with open("/proc/%d/task/%d/children" % (pid, pid), "r", encoding="ascii") as handle:
            children = [int(part) for part in handle.read().split() if part.isdigit()]
    except OSError:
        children = []
    for child in children:
        _kill_descendants(child)
        try:
            os.kill(child, signal.SIGKILL)
        except OSError:
            pass


def _terminate_probe(proc):
    """Kill the probe and its descendants, then reap."""
    _kill_descendants(proc.pid)
    try:
        proc.kill()
    except OSError:
        pass
    try:
        proc.wait(timeout=1)
    except (subprocess.TimeoutExpired, OSError):
        try:
            proc.kill()
        except OSError:
            pass


def bounded_communicate(proc, timeout, max_bytes):
    """Read stdout/stderr up to max_bytes. Kill the probe if it exceeds that.

    Returns (stdout, stderr, overflow_or_None). Raises TimeoutExpired.
    Overflow and timeout kill the whole process group so descendants cannot
    outlive the bounded probe.
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
                _terminate_probe(proc)
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
                    _terminate_probe(proc)
                    open_fds = []
                    break
        remaining = deadline - time.monotonic()
        if proc.poll() is None:
            if remaining <= 0:
                _terminate_probe(proc)
                raise subprocess.TimeoutExpired(proc.args, timeout)
            try:
                proc.wait(timeout=remaining)
            except subprocess.TimeoutExpired:
                _terminate_probe(proc)
                raise subprocess.TimeoutExpired(proc.args, timeout)
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


def _execute_probe(argv, timeout):
    # Stay in the watchdog session so the parent can killpg the watchdog
    # and reap a hung PATH lookup *and* any probe that already started.
    proc = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            shell=False)
    stdout, stderr, overflow = bounded_communicate(proc, timeout, MAX_PROBE_OUTPUT_BYTES)
    if overflow:
        _kill_session_except_self()
        return {"exit_code": 1, "stdout": "", "stderr": overflow, "output_exceeded": True}
    return {"exit_code": proc.returncode,
            "stdout": decode_probe_output(stdout),
            "stderr": decode_probe_output(stderr)}


def _kill_process_group(pid):
    try:
        os.killpg(pid, signal.SIGKILL)
    except OSError:
        try:
            os.kill(pid, signal.SIGKILL)
        except OSError:
            pass
    try:
        os.waitpid(pid, 0)
    except OSError:
        pass


def _kill_session_except_self():
    """SIGKILL other members of this process group (orphans holding pipes)."""
    me = os.getpid()
    try:
        pgid = os.getpgrp()
    except OSError:
        return
    try:
        names = os.listdir("/proc")
    except OSError:
        return
    for name in names:
        if not name.isdigit():
            continue
        pid = int(name)
        if pid == me:
            continue
        try:
            if os.getpgid(pid) == pgid:
                os.kill(pid, signal.SIGKILL)
        except OSError:
            pass


def _watchdog_execute_probe(argv, timeout):
    """Run the probe in a child so PATH lookup / `Popen` cannot hang the checker.

    The deadline starts before executable resolution. On expiry the child
    session is killed (probe process group included).
    """
    read_fd, write_fd = os.pipe()
    pid = os.fork()
    if pid == 0:
        os.close(read_fd)
        try:
            os.setsid()
        except OSError:
            pass
        try:
            try:
                result = _execute_probe(argv, timeout)
                if result.get("output_exceeded"):
                    _kill_session_except_self()
                payload = {"kind": "ok", "result": result}
            except FileNotFoundError:
                payload = {"kind": "fnf"}
            except subprocess.TimeoutExpired:
                _kill_session_except_self()
                payload = {"kind": "timeout"}
            except OSError as exc:
                payload = {"kind": "os", "detail": str(redact(exc))}
            os.write(write_fd, json.dumps(payload).encode("utf-8"))
        finally:
            try:
                os.close(write_fd)
            except OSError:
                pass
            os._exit(0)
    os.close(write_fd)
    deadline = time.monotonic() + timeout
    chunks = bytearray()
    try:
        os.set_blocking(read_fd, False)
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                _kill_process_group(pid)
                raise subprocess.TimeoutExpired(argv, timeout)
            ready, _, _ = select.select([read_fd], [], [], remaining)
            if not ready:
                _kill_process_group(pid)
                raise subprocess.TimeoutExpired(argv, timeout)
            try:
                chunk = os.read(read_fd, 65536)
            except BlockingIOError:
                continue
            if chunk == b"":
                break
            chunks.extend(chunk)
            if len(chunks) > MAX_WATCHDOG_PAYLOAD:
                _kill_process_group(pid)
                raise subprocess.TimeoutExpired(argv, timeout)
        try:
            os.waitpid(pid, 0)
        except OSError:
            pass
    finally:
        try:
            os.close(read_fd)
        except OSError:
            pass
    if not chunks:
        raise subprocess.TimeoutExpired(argv, timeout)
    try:
        payload = json.loads(chunks.decode("utf-8"))
    except (ValueError, UnicodeDecodeError, RecursionError) as exc:
        raise OSError("probe watchdog returned malformed result") from exc
    kind = payload.get("kind") if isinstance(payload, dict) else None
    if kind == "ok" and isinstance(payload.get("result"), dict):
        result = payload["result"]
        if result.get("output_exceeded"):
            _kill_process_group(pid)
        return result
    if kind == "fnf":
        raise FileNotFoundError(argv[0] if argv else "probe")
    if kind == "timeout":
        _kill_process_group(pid)
        raise subprocess.TimeoutExpired(argv, timeout)
    raise OSError(payload.get("detail", "probe could not run") if isinstance(payload, dict)
                  else "probe could not run")


def default_executor(argv, timeout):
    if hasattr(os, "fork"):
        return _watchdog_execute_probe(argv, timeout)
    return _execute_probe(argv, timeout)


def parse_timeout(value):
    """Positive finite seconds. Rejects negatives, zero, NaN and infinities."""
    try:
        timeout = float(value)
    except (TypeError, ValueError) as exc:
        raise argparse.ArgumentTypeError(
            "timeout must be a positive finite number") from exc
    if not math.isfinite(timeout) or timeout <= 0:
        raise argparse.ArgumentTypeError("timeout must be a positive finite number")
    if timeout > MAX_TIMEOUT:
        raise argparse.ArgumentTypeError(
            "timeout must be at most %ss" % format_seconds(MAX_TIMEOUT))
    return timeout


class PreflightArgumentParser(argparse.ArgumentParser):
    """Invalid CLI usage exits 3 so it is not confused with probe error 2."""

    def error(self, message):
        self.print_usage(sys.stderr)
        self.exit(3, "%s: error: %s\n" % (self.prog, message))


def main(argv=None, executor=None, now=None):
    parser = PreflightArgumentParser(description=__doc__)
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
    stamp = args.now if args.now is not None else now
    report = build_report(args.procfs_root, args.sysfs_root, args.workspace_path,
                          args.timeout, executor, stamp)
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
