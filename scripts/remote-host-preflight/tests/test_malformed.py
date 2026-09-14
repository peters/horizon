"""Deterministic tests for the read-only Linux host preflight."""
from harness import *  # noqa: F401,F403
class MalformedInputs(Harness):
    def test_nproc_malformed_falls_back_to_cpuinfo(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["cores"] = {"stdout": "not-a-number\n"}
        code, report, _ = self.run_main(fixture, cpuinfo_text="processor\t: 0\nprocessor\t: 1\n")
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["cpu_capacity"]["value"], 2)
        self.assertEqual(by_id["cpu_capacity"]["status"], "unsupported")

    def test_cpuinfo_processor_count_key_is_not_a_core(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["cores"] = {"stdout": "not-a-number\n"}
        cpuinfo = "\n".join("processor_count: %d" % i for i in range(8)) + "\n"
        code, report, _ = self.run_main(fixture, cpuinfo_text=cpuinfo)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["cpu_capacity"]["status"], "error")

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


