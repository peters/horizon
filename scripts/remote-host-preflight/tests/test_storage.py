"""Deterministic tests for the read-only Linux host preflight."""
from harness import *  # noqa: F401,F403
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


