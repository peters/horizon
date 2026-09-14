"""Deterministic tests for the read-only Linux host preflight.

Every test uses synthetic procfs/sysfs roots and an injected executor driven
by fixtures. Nothing here invokes a real host tool, and the subprocess module
is patched to fail if any code path falls back to a real spawn. The executor
also enforces the fixed argv allowlist, so a probe regression that interpolates
host values into arguments fails loudly.
"""
import errno
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import preflight  # noqa: E402

NOW = "2026-09-13T00:00:00+00:00"
JWT = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.c3ludA.JHQtZGF0YWxpZw"


def os_fixture(osname="Linux", release="6.1.0", machine="x86_64"):
    return {"stdout": "%s %s %s\n" % (osname, release, machine)}


def docker_ok(version="26.1.4"):
    return {"stdout": json.dumps({"Client": {"Version": version},
                                  "Server": {"Version": version, "OSType": "linux", "Os": "linux"}})}


def docker_daemon_down(stderr="Cannot connect to the Docker daemon at unix:///var/run/docker.sock. "
                              "Is the docker daemon running?"):
    return {"exit_code": 1, "stdout": "", "stderr": stderr}


def podman_ok(version="4.9.0"):
    # `podman info --format '{{.Version.Version}}'` returns one field.
    return {"stdout": version + "\n"}


def podman_client_ok():
    return {"stdout": "podman version 4.9.0\n"}


def podman_socket_ok(path="/run/podman/podman.sock"):
    return {"stdout": path + "\n"}


def nproc_fixture(cores="8"):
    return {"stdout": cores + "\n"}


def df_fixture(root_free_kb="50000000", mounts=(("/var/lib/horizon-workers", "30000000"),)):
    lines = ["Filesystem     1024-blocks      Used Available Capacity Mounted on",
             "/dev/root       100000000  50000000  %s        60%% /" % root_free_kb]
    for mount, free in mounts:
        lines.append("/dev/data      100000000  70000000  %s        70%% %s" % (free, mount))
    return {"stdout": "\n".join(lines) + "\n"}


def tailscale_version_fixture(version="1.78.3"):
    return {"stdout": version + "\n  long version: %s-t0\n" % version}


def tailscale_status_fixture(dns_name="vm.example.ts.net.", online=True):
    return {"stdout": json.dumps({"Self": {"DNSName": dns_name, "Online": online}})}


def docker_context_ok(host="unix:///var/run/docker.sock"):
    return {"stdout": host + "\n"}


def workspace_dir_ok(path, major=259, minor=42):
    return {"stdout": json.dumps({"path": path, "major": major, "minor": minor}) + "\n"}


DEFAULT_FIXTURE = {
    "os": os_fixture(),
    "docker_version": docker_ok(),
    "docker_info": {"stdout": "overlay2\n"},
    "docker_context": docker_context_ok(),
    "cores": nproc_fixture(),
    "disk": df_fixture(),
    "tailscale_version": tailscale_version_fixture(),
    "tailscale_status": tailscale_status_fixture(),
}


def meminfo(total_kb=33554432):
    return "MemTotal:       %d kB\nMemFree:         1000000 kB\n" % total_kb


def build_roots(tmp, meminfo_text=None, cpuinfo_text=None, ext4_options=None,
                block_exists=True):
    procfs = os.path.join(tmp, "procfs")
    sysfs = os.path.join(tmp, "sysfs")
    os.makedirs(os.path.join(procfs), exist_ok=True)
    os.makedirs(os.path.join(sysfs, "dev", "block"), exist_ok=True)
    if meminfo_text is not None:
        with open(os.path.join(procfs, "meminfo"), "w") as handle:
            handle.write(meminfo_text)
    if cpuinfo_text is not None:
        with open(os.path.join(procfs, "cpuinfo"), "w") as handle:
            handle.write(cpuinfo_text)
    block_id = None
    if block_exists:
        block_id = "259:42"
        target = os.path.join(sysfs, "devices", "fake", "nvme0n1p2")
        os.makedirs(target, exist_ok=True)
        link = os.path.join(sysfs, "dev", "block", block_id)
        if os.path.lexists(link):
            os.unlink(link)
        os.symlink(os.path.join("../../../devices", "fake", "nvme0n1p2"), link)
    if ext4_options is not None:
        device_dir = os.path.join(procfs, "fs", "ext4", "nvme0n1p2")
        os.makedirs(device_dir, exist_ok=True)
        with open(os.path.join(device_dir, "options"), "w") as handle:
            handle.write(ext4_options)
    return procfs, sysfs, block_id


