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
import os
import re
import subprocess
import sys
from datetime import datetime, timezone

SCHEMA = 1
TOOL = "remote-host-preflight"

SUPPORTED_ARCHS = ("x86_64", "aarch64")
MIN_CORES = 4
MIN_MEM_KB = 16 * 1024 * 1024  # 16 GiB: reference CPU worker baseline
MIN_FREE_KB = 20 * 1024 * 1024  # 20 GiB free on the workspace filesystem
DEFAULT_WORKSPACE_PATH = "/var/lib/horizon-workers"
DEFAULT_TIMEOUT = 10.0

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
    "docker_info": ["docker", "info"],
    "podman_info": ["podman", "info", "--format", "json"],
    "disk": ["df", "-kP"],
    "tailscale_version": ["tailscale", "version"],
    "tailscale_status": ["tailscale", "status", "--json"],
}

# Probes whose absence is a finding, not a crash: the engine pair and tailscale.
OPTIONAL_PROBES = {"docker_version", "docker_info", "podman_info",
                   "tailscale_version", "tailscale_status"}

# Credential-shaped material that must never survive into the report.
REDACTED_PATTERNS = (
    re.compile(r"eyJ[A-Za-z0-9_-]{4,}\.[A-Za-z0-9_-]{4,}(?:\.[A-Za-z0-9_-]{4,})?"),  # JWT-like
    re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*"),
    re.compile(r"(?i)\b(password|passwd|secret|token|api[_-]?key)\b\s*[:=]\s*\S+"),
    re.compile(r"\b[Aa]uthorization\s*:\s*\S+"),
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
    for pattern in REDACTED_PATTERNS:
        out = pattern.sub("<redacted>", out)
    return out[:400]


def run_probe(executor, key, timeout):
    """Run one fixed probe; returns (result_dict, error_string_or_None)."""
    argv = list(PROBE_ARGS[key])
    try:
        result = executor(argv, timeout)
    except subprocess.TimeoutExpired:
        return None, "probe timed out after %.0fs" % timeout
    except FileNotFoundError:
        if key in OPTIONAL_PROBES:
            return None, "tool not present"
        return None, "required tool missing"
    except OSError as exc:
        return None, "probe could not run: %s" % redact(exc)
    if not isinstance(result, dict):
        return None, "probe returned malformed result"
    exit_code = result.get("exit_code")
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
        return {"id": "os_linux", "status": ERROR, "detail": error}
    fields = redact(result.get("stdout", "")).strip().split()
    if result["exit_code"] != 0 or len(fields) < 3 or fields[0] != "Linux":
        detail = "kernel reports %s" % (" ".join(fields[:2]) or "unknown")
        return {"id": "os_linux", "status": UNSUPPORTED, "value": None,
                "detail": redact(detail)}
    machine = fields[2]
    if machine in SUPPORTED_ARCHS:
        return {"id": "os_linux", "status": SUPPORTED, "value": " ".join(fields[:3]),
                "detail": "Linux on a supported architecture"}
    return {"id": "os_linux", "status": UNSUPPORTED, "value": " ".join(fields[:3]),
            "detail": "architecture %s is outside %s" % (machine, "/".join(SUPPORTED_ARCHS))}


def check_container_engine(executor, timeout):
    docker_version, dv_err = run_probe(executor, "docker_version", timeout)
    podman_info, pi_err = run_probe(executor, "podman_info", timeout)

    engines = []
    engine_ok = None
    docker_server = None
    if docker_version is not None and docker_version["exit_code"] == 0:
        payload = parse_json_output(docker_version)
        server = payload.get("Server") if isinstance(payload, dict) else None
        if isinstance(server, dict) and server.get("Version"):
            docker_server = str(server["Version"])
            engines.append("docker %s" % docker_server)
            engine_ok = "docker"
    if podman_info is not None and podman_info["exit_code"] == 0:
        payload = parse_json_output(podman_info)
        host = payload.get("host") if isinstance(payload, dict) else None
        version = None
        if isinstance(host, dict):
            outputs = host.get("version")
            if isinstance(outputs, dict):
                version = outputs.get("output")
        if version:
            engines.append("podman %s" % str(version))
            if engine_ok is None:
                engine_ok = "podman"

    if engine_ok is None:
        detail_bits = []
        if dv_err:
            detail_bits.append("docker: %s" % dv_err)
        elif docker_version is not None:
            stderr = redact(docker_version.get("stderr", ""))
            detail_bits.append("docker present but daemon unreachable (%s)"
                               % (stderr or "no server response"))
        if pi_err:
            detail_bits.append("podman: %s" % pi_err)
        return {"id": "container_engine", "status": UNSUPPORTED, "value": None,
                "detail": "no usable container engine: %s"
                          % ("; ".join(detail_bits) or "none found")}

    detail = "usable engine: %s" % engines[0]
    if len(engines) > 1:
        detail += " (also found %s)" % ", ".join(engines[1:])
    if engine_ok == "docker":
        info, err = run_probe(executor, "docker_info", timeout)
        if err is None and info["exit_code"] == 0:
            for line in str(info.get("stdout", "")).splitlines():
                if line.strip().startswith("Storage Driver:"):
                    detail += "; storage driver %s" % redact(line.split(":", 1)[1].strip())
                    break
    return {"id": "container_engine", "status": SUPPORTED, "value": engines[0],
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
            cores = sum(1 for line in cpuinfo.splitlines() if line.startswith("processor"))
    if cores is None:
        status = ERROR if probe_failed else UNSUPPORTED
        detail = "cpu count unreadable (nprobe probe failed)" if probe_failed \
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
        return {"id": "memory_capacity", "status": UNSUPPORTED, "value": None,
                "detail": "MemTotal missing or malformed"}
    status = SUPPORTED if total_kb >= MIN_MEM_KB else UNSUPPORTED
    return {"id": "memory_capacity", "status": status, "value": total_kb,
            "detail": "%d MiB total (reference baseline 16 GiB)" % (total_kb // 1024)}


def check_disk(executor, timeout, workspace_path):
    result, error = run_probe(executor, "disk", timeout)
    if error:
        return {"id": "disk_capacity", "status": ERROR, "value": None, "detail": error}
    if result["exit_code"] != 0:
        return {"id": "disk_capacity", "status": ERROR, "value": None,
                "detail": redact(result.get("stderr", "")) or "df failed"}
    available = {}
    for line in str(result.get("stdout", "")).splitlines()[1:]:
        fields = line.split()
        if len(fields) >= 6:
            available[fields[5]] = int(fields[3]) if fields[3].isdigit() else None
    root_kb = available.get("/")
    workspace_kb = available.get(workspace_path)
    if root_kb is None:
        return {"id": "disk_capacity", "status": ERROR, "value": None,
                "detail": "df output did not include /"}
    free_kb = workspace_kb if workspace_kb is not None else root_kb
    status = SUPPORTED if free_kb >= MIN_FREE_KB else UNSUPPORTED
    detail = "%d MiB free on %s (reference baseline 20 GiB)" \
        % (free_kb // 1024, workspace_path if workspace_kb is not None else "/")
    return {"id": "disk_capacity", "status": status, "value": free_kb, "detail": detail}


def nearest_existing(path):
    current = path
    while True:
        if os.path.exists(current):
            return current
        parent = os.path.dirname(current)
        if parent == current:
            return None
        current = parent


def read_ext4_options(procfs_root, sysfs_root, dev_major, dev_minor):
    """Resolve the block device name via sysfs, then read its ext4 options.

    Returns (name, options_content_or_None, error_or_None).
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
    options = read_procfs(procfs_root, os.path.join("fs", "ext4", name, "options"))
    if options is None:
        return name, None, "no /proc/fs/ext4/%s entry (not a mounted ext4 filesystem)" % name
    return name, options, None


def check_storage_qualifier(procfs_root, sysfs_root, workspace_path):
    target = nearest_existing(workspace_path)
    if target is None:
        return {"id": "storage_ext4_qualifier", "status": UNSUPPORTED, "value": None,
                "detail": "workspace path %s does not exist and no ancestor is stat-able"
                          % workspace_path}
    try:
        stat = os.stat(target)
    except OSError:
        return {"id": "storage_ext4_qualifier", "status": ERROR, "value": None,
                "detail": "workspace path %s could not be stat-ed" % workspace_path}
    name, options, error = read_ext4_options(procfs_root, sysfs_root,
                                              os.major(stat.st_dev), os.minor(stat.st_dev))
    if error is not None or options is None:
        status = UNSUPPORTED
        value = name
        detail = error or "ext4 options unreadable"
    else:
        lines = [line.strip() for line in options.splitlines() if line.strip()]
        has_rw = any(line.startswith("rw") for line in lines)
        has_ro = any(line.startswith("ro") for line in lines)
        has_barrier = any(line.startswith("barrier") for line in lines)
        data_entries = [line for line in lines if line.startswith("data=")]
        data_ok = len(data_entries) == 1 and data_entries[0] in ("data=ordered", "data=journal")
        has_nobarrier = any(line.startswith("nobarrier") for line in lines)
        if has_rw and has_barrier and data_ok and not has_ro and not has_nobarrier:
            status, value = SUPPORTED, name
            detail = "device %s meets the on-worker ext4 qualifier" % name
        else:
            status, value = UNSUPPORTED, name
            missing = []
            if not has_rw:
                missing.append("rw")
            if has_ro:
                missing.append("ro present")
            if not has_barrier:
                missing.append("barrier")
            if has_nobarrier:
                missing.append("nobarrier present")
            if not data_ok:
                missing.append("data=ordered|journal")
            detail = "device %s fails the qualifier: %s" % (name, ", ".join(missing))
    return {"id": "storage_ext4_qualifier", "status": status, "value": value,
            "detail": redact(detail)}


def check_tailscale(executor, timeout):
    version, verr = run_probe(executor, "tailscale_version", timeout)
    if verr:
        return {"id": "tailscale", "status": UNSUPPORTED, "value": None,
                "detail": "tailscale not present: %s" % verr}
    if version["exit_code"] != 0:
        return {"id": "tailscale", "status": UNSUPPORTED, "value": None,
                "detail": redact(version.get("stderr", "")) or "tailscale version failed"}
    self_name = None
    online = None
    status, serr = run_probe(executor, "tailscale_status", timeout)
    if serr is None and status["exit_code"] == 0:
        payload = parse_json_output(status)
        if isinstance(payload, dict) and isinstance(payload.get("Self"), dict):
            if payload["Self"].get("DNSName"):
                self_name = str(payload["Self"]["DNSName"]).rstrip(".")
            online = payload["Self"].get("Online")
    stdout_lines = [line for line in str(version.get("stdout", "")).splitlines() if line.strip()]
    version_line = redact(stdout_lines[0] if stdout_lines else "unknown")
    value = self_name or version_line.split()[0]
    detail = "tailscale %s" % version_line
    if self_name:
        detail += " as %s (online=%s)" % (self_name, str(online).lower())
    return {"id": "tailscale", "status": SUPPORTED, "value": value,
            "detail": detail + "; client-side reachability remains unverified"}


def build_report(procfs_root, sysfs_root, workspace_path, timeout, executor, now):
    checks = [
        check_os(executor, timeout),
        check_container_engine(executor, timeout),
        check_capacity(executor, timeout, procfs_root),
        check_memory(procfs_root),
        check_disk(executor, timeout, workspace_path),
        check_storage_qualifier(procfs_root, sysfs_root, workspace_path),
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


def render_text(report):
    lines = ["%s report (schema %d)" % (report["tool"], report["schema"]), ""]
    for check in report["checks"]:
        lines.append("%-24s %-11s %s" % (check["id"], check["status"], check.get("detail", "")))
    summary = report["summary"]
    lines.append("")
    lines.append("verdict: %s (supported %d, unsupported %d, unverified %d, errors %d)"
                 % (summary["verdict"], summary["supported"], summary["unsupported"],
                    summary["unverified"], summary["error"]))
    return "\n".join(lines)


def default_executor(argv, timeout):
    proc = subprocess.run(argv, timeout=timeout, capture_output=True, text=True,
                          check=False, shell=False)
    return {"exit_code": proc.returncode, "stdout": proc.stdout, "stderr": proc.stderr}


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
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT,
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
