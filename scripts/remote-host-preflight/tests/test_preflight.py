"""Deterministic tests for the read-only Linux host preflight.

Every test uses synthetic procfs/sysfs roots and an injected executor driven
by fixtures. Nothing here invokes a real host tool, and the subprocess module
is patched to fail if any code path falls back to a real spawn. The executor
also enforces the fixed argv allowlist, so a probe regression that interpolates
host values into arguments fails loudly.
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
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
                                  "Server": {"Version": version, "OSType": "linux"}})}


def docker_daemon_down(stderr="Cannot connect to the Docker daemon at unix:///var/run/docker.sock. "
                              "Is the docker daemon running?"):
    return {"exit_code": 1, "stdout": "", "stderr": stderr}


def podman_ok(version="4.9.0"):
    # Real `podman info --format json` schema: top-level version.Version.
    return {"stdout": json.dumps({"version": {"Version": version}, "host": {}})}


def podman_ok_legacy(version="4.9.0"):
    # Legacy shape, kept as a parser fallback.
    return {"stdout": json.dumps({"host": {"version": {"output": version}}})}


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


def tailscale_status_fixture(dns_name="fintermac-vm.tailnet-f382.ts.net.", online=True):
    return {"stdout": json.dumps({"Self": {"DNSName": dns_name, "Online": online}})}


DEFAULT_FIXTURE = {
    "os": os_fixture(),
    "docker_version": docker_ok(),
    "docker_info": {"stdout": "Storage Driver: overlay2\nCgroup Driver: systemd\n"},
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
        patcher = mock.patch.object(subprocess, "run", side_effect=AssertionError(
            "no real subprocess allowed in unit tests"))
        self.popen = patcher.start()
        self.addCleanup(patcher.stop)

    def executor_for(self, fixture):
        seen = []

        def executor(argv, timeout):
            seen.append(list(argv))
            for key, args in preflight.PROBE_ARGS.items():
                if list(args) == list(argv):
                    entry = fixture.get(key)
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
        self.assertEqual(by_id["storage_ext4_qualifier"]["value"], "nvme0n1p2")
        self.assertEqual(by_id["tailscale"]["value"], "fintermac-vm.tailnet-f382.ts.net")

    def test_aarch64_podman_only_supported(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["os"] = os_fixture(machine="aarch64")
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture["podman_info"] = podman_ok()
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["os_linux"]["status"], "supported")
        self.assertEqual(by_id["container_engine"]["value"], "podman 4.9.0")

    def test_darwin_unsupported(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["os"] = os_fixture(osname="Darwin", machine="arm64")
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["os_linux"]["status"], "unsupported")
        self.assertEqual(report["summary"]["verdict"], "unsupported")

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

    def test_podman_legacy_version_shape_still_parses(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture["podman_info"] = podman_ok_legacy("4.9.0")
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["value"], "podman 4.9.0")

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

    def test_disk_selects_longest_mount_ancestor(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["disk"] = {"stdout":
            "Filesystem     1024-blocks      Used Available Capacity Mounted on\n"
            "/dev/root        1000000000  800000000   200000000     80% /\n"
            "/dev/data        2000000000 1900000000    41943040    98% /mnt/workspaces\n"}
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

    def test_nproc_missing_and_no_cpuinfo(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["cores"] = {"exit_code": 1, "stdout": "", "stderr": "boom"}
        code, report, _ = self.run_main(fixture, cpuinfo_text=None)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["cpu_capacity"]["status"], "error")

    def test_meminfo_missing_total(self):
        fixture = dict(DEFAULT_FIXTURE)
        code, report, _ = self.run_main(fixture, meminfo_text="MemFree: 100 kB\n")
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["memory_capacity"]["status"], "unsupported")

    def test_meminfo_absent(self):
        code, report, _ = self.run_main(dict(DEFAULT_FIXTURE), meminfo_text=None)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["memory_capacity"]["status"], "error")

    def test_df_missing_root(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["disk"] = {"stdout": "Filesystem  1024-blocks Used Available Capacity Mounted on\n"
                                     "/dev/data  100000 10000 90000 10% /data\n"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["disk_capacity"]["status"], "error")

    def test_df_non_numeric_free(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["disk"] = {"stdout": "Filesystem  1024-blocks Used Available Capacity Mounted on\n"
                                     "/dev/root  100000 10000 ?? 10% /\n"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["disk_capacity"]["status"], "error")


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
    def test_workspace_mount_free_used_over_root(self):
        fixture = dict(DEFAULT_FIXTURE)
        # Workspace mount has less than 20 GiB free -> unsupported even though / is huge.
        fixture["disk"] = df_fixture(mounts=(("/var/lib/horizon-workers", "10000000"),))
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

    def test_argv_allowlist_enforced(self):
        fixture = dict(DEFAULT_FIXTURE)
        _, _, executor = self.run_main(fixture)
        allowed = [list(args) for args in preflight.PROBE_ARGS.values()]
        for argv in executor.seen:
            self.assertIn(argv, allowed)


if __name__ == "__main__":
    unittest.main()
