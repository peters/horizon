"""Deterministic tests for the read-only Linux host preflight."""
from harness import *  # noqa: F401,F403
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


