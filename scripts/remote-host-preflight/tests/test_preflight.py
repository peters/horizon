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


class BytecodeContract(unittest.TestCase):
    def test_import_disables_bytecode(self):
        self.assertTrue(sys.dont_write_bytecode)

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


class PreflightVerdicts(Harness):
    def test_happy_path_docker_supported(self):
        code, report, _ = self.run_main(dict(DEFAULT_FIXTURE))
        self.assertEqual(code, 0)
        self.assertEqual(report["summary"]["verdict"], "supported")
        self.assertEqual(report["summary"]["unsupported"], 0)
        self.assertEqual(report["summary"]["unverified"], 3)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["os_linux"]["status"], "supported")
        self.assertEqual(by_id["container_engine"]["value"], "docker 26.1.4")
        self.assertEqual(by_id["container_storage_driver"]["status"], "supported")
        self.assertEqual(by_id["container_storage_driver"]["value"], "overlay2")
        self.assertEqual(by_id["storage_ext4_qualifier"]["value"], "nvme0n1p2")
        self.assertEqual(by_id["tailscale"]["value"], "vm.example.ts.net")

    def test_storage_qualifier_value_is_redacted(self):
        fixture = dict(DEFAULT_FIXTURE)
        with mock.patch.object(
                preflight, "read_ext4_options",
                return_value=("token=supersecretvalue", EXT4_OK.encode(), None)):
            _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn("supersecretvalue", text)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["storage_ext4_qualifier"]["value"], "<redacted>")

    def test_aarch64_podman_only_supported(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["os"] = os_fixture(machine="aarch64")
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = podman_socket_ok()
        fixture["podman_info"] = podman_ok()
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["os_linux"]["status"], "supported")
        self.assertEqual(by_id["container_engine"]["value"], "podman 4.9.0")

    def test_darwin_unsupported(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["os"] = os_fixture(osname="Darwin", machine="arm64")
        code, report, executor = self.run_main(
            fixture, meminfo_text=None, cpuinfo_text=None,
            ext4_options=None, block_exists=False)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["os_linux"]["status"], "unsupported")
        self.assertEqual(by_id["os_linux"]["value"], "Darwin 6.1.0 arm64")
        self.assertIn("arm64", by_id["os_linux"]["detail"])
        self.assertEqual(report["summary"]["verdict"], "unsupported")
        self.assertEqual(report["summary"]["error"], 0)
        for check_id in ("container_engine", "container_storage_driver",
                         "cpu_capacity", "memory_capacity",
                         "disk_capacity", "storage_ext4_qualifier"):
            self.assertEqual(by_id[check_id]["status"], "unverified", check_id)
        self.assertFalse(any(argv and argv[0] == "docker" for argv in executor.seen))
        self.assertFalse(any(argv and argv[0] == "df" for argv in executor.seen))

    def test_unsupported_architecture(self):
        for machine in ("armv7l", "riscv64", "i686"):
            fixture = dict(DEFAULT_FIXTURE)
            fixture["os"] = os_fixture(machine=machine)
            _, report, _ = self.run_main(fixture)
            by_id = {check["id"]: check for check in report["checks"]}
            self.assertEqual(by_id["os_linux"]["status"], "unsupported", machine)