EXT4_OK = "rw\nbsddf\nbarrier\ndata=ordered\n"


class Harness(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.real_popen = subprocess.Popen
        for name in ("Popen", "run"):
            patcher = mock.patch.object(subprocess, name, side_effect=AssertionError(
                "no real subprocess allowed in unit tests"))
            patcher.start()
            self.addCleanup(patcher.stop)
        saved = {var: os.environ[var] for var in preflight.ENDPOINT_VARS if var in os.environ}
        for var in preflight.ENDPOINT_VARS:
            os.environ.pop(var, None)

        def restore_endpoint_vars():
            for var in preflight.ENDPOINT_VARS:
                os.environ.pop(var, None)
            os.environ.update(saved)

        self.addCleanup(restore_endpoint_vars)

    def executor_for(self, fixture):
        seen = []
        queues = {key: list(value) for key, value in fixture.items()
                  if isinstance(value, list)}

        def executor(argv, timeout):
            seen.append(list(argv))
            for key, args in preflight.PROBE_ARGS.items():
                args = list(args)
                matches = list(argv) == args
                if key in ("disk", "workspace_dir", "podman_socket") and list(argv[:len(args)]) == args and len(argv) in (len(args), len(args) + 1):
                    matches = True
                if key in ("docker_version", "docker_info") and len(argv) == len(args) + 2 and argv[:1] == args[:1] and argv[1] == "--host" and argv[2].startswith("unix://") and argv[3:] == args[1:]:
                    matches = True
                if key == "podman_info" and list(argv[:len(args)]) == args and list(argv[len(args)+1:]) == list(preflight.PROBE_ARGS["podman_info_tail"]):
                    matches = True
                if matches:
                    if key in queues:
                        if not queues[key]:
                            raise FileNotFoundError(argv[0])
                        entry = queues[key].pop(0)
                    else:
                        entry = fixture.get(key)
                    if entry is None and key == "podman_socket":
                        return {"exit_code": 1, "stdout": "", "stderr": ""}
                    if entry is None and key == "workspace_dir":
                        target, problem = preflight.workspace_directory(argv[-1])
                        if problem:
                            code = 2 if "not a directory" in problem else 3
                            return {"exit_code": code, "stdout": "", "stderr": problem}
                        st = os.stat(target)
                        return {"exit_code": 0,
                                "stdout": json.dumps({
                                    "path": target,
                                    "major": os.major(st.st_dev),
                                    "minor": os.minor(st.st_dev),
                                }) + "\n",
                                "stderr": ""}
                    if entry is None:
                        raise FileNotFoundError
                    if entry.get("timeout"):
                        raise subprocess.TimeoutExpired(args, timeout)
                    return {"exit_code": entry.get("exit_code", 0),
                            "stdout": entry.get("stdout", ""),
                            "stderr": entry.get("stderr", "")}
            raise AssertionError("argv outside the fixed allowlist: %r" % (argv,))

        executor.seen = seen
        return executor

    def run_main(self, fixture, workspace=None, meminfo_text=meminfo(),
                 cpuinfo_text=None, ext4_options=EXT4_OK, block_exists=True):
        procfs, sysfs, block_id = build_roots(self.tmp.name, meminfo_text, cpuinfo_text,
                                               ext4_options, block_exists)
        if workspace is None:
            workspace = os.path.join(self.tmp.name, "ws")
            os.makedirs(workspace, exist_ok=True)
        target = preflight.nearest_existing(workspace)
        if target is None:
            target = os.path.dirname(os.path.abspath(workspace))
        st = os.stat(target)
        real_block = "%d:%d" % (os.major(st.st_dev), os.minor(st.st_dev))
        # Point the synthetic block entry at the workspace's real device id.
        if block_exists:
            link = os.path.join(self.tmp.name, "sysfs", "dev", "block", real_block)
            if os.path.lexists(link):
                os.unlink(link)
            os.symlink(os.path.join(self.tmp.name, "sysfs", "devices", "fake", "nvme0n1p2"),
                       link)
        argv = ["--procfs-root", procfs, "--sysfs-root", sysfs, "--workspace-path",
                workspace, "--now", NOW, "--json"]
        executor = self.executor_for(fixture)
        import io
        from contextlib import redirect_stdout
        buffer = io.StringIO()
        with redirect_stdout(buffer):
            code = preflight.main(argv, executor=executor, now=NOW)
        report = json.loads(buffer.getvalue())
        return code, report, executor


