"""Deterministic tests for the read-only Linux host preflight."""
from harness import *  # noqa: F401,F403
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