class EngineFailures(Harness):
    def test_no_engine_found(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture.pop("podman_info", None)
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("no usable container engine", by_id["container_engine"]["detail"])

    def test_docker_daemon_access_denied(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = docker_daemon_down(
            "Got permission denied while trying to connect to the Docker daemon "
            "socket at unix:///var/run/docker.sock")
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("docker present but probe failed", by_id["container_engine"]["detail"])
        self.assertIn("permission denied", by_id["container_engine"]["detail"].lower())

    def test_podman_skipped_when_local_service_is_not_running(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_info"] = podman_ok()
        code, report, executor = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("podman local service is not running", by_id["container_engine"]["detail"])
        self.assertIn(["podman", "--version"], executor.seen)
        self.assertFalse(any(argv and argv[0] == "podman" and "info" in argv
                             for argv in executor.seen))

    def test_podman_missing_binary_is_tool_not_present(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        code, report, executor = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("podman: tool not present", by_id["container_engine"]["detail"])
        self.assertFalse(any(argv and argv[0] == "podman" and "info" in argv
                             for argv in executor.seen))

    def test_podman_nonzero_info_preserves_stderr(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = podman_socket_ok()
        fixture["podman_info"] = {
            "exit_code": 1, "stdout": "", "stderr": "permission denied"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("permission denied", by_id["container_engine"]["detail"])
        self.assertNotIn("malformed version", by_id["container_engine"]["detail"])

    def test_podman_empty_version_is_unusable(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = podman_socket_ok()
        fixture["podman_info"] = {"stdout": "\n"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")

    def test_podman_multiline_version_is_unusable(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = podman_socket_ok()
        fixture["podman_info"] = {"stdout": "4.9.0\nWARN: extra line\n"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")

    def test_docker_non_string_version_is_unusable(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = {"stdout": json.dumps(
            {"Server": {"Version": ["26.1.4"], "OSType": "linux"}})}
        fixture.pop("podman_info", None)
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")

    def test_bearer_authorization_line_is_fully_redacted(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = docker_daemon_down(
            "error: Authorization: Bearer supersecrettok value")
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        detail = by_id["container_engine"]["detail"]
        self.assertNotIn("supersecrettok", detail)
        self.assertNotIn("Bearer", detail)
        self.assertIn("<redacted>", detail)

    def test_authorization_equals_bearer_is_fully_redacted(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = docker_daemon_down(
            "error: Authorization=Bearer supersecrettoken")
        _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn("supersecrettoken", text)
        self.assertNotIn("Bearer", text)
        self.assertIn("<redacted>", text)

    def test_disk_selects_longest_mount_ancestor(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["disk"] = {"stdout":
            "Filesystem     1024-blocks      Used Available Capacity Mounted on\n"
            "/dev/root        1000000000  800000000   200000000     80% /\n"
            "/dev/data        2000000000 1900000000    41943040    98% /mnt/workspaces\n"}
        fixture["workspace_dir"] = workspace_dir_ok("/mnt/workspaces")
        code, report, _ = self.run_main(fixture, workspace="/mnt/workspaces/job")
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        # /mnt/workspaces (40 GiB free) is the workspace's real filesystem,
        # not / (200 GB free).
        self.assertEqual(by_id["disk_capacity"]["value"], 41943040)
        self.assertIn("/mnt/workspaces", by_id["disk_capacity"]["detail"])

    def test_docker_malformed_json_falls_through_to_unsupported(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = {"stdout": "this is not json {{{"}
        fixture.pop("docker_info")
        fixture.pop("podman_info", None)
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")

    def test_remote_docker_endpoint_is_rejected(self):
        fixture = dict(DEFAULT_FIXTURE)
        with mock.patch.dict(os.environ, {"DOCKER_HOST": "tcp://remote-daemon:2376"}):
            code, report, executor = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("endpoint is remote", by_id["container_engine"]["detail"])
        self.assertIn("tcp://", by_id["container_engine"]["detail"])
        self.assertNotIn(list(preflight.PROBE_ARGS["docker_version"]), executor.seen)
        self.assertNotIn(list(preflight.PROBE_ARGS["docker_info"]), executor.seen)

    def test_local_docker_host_skips_context_inspect(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_context"] = {
            "exit_code": 1, "stdout": "", "stderr": "no context support"}
        with mock.patch.dict(os.environ, {"DOCKER_HOST": "unix:///var/run/docker.sock"}):
            code, report, executor = self.run_main(fixture)
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "supported")
        self.assertFalse(any("context" in argv for argv in executor.seen))

    def test_local_unix_docker_endpoint_is_accepted(self):
        fixture = dict(DEFAULT_FIXTURE)
        with mock.patch.dict(os.environ, {"DOCKER_HOST": "unix:///var/run/docker.sock"}):
            code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "supported")

    def test_rejected_docker_is_not_listed_as_also_found(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = {"stdout": json.dumps(
            {"Server": {"Version": "26.1.4", "OSType": "windows"}})}
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = podman_socket_ok()
        fixture["podman_info"] = podman_ok()
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["value"], "podman 4.9.0")
        self.assertNotIn("also found", by_id["container_engine"]["detail"])

    def test_non_linux_docker_server_os_is_rejected(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = {"stdout": json.dumps(
            {"Client": {"Version": "26.1.4"},
             "Server": {"Version": "26.1.4", "OSType": "windows"}})}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("not linux", by_id["container_engine"]["detail"])

    def test_remote_docker_context_is_rejected(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_context"] = docker_context_ok(host="tcp://remote-daemon:2376")
        code, report, executor = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("context Host=", by_id["container_engine"]["detail"])
        self.assertIn("tcp://", by_id["container_engine"]["detail"])
        self.assertNotIn(list(preflight.PROBE_ARGS["docker_version"]), executor.seen)
        self.assertNotIn(list(preflight.PROBE_ARGS["docker_info"]), executor.seen)

    def test_missing_docker_server_ostype_is_rejected(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = {"stdout": json.dumps(
            {"Client": {"Version": "26.1.4"}, "Server": {"Version": "26.1.4"}})}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("OSType missing", by_id["container_engine"]["detail"])

    def test_docker_version_os_field_is_accepted(self):
        # Live `docker version --format json` on Engine 29 uses Server.Os,
        # not Server.OSType.
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = {"stdout": json.dumps(
            {"Client": {"Version": "29.7.2"},
             "Server": {"Version": "29.7.2", "Os": "linux"}})}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "supported")
        self.assertEqual(by_id["container_engine"]["value"], "docker 29.7.2")

    def test_podman_container_connection_is_rejected(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_info"] = podman_ok()
        with mock.patch.dict(os.environ, {"CONTAINER_CONNECTION": "remote-worker"}):
            code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("CONTAINER_CONNECTION", by_id["container_engine"]["detail"])

    def test_podman_named_connection_is_rejected(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_info"] = podman_ok()
        with mock.patch.dict(os.environ, {"PODMAN_CONNECTION": "remote-worker"}):
            code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("PODMAN_CONNECTION", by_id["container_engine"]["detail"])

    def test_storage_driver_unverified_when_info_fails(self):
        cases = {
            "timeout": {"docker_info": {"timeout": True}},
            "nonzero": {"docker_info": {"exit_code": 1, "stdout": "", "stderr": "denied"}},
            "no_line": {"docker_info": {"stdout": ""}},
            "multiline": {"docker_info": {"stdout": "overlay2\nWARNING: extra\n"}},
        }
        for label, overrides in cases.items():
            with self.subTest(label=label):
                fixture = dict(DEFAULT_FIXTURE)
                fixture.update(overrides)
                code, report, _ = self.run_main(fixture)
                self.assertEqual(code, 0, label)
                by_id = {check["id"]: check for check in report["checks"]}
                self.assertEqual(by_id["container_engine"]["status"], "supported", label)
                self.assertEqual(by_id["container_storage_driver"]["status"], "unverified", label)

    def test_engine_version_strings_are_redacted(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = {"stdout": json.dumps(
            {"Server": {"Version": JWT, "OSType": "linux"}})}
        _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn(JWT, text)
        self.assertIn("<redacted>", text)

    def test_uname_truncated_is_error_not_rejection(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["os"] = {"stdout": "Linux\n"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["os_linux"]["status"], "error")
        self.assertIn("truncated", by_id["os_linux"]["detail"])

    def test_uname_timeout_includes_value_none(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["os"] = {"timeout": True}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["os_linux"]["status"], "error")
        self.assertIsNone(by_id["os_linux"].get("value"))

    def test_workspace_is_resolved_once_for_disk_and_storage(self):
        _, _, executor = self.run_main(dict(DEFAULT_FIXTURE))
        prefix = list(preflight.PROBE_ARGS["workspace_dir"])
        calls = [argv for argv in executor.seen if argv[:len(prefix)] == prefix]
        self.assertEqual(len(calls), 1)

    def test_workspace_helper_disables_bytecode(self):
        self.assertIn("-B", preflight.PROBE_ARGS["workspace_dir"])
        self.assertIn("-B", preflight.PROBE_ARGS["podman_socket"])

    def test_bounded_communicate_kills_runaway_output(self):
        proc = self.real_popen(
            [sys.executable, "-B", "-c", "import sys; sys.stdout.write('x'*200000)"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        _stdout, _stderr, overflow = preflight.bounded_communicate(proc, 5, 1024)
        self.assertIsNotNone(overflow)
        self.assertIn("exceeded", overflow)
        self.assertIsNotNone(proc.poll())

    def test_default_executor_times_out_before_hanging(self):
        pidfile = os.path.join(self.tmp.name, "probe.pid")
        script = (
            "import os,time\n"
            "open(%r,'w').write(str(os.getpid()))\n"
            "time.sleep(5)\n"
        ) % pidfile
        with mock.patch.object(subprocess, "Popen", self.real_popen):
            with self.assertRaises(subprocess.TimeoutExpired):
                preflight.default_executor(
                    [sys.executable, "-B", "-c", script], 0.2)
        with open(pidfile, encoding="utf-8") as handle:
            probe_pid = int(handle.read().strip())
        deadline = time.monotonic() + 2
        alive = True
        while time.monotonic() < deadline:
            try:
                os.kill(probe_pid, 0)
            except OSError:
                alive = False
                break
            time.sleep(0.05)
        self.assertFalse(alive)

    def test_setsid_failure_does_not_run_probe(self):
        with mock.patch.object(os, "setsid", side_effect=OSError("denied")):
            with mock.patch.object(subprocess, "Popen", self.real_popen):
                with self.assertRaises(OSError) as caught:
                    preflight.default_executor(
                        [sys.executable, "-B", "-c", "print(1)"], 1.0)
        self.assertIn("process session", str(caught.exception))

    def test_default_executor_missing_tool_is_file_not_found(self):
        with mock.patch.object(subprocess, "Popen", self.real_popen):
            with self.assertRaises(FileNotFoundError):
                preflight.default_executor(
                    ["/nonexistent-horizon-preflight-probe"], 1.0)

    def test_timeout_kills_child_that_inherited_stdout(self):
        pidfile = os.path.join(self.tmp.name, "orphan.pid")
        script = (
            "import os,time\n"
            "child=os.fork()\n"
            "if child==0:\n"
            "    open(%r,'w').write(str(os.getpid()))\n"
            "    time.sleep(30)\n"
            "    os._exit(0)\n"
            "os._exit(0)\n"
        ) % pidfile
        with mock.patch.object(subprocess, "Popen", self.real_popen):
            with self.assertRaises(subprocess.TimeoutExpired):
                preflight.default_executor(
                    [sys.executable, "-B", "-c", script], 0.4)
        deadline = time.monotonic() + 2
        grandchild = None
        while time.monotonic() < deadline:
            if os.path.exists(pidfile):
                with open(pidfile, encoding="utf-8") as handle:
                    text = handle.read().strip()
                if text.isdigit():
                    grandchild = int(text)
                    break
            time.sleep(0.05)
        self.assertIsNotNone(grandchild)
        alive = True
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            try:
                os.kill(grandchild, 0)
            except OSError:
                alive = False
                break
            time.sleep(0.05)
        self.assertFalse(alive)

    def test_sysfs_device_name_matches_worker_gate(self):
        self.assertTrue(preflight.sysfs_device_name_ok("nvme0n1p2"))
        self.assertFalse(preflight.sysfs_device_name_ok("nvme0n1p2:0"))
        self.assertFalse(preflight.sysfs_device_name_ok("nvme0n1p2."))
        self.assertFalse(preflight.sysfs_device_name_ok("nvme0n1p2 "))
        self.assertFalse(preflight.sysfs_device_name_ok("a" * 256))
        self.assertFalse(preflight.sysfs_device_name_ok(".git"))
        self.assertFalse(preflight.sysfs_device_name_ok("id_rsa"))
        self.assertFalse(preflight.sysfs_device_name_ok("foo.env"))
        self.assertTrue(preflight.sysfs_device_name_ok("a" * 255))
        self.assertFalse(preflight.sysfs_device_name_ok("nvme\udc80n1"))
        self.assertFalse(preflight.sysfs_device_name_ok("nvme\x85n1"))

    def test_overflow_kills_probe_descendants(self):
        pidfile = os.path.join(self.tmp.name, "grandchild.pid")
        script = (
            "import subprocess,sys\n"
            "child=subprocess.Popen(['sleep','30'])\n"
            "open(%r,'w').write(str(child.pid))\n"
            "sys.stdout.write('x'*200000)\n"
        ) % pidfile
        with mock.patch.object(subprocess, "Popen", self.real_popen):
            result = preflight.default_executor(
                [sys.executable, "-B", "-c", script], 2.0)
        self.assertTrue(result.get("output_exceeded"))
        with open(pidfile, encoding="utf-8") as handle:
            grandchild = int(handle.read().strip())
        deadline = time.monotonic() + 2
        alive = True
        while time.monotonic() < deadline:
            try:
                os.kill(grandchild, 0)
            except OSError:
                alive = False
                break
            time.sleep(0.05)
        self.assertFalse(alive)

    def test_overflow_kills_child_that_inherited_stdout(self):
        pidfile = os.path.join(self.tmp.name, "overflow-orphan.pid")
        script = (
            "import os,sys,time\n"
            "child=os.fork()\n"
            "if child==0:\n"
            "    open(%r,'w').write(str(os.getpid()))\n"
            "    sys.stdout.write('x'*200000)\n"
            "    sys.stdout.flush()\n"
            "    time.sleep(30)\n"
            "    os._exit(0)\n"
            "os._exit(0)\n"
        ) % pidfile
        with mock.patch.object(subprocess, "Popen", self.real_popen):
            result = preflight.default_executor(
                [sys.executable, "-B", "-c", script], 2.0)
        self.assertTrue(result.get("output_exceeded"))
        deadline = time.monotonic() + 2
        grandchild = None
        while time.monotonic() < deadline:
            if os.path.exists(pidfile):
                with open(pidfile, encoding="utf-8") as handle:
                    text = handle.read().strip()
                if text.isdigit():
                    grandchild = int(text)
                    break
            time.sleep(0.05)
        self.assertIsNotNone(grandchild)
        alive = True
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            try:
                os.kill(grandchild, 0)
            except OSError:
                alive = False
                break
            time.sleep(0.05)
        self.assertFalse(alive)

    def test_nul_output_does_not_timeout_watchdog(self):
        with mock.patch.object(subprocess, "Popen", self.real_popen):
            result = preflight.default_executor(
                [sys.executable, "-B", "-c",
                 "import sys; sys.stdout.buffer.write(b'\\x00'*50000)"],
                2.0)
        self.assertFalse(result.get("output_exceeded"))
        self.assertEqual(result.get("exit_code"), 0)

    def test_stale_rootless_podman_socket_falls_through(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = {
            "stdout": "/run/user/1000/podman/podman.sock\n/run/podman/podman.sock\n"}
        fixture["podman_info"] = [{"stdout": "\n"}, podman_ok()]
        with mock.patch.dict(os.environ, {"XDG_RUNTIME_DIR": "/run/user/1000"}):
            code, report, executor = self.run_main(fixture)
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["value"], "podman 4.9.0")
        info_urls = [argv[3] for argv in executor.seen
                     if argv[:3] == ["podman", "--remote=true", "--url"]]
        self.assertEqual(info_urls, [
            "unix:///run/user/1000/podman/podman.sock",
            "unix:///run/podman/podman.sock",
        ])

    def test_podman_probe_forces_local_mode(self):
        self.assertEqual(preflight.PROBE_ARGS["podman_info"][:2], ["podman", "--remote=true"])
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = podman_socket_ok()
        fixture["podman_info"] = podman_ok()
        _, _, executor = self.run_main(fixture)
        self.assertIn(
            ["podman", "--remote=true", "--url", "unix:///run/podman/podman.sock",
             "info", "--format", "{{.Version.Version}}"],
            executor.seen)

    def test_podman_socket_discovery_timeout_is_bounded(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = {"timeout": True}
        code, report, executor = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("timed out", by_id["container_engine"]["detail"])
        self.assertIn(["podman", "--version"], executor.seen)
        self.assertFalse(any(argv and argv[0] == "podman" and "info" in argv
                             for argv in executor.seen))

    def test_podman_socket_helper_rejects_unexpected_path(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = {"stdout": "/tmp/evil.sock\n"}
        fixture["podman_info"] = podman_ok()
        code, report, executor = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("unexpected path", by_id["container_engine"]["detail"])
        self.assertIn(["podman", "--version"], executor.seen)
        self.assertFalse(any(argv and argv[0] == "podman" and "info" in argv
                             for argv in executor.seen))

    def test_select_mount_point_does_not_realpath(self):
        with mock.patch.object(os.path, "realpath",
                               side_effect=AssertionError("realpath in main process")):
            self.assertEqual(
                preflight.select_mount_point(["/", "/mnt/data"], "/mnt/data/workers"),
                "/mnt/data")

    def test_decode_probe_output_replaces_invalid_utf8(self):
        self.assertIn("\ufffd", preflight.decode_probe_output(b"ok\xffend"))

    def test_uname_nonzero_exit_is_error_not_rejection(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["os"] = {"exit_code": 1, "stdout": "", "stderr": "uname: boom"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["os_linux"]["status"], "error")

    def test_os_detail_reports_release_and_arch(self):
        code, report, _ = self.run_main(dict(DEFAULT_FIXTURE))
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(code, 0)
        self.assertIn("6.1.0", by_id["os_linux"]["detail"])
        self.assertIn("x86_64", by_id["os_linux"]["detail"])

    def test_engine_probe_timeout(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = {"timeout": True}
        fixture.pop("docker_info")
        fixture.pop("podman_info", None)
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("timed out", by_id["container_engine"]["detail"])


class MalformedInputs(Harness):
    def test_nproc_malformed_falls_back_to_cpuinfo(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["cores"] = {"stdout": "not-a-number\n"}
        code, report, _ = self.run_main(fixture, cpuinfo_text="processor\t: 0\nprocessor\t: 1\n")
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["cpu_capacity"]["value"], 2)
        self.assertEqual(by_id["cpu_capacity"]["status"], "unsupported")

    def test_nproc_non_positive_falls_back_to_cpuinfo(self):
        cpuinfo = "processor\t: 0\nprocessor\t: 1\nprocessor\t: 2\nprocessor\t: 3\n"
        for value in ("0", "-1"):
            with self.subTest(value=value):
                fixture = dict(DEFAULT_FIXTURE)
                fixture["cores"] = {"stdout": value + "\n"}
                code, report, _ = self.run_main(fixture, cpuinfo_text=cpuinfo)
                self.assertEqual(code, 0, value)
                by_id = {check["id"]: check for check in report["checks"]}
                self.assertEqual(by_id["cpu_capacity"]["value"], 4, value)
                self.assertEqual(by_id["cpu_capacity"]["status"], "supported", value)

    def test_cpuinfo_without_processor_records_is_error(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["cores"] = {"exit_code": 1, "stdout": "", "stderr": "nproc failed"}
        code, report, _ = self.run_main(fixture, cpuinfo_text="vendor_id\t: GenuineIntel\n")
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["cpu_capacity"]["status"], "error")

    def test_nproc_missing_and_no_cpuinfo(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["cores"] = {"exit_code": 1, "stdout": "", "stderr": "boom"}
        code, report, _ = self.run_main(fixture, cpuinfo_text=None)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["cpu_capacity"]["status"], "error")

    def test_meminfo_missing_total_is_error_not_rejection(self):
        # An unreadable MemTotal is a probe failure (incomplete report),
        # not evidence of insufficient memory.
        fixture = dict(DEFAULT_FIXTURE)
        code, report, _ = self.run_main(fixture, meminfo_text="MemFree: 100 kB\n")
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["memory_capacity"]["status"], "error")

    def test_meminfo_overflow_is_error(self):
        huge = "MemTotal: 16777216 kB\n" + ("x" * (preflight.MAX_PROBE_OUTPUT_BYTES + 1))
        code, report, _ = self.run_main(dict(DEFAULT_FIXTURE), meminfo_text=huge)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["memory_capacity"]["status"], "error")

    def test_memtotal_negative_or_wrong_unit_is_error(self):
        cases = (
            "MemTotal: -1 kB\n",
            "MemTotal: 16777216 bytes\n",
            "MemTotal: 0 kB\n",
        )
        for text in cases:
            with self.subTest(text=text.strip()):
                code, report, _ = self.run_main(dict(DEFAULT_FIXTURE), meminfo_text=text)
                self.assertEqual(code, 2)
                by_id = {check["id"]: check for check in report["checks"]}
                self.assertEqual(by_id["memory_capacity"]["status"], "error")

    def test_meminfo_absent(self):
        code, report, _ = self.run_main(dict(DEFAULT_FIXTURE), meminfo_text=None)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["memory_capacity"]["status"], "error")

    def test_df_missing_root(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["disk"] = {"stdout": "Filesystem  1024-blocks Used Available Capacity Mounted on\n"
                                     "/dev/data  100000 10000 90000 10% /data\n"}
        workspace = os.path.join(self.tmp.name, "token=supersecretvalue")
        os.makedirs(workspace)
        code, report, _ = self.run_main(fixture, workspace=workspace)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["disk_capacity"]["status"], "error")
        self.assertNotIn("supersecretvalue", json.dumps(report))

    def test_df_non_numeric_free(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["disk"] = {"stdout": "Filesystem  1024-blocks Used Available Capacity Mounted on\n"
                                     "/dev/root  100000 10000 ?? 10% /\n"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["disk_capacity"]["status"], "error")

    def test_df_oversized_digit_field_is_error(self):
        huge = "9" * (preflight.MAX_NONNEG_INT_DIGITS + 1)
        self.assertIsNone(preflight.parse_nonneg_int(huge))
        self.assertIsNone(preflight.parse_nonneg_int("١٢٣"))
        fixture = dict(DEFAULT_FIXTURE)
        fixture["disk"] = {"stdout":
            "Filesystem     1024-blocks      Used Available Capacity Mounted on\n"
            "/dev/root       100000000  50000000  %s        60%% /\n" % huge}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["disk_capacity"]["status"], "error")
        self.assertIn("malformed free value", by_id["disk_capacity"]["detail"])

    def test_deeply_nested_json_is_a_parse_failure(self):
        payload = "[" * 10000 + "]" * 10000
        self.assertIsNone(preflight.parse_json_output({"stdout": payload}))
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = {"stdout": payload}
        fixture.pop("podman_info", None)
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")


class StorageQualifier(Harness):
    def variant(self, options):
        return self.run_main(dict(DEFAULT_FIXTURE), ext4_options=options)

    def test_qualifier_pass(self):
        code, report, _ = self.variant("rw\nbarrier\ndata=journal\n")
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["storage_ext4_qualifier"]["status"], "supported")

    def test_data_writeback_fails(self):
        _, report, _ = self.variant("rw\nbarrier\ndata=writeback\n")
        by_id = {check["id"]: check for check in report["checks"]}
        check = by_id["storage_ext4_qualifier"]
        self.assertEqual(check["status"], "unsupported")
        self.assertIn("exactly one data=ordered or data=journal", check["detail"])

    def test_read_only_mount_fails(self):
        _, report, _ = self.variant("ro\nbarrier\ndata=ordered\n")
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["storage_ext4_qualifier"]["status"], "unsupported")

    def test_nobarrier_fails(self):
        _, report, _ = self.variant("rw\nnobarrier\ndata=ordered\n")
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["storage_ext4_qualifier"]["status"], "unsupported")

    def test_gate_rejects_rust_contract_violations(self):
        # Mirrors storage.rs journal_contract tests: each must fail.
        bad = {
            "rwx": "rwx\nbarrier\ndata=ordered\n",             # prefix, not exact token
            "space": "rw\n barrier\ndata=ordered\n",           # space in line
            "duplicate": "rw\nbarrier\nbarrier\ndata=ordered\n",
            "no_newline": "rw\nbarrier\ndata=ordered",
            "crlf": "rw\nbarrier\ndata=ordered\r\n",
            "ro": "rw\nbarrier\ndata=ordered\nro\n",
            "two_data": "rw\nbarrier\ndata=ordered\ndata=journal\n",
            "empty_line": "rw\nbarrier\ndata=ordered\n\n",
        }
        for label, options in bad.items():
            with self.subTest(label=label):
                _, report, _ = self.variant(options)
                by_id = {check["id"]: check for check in report["checks"]}
                self.assertEqual(by_id["storage_ext4_qualifier"]["status"], "unsupported", label)
        # over the 4096-byte cap
        _, report, _ = self.variant("rw\nbarrier\ndata=ordered\n" + "x" * 4100 + "\n")
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["storage_ext4_qualifier"]["status"], "unsupported")

    def test_gate_unit_rejects_and_accepts(self):
        bad = [b"rwx\nbarrier\ndata=ordered\n", b"rw\n barrier\ndata=ordered\n",
               b"rw\nbarrier\nbarrier\ndata=ordered\n", b"rw\nbarrier\ndata=ordered",
               b"rw\nbarrier\ndata=ordered\r\n", b"rw\nbarrier\ndata=writeback\n",
               b"rw\nbarrier\ndata=ordered\nro\n", b"rw\nbarrier\ndata=ordered\nnobarrier\n",
               b"rw\nbarrier\ndata=ordered\ndata=journal\n", b"\n",
               b"rw\nbarrier\ndata=ordered\n" + b"x" * 4100]
        for raw in bad:
            with self.subTest(raw=raw):
                self.assertTrue(preflight.ext4_qualifier_problems(raw), "accepted %r" % raw)
        for raw in (b"rw\nbarrier\ndata=ordered\n", b"rw\nbarrier\ndata=journal\n"):
            self.assertEqual(preflight.ext4_qualifier_problems(raw), [])

    def test_ext4_options_io_failure_is_error(self):
        procfs, sysfs, _ = build_roots(self.tmp.name, meminfo(), None, EXT4_OK, True)
        options_path = os.path.join(procfs, "fs", "ext4", "nvme0n1p2", "options")
        os.remove(options_path)
        os.mkdir(options_path)
        _name, _raw, error = preflight.read_ext4_options(procfs, sysfs, 259, 42)
        self.assertIsNotNone(error)
        self.assertIn("unreadable", error)

    def test_not_ext4(self):
        _, report, _ = self.run_main(dict(DEFAULT_FIXTURE), ext4_options=None)
        by_id = {check["id"]: check for check in report["checks"]}
        check = by_id["storage_ext4_qualifier"]
        self.assertEqual(check["status"], "unsupported")
        self.assertIn("not a mounted ext4 filesystem", check["detail"])

    def test_block_device_unresolvable(self):
        _, report, _ = self.run_main(dict(DEFAULT_FIXTURE), block_exists=False)
        by_id = {check["id"]: check for check in report["checks"]}
        check = by_id["storage_ext4_qualifier"]
        self.assertEqual(check["status"], "unsupported")
        self.assertIn("not resolvable via sysfs", check["detail"])

    def test_missing_workspace_path_uses_nearest_ancestor(self):
        fixture = dict(DEFAULT_FIXTURE)
        code, report, _ = self.run_main(fixture,
                                        workspace=os.path.join(self.tmp.name, "missing", "deep"))
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["storage_ext4_qualifier"]["status"], "supported")


class DiskAndCapacity(Harness):
    def test_symlinked_workspace_selects_target_mount(self):
        # A dangling workspace symlink must be judged on the target-side
        # mount and the same resolved ancestor the storage qualifier stats.
        target_root = os.path.join(self.tmp.name, "data", "workspaces")
        os.makedirs(target_root)
        link = os.path.join(self.tmp.name, "ws-link")
        os.symlink(os.path.join(target_root, "job"), link)
        fixture = dict(DEFAULT_FIXTURE)
        fixture["disk"] = {"stdout":
            "Filesystem     1024-blocks      Used Available Capacity Mounted on\n"
            "/dev/root        1000000000  800000000   200000000     80% /\n"
            "/dev/data        2000000000 1900000000    41943040    98% "
            + target_root + "\n"}
        code, report, _ = self.run_main(fixture, workspace=link)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["disk_capacity"]["value"], 41943040)
        self.assertIn(target_root, by_id["disk_capacity"]["detail"])
        self.assertEqual(by_id["storage_ext4_qualifier"]["status"], "supported")
        self.assertEqual(by_id["storage_ext4_qualifier"]["value"], "nvme0n1p2")

    def test_df_keeps_mount_points_with_spaces(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["disk"] = {"stdout":
            "Filesystem     1024-blocks      Used Available Capacity Mounted on\n"
            "/dev/root        1000000000  800000000   200000000     80% /\n"
            "/dev/data        2000000000 1900000000    41943040    98% /mnt/worker data\n"}
        fixture["workspace_dir"] = workspace_dir_ok("/mnt/worker data")
        code, report, _ = self.run_main(fixture, workspace="/mnt/worker data/job")
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["disk_capacity"]["value"], 41943040)
        self.assertIn("/mnt/worker data", by_id["disk_capacity"]["detail"])

    def test_credential_shaped_mount_is_redacted(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["disk"] = {"stdout":
            "Filesystem     1024-blocks      Used Available Capacity Mounted on\n"
            "/dev/root        1000000000  800000000  200000000     80% /\n"
            "/dev/data        2000000000    100000  50000000     1% /mnt/token=supersecretvalue\n"}
        fixture["workspace_dir"] = workspace_dir_ok("/mnt/token=supersecretvalue")
        code, report, _ = self.run_main(fixture, workspace="/mnt/token=supersecretvalue/job")
        text = json.dumps(report)
        self.assertNotIn("supersecretvalue", text)
        self.assertIn("<redacted>", text)

    def test_workspace_mount_free_used_over_root(self):
        fixture = dict(DEFAULT_FIXTURE)
        # Workspace mount has less than 20 GiB free -> unsupported even though / is huge.
        fixture["disk"] = df_fixture(mounts=(("/var/lib/horizon-workers", "10000000"),))
        fixture["workspace_dir"] = workspace_dir_ok("/var/lib/horizon-workers")
        code, report, _ = self.run_main(fixture,
                                        workspace="/var/lib/horizon-workers")
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["disk_capacity"]["status"], "unsupported")
        self.assertEqual(code, 1)

    def test_low_memory_unsupported(self):
        code, report, _ = self.run_main(dict(DEFAULT_FIXTURE),
                                        meminfo_text=meminfo(total_kb=4 * 1024 * 1024))
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["memory_capacity"]["status"], "unsupported")

    def test_nominal_16_gib_memtotal_is_supported(self):
        # Installed 16 GiB minus ~200 MiB kernel reserve still meets the gate.
        reserved = 16 * 1024 * 1024 - 200 * 1024
        code, report, _ = self.run_main(dict(DEFAULT_FIXTURE),
                                        meminfo_text=meminfo(total_kb=reserved))
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["memory_capacity"]["status"], "supported")


class TailscaleChecks(Harness):
    def test_tailscale_absent_keeps_verdict_supported(self):
        # Issue #604 permits ordinary pinned SSH, so tailscale absence is
        # informational (unverified), not a prerequisite failure.
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("tailscale_version")
        fixture.pop("tailscale_status")
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 0)
        self.assertEqual(report["summary"]["verdict"], "supported")
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["tailscale"]["status"], "unverified")
        self.assertIn("pinned SSH", by_id["tailscale"]["detail"])

    def test_tailscale_dns_name_is_redacted(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["tailscale_status"] = {"stdout": json.dumps(
            {"Self": {"DNSName": "%s." % JWT, "Online": True}})}
        _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn(JWT, text)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("<redacted>", by_id["tailscale"]["detail"])

    def test_tailscale_online_without_dns_name(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["tailscale_status"] = {"stdout": json.dumps({"Self": {"Online": False}})}
        _, report, _ = self.run_main(fixture)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("online=false", by_id["tailscale"]["detail"])
        self.assertNotIn(" as ", by_id["tailscale"]["detail"])

    def test_tailscale_non_boolean_online_is_unknown(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["tailscale_status"] = {"stdout": json.dumps(
            {"Self": {"DNSName": "vm.tailnet.ts.net.", "Online": "true"}})}
        _, report, _ = self.run_main(fixture)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("online=unknown", by_id["tailscale"]["detail"])

    def test_mixed_case_authorization_is_redacted(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = docker_daemon_down(
            "AUTHORIZATION: bearer mixedcase-secret-value")
        _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn("mixedcase-secret-value", text)
        self.assertNotIn("AUTHORIZATION", text)

    def test_tailscale_version_only_when_status_fails(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["tailscale_status"] = {"exit_code": 1, "stdout": "", "stderr": "no daemon"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["tailscale"]["status"], "supported")
        self.assertEqual(by_id["tailscale"]["value"], "1.78.3")


class RedactionAndDeterminism(Harness):
    def test_credentials_are_redacted(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = docker_daemon_down(
            "connect unix:///var/run/docker.sock token=supersecretvalue refused")
        fixture["tailscale_version"] = {"stdout": "1.0.0 %s\n" % JWT}
        _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn("supersecretvalue", text)
        self.assertNotIn(JWT, text)
        self.assertIn("<redacted>", text)

    def test_redaction_of_nonmatching_line_is_not_quadratic(self):
        blob = "a" * 65536
        started = time.monotonic()
        out = preflight.redact(blob)
        elapsed = time.monotonic() - started
        self.assertEqual(out, blob[:400])
        self.assertLess(elapsed, 0.5)

    def test_quoted_json_diagnostics_are_redacted(self):
        blob = '{"password":"hunter2","Authorization":"Bearer supersecrettok"}'
        self.assertNotIn("hunter2", preflight.redact(blob))
        self.assertNotIn("supersecrettok", preflight.redact(blob))
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = docker_daemon_down(blob)
        _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn("hunter2", text)
        self.assertNotIn("supersecrettok", text)
        self.assertIn("<redacted>", text)

    def test_github_pat_and_uri_userinfo_are_redacted(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = docker_daemon_down(
            "clone https://user:hunter2@github.com/org/repo.git ghp_abcdefghijklmnop123")
        _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn("hunter2", text)
        self.assertNotIn("ghp_abcdefghijklmnop123", text)
        self.assertIn("<redacted>", text)

    def test_overlong_uri_userinfo_is_redacted(self):
        secret = "s" * 300
        fixture = dict(DEFAULT_FIXTURE)
        with mock.patch.dict(os.environ, {
                "DOCKER_HOST": "tcp://user:%s@remote:2376" % secret}):
            _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn(secret, text)
        self.assertIn("<redacted>", text)

    def test_username_only_uri_userinfo_is_redacted(self):
        fixture = dict(DEFAULT_FIXTURE)
        with mock.patch.dict(os.environ, {"DOCKER_HOST": "tcp://supersecret@remote:2376"}):
            _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn("supersecret", text)
        self.assertIn("<redacted>", text)

    def test_printable_line_strips_c1_and_bidi_controls(self):
        raw = "ok\x9bhidden\u202eflip"
        out = preflight.printable_line(raw)
        self.assertNotIn("\x9b", out)
        self.assertNotIn("\u202e", out)
        self.assertIn("ok", out)
        self.assertIn("hidden", out)

    def test_format_seconds_keeps_submicrosecond_timeouts(self):
        self.assertEqual(preflight.format_seconds(0.1), "0.1")
        self.assertIn("e-", preflight.format_seconds(1e-9))
        self.assertNotEqual(preflight.format_seconds(1e-9), "")

    def test_sysfs_block_access_denied_is_error(self):
        def denied(path):
            raise OSError(errno.EACCES, "Permission denied", path)

        with mock.patch.object(os, "readlink", side_effect=denied):
            code, report, _ = self.run_main(dict(DEFAULT_FIXTURE))
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["storage_ext4_qualifier"]["status"], "error")
        self.assertIn("unreadable", by_id["storage_ext4_qualifier"]["detail"])

    def test_timeout_message_preserves_fractional_seconds(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["os"] = {"timeout": True}
        argv = ["--procfs-root", os.path.join(self.tmp.name, "procfs"),
                "--sysfs-root", os.path.join(self.tmp.name, "sysfs"),
                "--workspace-path", os.path.join(self.tmp.name, "ws"),
                "--now", NOW, "--json", "--timeout", "0.1"]
        os.makedirs(os.path.join(self.tmp.name, "ws"), exist_ok=True)
        executor = self.executor_for(fixture)
        import io
        from contextlib import redirect_stdout
        buffer = io.StringIO()
        with redirect_stdout(buffer):
            preflight.main(argv, executor=executor, now=NOW)
        report = json.loads(buffer.getvalue())
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("0.1", by_id["os_linux"]["detail"])
        self.assertNotIn("after 0s", by_id["os_linux"]["detail"])

    def test_existing_file_workspace_is_rejected(self):
        fixture = dict(DEFAULT_FIXTURE)
        path = os.path.join(self.tmp.name, "not-a-dir")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write("x")
        code, report, _ = self.run_main(fixture, workspace=path)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["disk_capacity"]["status"], "error")
        self.assertIn("not a directory", by_id["disk_capacity"]["detail"])
        self.assertEqual(by_id["storage_ext4_qualifier"]["status"], "unsupported")
        self.assertIn("not a directory", by_id["storage_ext4_qualifier"]["detail"])

    def test_prefixed_credential_keys_are_redacted(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = docker_daemon_down(
            "access_token=aaa111 refresh-token=bbb222 client_secret=ccc333")
        _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn("aaa111", text)
        self.assertNotIn("bbb222", text)
        self.assertNotIn("ccc333", text)
        self.assertIn("<redacted>", text)

    def test_timeout_must_be_positive_finite(self):
        import io
        from contextlib import redirect_stderr
        for value in ("-1", "0", "nan", "inf", "-inf", "1e300"):
            with self.subTest(value=value):
                with redirect_stderr(io.StringIO()):
                    with self.assertRaises(SystemExit) as caught:
                        preflight.main(["--timeout", value])
                self.assertEqual(caught.exception.code, 3)

    def test_report_is_deterministic(self):
        import io
        from contextlib import redirect_stdout
        fixture = dict(DEFAULT_FIXTURE)

        def render():
            argv = ["--procfs-root", "/nonexistent-proc", "--sysfs-root", "/nonexistent-sys",
                    "--workspace-path", "/nonexistent-ws", "--now", NOW, "--json"]
            executor = self.executor_for(fixture)
            buffer = io.StringIO()
            with redirect_stdout(buffer):
                preflight.main(argv, executor=executor, now=NOW)
            return buffer.getvalue()

        first = render()
        second = render()
        self.assertEqual(first, second)

    def test_main_honors_injected_now_without_cli_flag(self):
        fixture = dict(DEFAULT_FIXTURE)
        procfs, sysfs, _ = build_roots(self.tmp.name, meminfo(), None, EXT4_OK, True)
        workspace = os.path.join(self.tmp.name, "ws")
        os.makedirs(workspace, exist_ok=True)
        argv = ["--procfs-root", procfs, "--sysfs-root", sysfs,
                "--workspace-path", workspace, "--json"]
        executor = self.executor_for(fixture)
        import io
        from contextlib import redirect_stdout
        buffer = io.StringIO()
        with redirect_stdout(buffer):
            preflight.main(argv, executor=executor, now=NOW)
        report = json.loads(buffer.getvalue())
        self.assertEqual(report["generated_at"], NOW)

    def test_argv_allowlist_enforced(self):
        permitted = {
            "os": ["uname", "-srm"],
            "cores": ["nproc"],
            "docker_version": ["docker", "version", "--format", "json"],
            "docker_info": ["docker", "info", "--format", "{{.Driver}}"],
            "docker_context": ["docker", "context", "inspect", "--format",
                               "{{.Endpoints.docker.Host}}"],
            "podman_client": ["podman", "--version"],
            "podman_info": ["podman", "--remote=true", "--url"],
            "podman_info_tail": ["info", "--format", "{{.Version.Version}}"],
            "disk": ["df", "-kP"],
            "tailscale_version": ["tailscale", "version"],
            "tailscale_status": ["tailscale", "status", "--json", "--peers=false"],
        }
        helper_keys = ("podman_socket", "workspace_dir")
        self.assertEqual(set(preflight.PROBE_ARGS), set(permitted) | set(helper_keys))
        for key, args in permitted.items():
            self.assertEqual(list(preflight.PROBE_ARGS[key]), args, key)
        for key in helper_keys:
            argv = list(preflight.PROBE_ARGS[key])
            self.assertEqual(argv[:3], [sys.executable, "-B", "-c"], key)
            self.assertEqual(len(argv), 4, key)
            script = argv[3]
            self.assertNotIn("unlink", script, key)
            self.assertNotIn("mkdir", script, key)
            self.assertNotIn("rmtree", script, key)
            self.assertNotIn("Popen", script, key)
        fixture = dict(DEFAULT_FIXTURE)
        _, _, executor = self.run_main(fixture)
        allowed = [list(args) for args in permitted.values()]
        extra_prefixes = [list(preflight.PROBE_ARGS[key]) for key in helper_keys]
        extra_prefixes.append(list(permitted["podman_info"]))
        extra_prefixes.append(list(permitted["disk"]))
        for argv in executor.seen:
            skipped = False
            if (len(argv) >= 3 and argv[0] == "docker" and argv[1] == "--host"
                    and argv[2].startswith("unix://")
                    and (["docker"] + argv[3:]) in allowed):
                skipped = True
            for prefix in extra_prefixes:
                if argv[:len(prefix)] == prefix and len(argv) > len(prefix):
                    skipped = True
                    break
            if skipped:
                continue
            self.assertIn(argv, allowed)

    def test_docker_daemon_probes_pin_validated_host(self):
        fixture = dict(DEFAULT_FIXTURE)
        _, _, executor = self.run_main(fixture)
        self.assertIn(
            ["docker", "--host", "unix:///var/run/docker.sock",
             "version", "--format", "json"],
            executor.seen)
        self.assertIn(
            ["docker", "--host", "unix:///var/run/docker.sock",
             "info", "--format", "{{.Driver}}"],
            executor.seen)
        self.assertNotIn(list(preflight.PROBE_ARGS["docker_version"]), executor.seen)
        self.assertNotIn(list(preflight.PROBE_ARGS["docker_info"]), executor.seen)

    def test_docker_host_pin_is_not_redacted(self):
        socket = "unix:///tmp/client_secret=supersecretvalue/docker.sock"
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_context"] = docker_context_ok(host=socket)
        _, report, executor = self.run_main(fixture)
        self.assertIn(
            ["docker", "--host", socket, "version", "--format", "json"],
            executor.seen)
        self.assertNotIn("supersecretvalue", json.dumps(report))

    def test_docker_context_nonzero_exit_is_inspect_failure(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_context"] = {
            "exit_code": 1, "stdout": "", "stderr": "permission denied"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("docker context inspect failed", by_id["container_engine"]["detail"])
        self.assertIn("permission denied", by_id["container_engine"]["detail"])
        self.assertNotIn("endpoint missing", by_id["container_engine"]["detail"])

    def test_workspace_resolver_preserves_tab_in_path(self):
        path = "/mnt/ws\tname"

        def executor(argv, timeout):
            return {"exit_code": 0,
                    "stdout": json.dumps({"path": path, "major": 8, "minor": 1}) + "\n",
                    "stderr": ""}

        resolved, major, minor, problem = preflight.resolve_workspace_directory(
            executor, 1.0, path)
        self.assertIsNone(problem)
        self.assertEqual(resolved, path)
        self.assertEqual((major, minor), (8, 1))

    def test_workspace_unreadable_is_error(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["workspace_dir"] = {"exit_code": 4, "stderr": "unreadable\n"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["disk_capacity"]["status"], "error")
        self.assertEqual(by_id["storage_ext4_qualifier"]["status"], "error")
        self.assertIn("unreadable", by_id["storage_ext4_qualifier"]["detail"])

    def test_workspace_resolver_malformed_output_is_error(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["workspace_dir"] = {"stdout": "not-a-triple\n"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["storage_ext4_qualifier"]["status"], "error")
        self.assertEqual(by_id["disk_capacity"]["status"], "error")
        self.assertIn("malformed", by_id["storage_ext4_qualifier"]["detail"])

    def test_workspace_resolution_timeout_is_bounded(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["workspace_dir"] = {"timeout": True}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["disk_capacity"]["status"], "error")
        self.assertIn("timed out", by_id["disk_capacity"]["detail"])

    def test_disk_probe_is_restricted_to_workspace_path(self):
        fixture = dict(DEFAULT_FIXTURE)
        workspace = os.path.join(self.tmp.name, "ws")
        os.makedirs(workspace, exist_ok=True)
        _, _, executor = self.run_main(fixture, workspace=workspace)
        disk_calls = [argv for argv in executor.seen
                      if argv[:2] == ["df", "-kP"]]
        self.assertEqual(len(disk_calls), 1)
        resolved, problem = preflight.workspace_directory(workspace)
        self.assertIsNone(problem)
        self.assertEqual(disk_calls[0][-1], resolved)

    def test_human_report_normalizes_multiline_details(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = docker_daemon_down(
            "denied\ncpu_capacity             supported   forged")
        _, report, _ = self.run_main(fixture)
        text = preflight.render_text(report)
        cpu_rows = [line for line in text.splitlines() if line.startswith("cpu_capacity")]
        self.assertEqual(len(cpu_rows), 1)


if __name__ == "__main__":
    unittest.main()
