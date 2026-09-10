"""Deterministic tests for the read-only Azure readiness preflight.

Every test uses synthetic fixtures or an injected executor. Nothing here invokes
the Azure CLI, needs credentials or inspects the operator's account.
"""
import io
import json
import os
import pathlib
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import preflight  # noqa: E402

SUB = "0f0e0d0c-0b0a-4908-8706-050403020100"
OTHER_UUID = "9a8b7c6d-5e4f-4a3b-9c2d-1e0f9a8b7c6d"
REGION = "northeurope"
SECRET = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.c3ludGhldGljLXBheWxvYWQ.c2lnbmF0dXJl"
BASE = ["--subscription", SUB, "--region", REGION]


def account(sub=SUB, state="Enabled"):
    return {"exit_code": 0, "stdout": {"id": sub, "state": state, "tenantId": OTHER_UUID,
                                       "user": {"name": "operator@example.test", "type": "user"}}}


def registered(namespace):
    return {"exit_code": 0, "stdout": {"namespace": namespace, "registrationState": "Registered"}}


def usage(*entries):
    return {"exit_code": 0, "stdout": [{"name": {"value": n, "localizedValue": n}, "currentValue": c, "limit": l,
                                        "unit": "Count"} for n, c, l in entries]}


LOCATIONS = {"exit_code": 0, "stdout": [{"name": "westeurope", "type": "Region", "regionType": "Physical"},
                                        {"name": REGION, "type": "Region", "regionType": "Physical"}]}
ACI_CAPS = {"exit_code": 0, "stdout": [
    {"osType": "Windows", "ipAddressType": "Public", "gpu": "None", "capabilities": {"maxCpu": 4, "maxMemoryInGB": 14}},
    {"osType": "Linux", "ipAddressType": "Public", "gpu": "None", "capabilities": {"maxCpu": 4, "maxMemoryInGB": 16}}]}
VM_SKU = {"exit_code": 0, "stdout": [
    {"name": "Standard_D4s_v3_extra", "resourceType": "virtualMachines", "family": "other", "restrictions": [],
     "capabilities": [{"name": "vCPUs", "value": "8"}]},
    {"name": "Standard_D4s_v3", "resourceType": "virtualMachines", "family": "standardDSv3Family",
     "restrictions": [{"type": "Zone", "reasonCode": "NotAvailableForSubscription"}],
     "capabilities": [{"name": "vCPUs", "value": "4"}]}]}


def happy_fixture(candidate):
    spec = preflight.CANDIDATES[candidate]
    fixture = {"account_context": account(), "region_available": LOCATIONS}
    for namespace in spec["required"] + spec["supporting"]:
        fixture[f"provider_{namespace.split('.', 1)[1].lower()}"] = registered(namespace)
    if candidate == "aci":
        fixture.update(aci_regional_quota=usage(("ContainerGroups", 3, 100), ("StandardCores", 10, 100)),
                       aci_regional_capabilities=ACI_CAPS)
    elif candidate == "vm":
        fixture.update(vm_sku_availability=VM_SKU,
                       vm_regional_quota=usage(("cores", 4, 10), ("standardDSv3Family", 0, 8)))
    else:
        fixture["container_apps_regional_quota"] = usage(("ManagedEnvironmentCount", 1, 10),
                                                         ("ManagedEnvironmentCores", 2, 20))
    return fixture


class Harness(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        patcher = mock.patch.object(subprocess, "Popen", side_effect=AssertionError("no subprocess allowed"))
        self.popen = patcher.start()
        self.addCleanup(patcher.stop)

    def run_main(self, args, fixture=None):
        if fixture is not None:
            path = os.path.join(self.tmp.name, f"fixture-{len(os.listdir(self.tmp.name))}.json")
            pathlib.Path(path).write_text(json.dumps(fixture), encoding="utf-8")
            args = [*args, "--fixture", path]
        out, err = io.StringIO(), io.StringIO()
        code = preflight.main(args, stdout=out, stderr=err)
        return code, out.getvalue(), err.getvalue()

    def report(self, candidate, fixture, extra=()):
        code, out, err = self.run_main(["--candidate", candidate, *BASE, *extra, "--json"], fixture)
        self.assertEqual(err, "")
        return code, json.loads(out)

    def check(self, report, check_id):
        return next(c for c in report["checks"] if c["id"] == check_id)


class OfflineAndInputTests(Harness):
    def test_plan_mode_runs_zero_commands_and_shows_redacted_operations(self):
        code, out, err = self.run_main(["--candidate", "aci", *BASE])
        self.assertEqual((code, err), (0, ""))
        self.assertIn("status=planned", out)
        self.assertIn("az account show --subscription <subscription>", out)
        self.assertNotIn(SUB, out)
        self.popen.assert_not_called()

    def test_invalid_inputs_fail_before_any_execution(self):
        cases = [
            ["--candidate", "aci", "--subscription", "My Subscription", "--region", REGION, "--live"],
            ["--candidate", "aci", "--subscription", SUB, "--region", "North Europe", "--live"],
            ["--candidate", "vm", *BASE, "--live"],
            ["--candidate", "aci", *BASE, "--vm-size", "Standard_D4s_v3"],
            ["--candidate", "aci", *BASE, "--cpu-cores", "0"], ["--candidate", "aci", *BASE, "--fixture", ""],
            ["--candidate", "aci", *BASE, "--timeout-seconds", "9999", "--live"],
            ["--candidate", "aci", *BASE, "--live", "--fixture", "/nonexistent.json"],
            ["--candidate", "aci", *BASE, "--cpu-cores", "abc"], ["--candidate", "aks", *BASE],
        ]
        for args in cases:
            code, out, err = self.run_main(args)
            self.assertEqual(code, 3, args)
            self.assertTrue(err.startswith("error: "), args)
            self.assertNotIn(SUB, err)
        self.popen.assert_not_called()

    def test_live_mode_without_azure_cli_or_posix_host_is_rejected_before_execution(self):
        with mock.patch.object(preflight.shutil, "which", return_value=None):
            code, _, err = self.run_main(["--candidate", "aci", *BASE, "--live", "--az-path", "az-missing"])
        self.assertEqual((code, "not found" in err), (3, True))
        with mock.patch.object(preflight.os, "name", "nt"):
            code, _, err = self.run_main(["--candidate", "aci", *BASE, "--live"])
        self.assertEqual((code, "POSIX host" in err), (3, True))
        self.popen.assert_not_called()


class PlanningTests(Harness):
    def request(self, candidate, **overrides):
        base = dict(candidate=candidate, subscription=SUB, region=REGION, mode="plan", cpu_cores=2, memory_gb=4,
                    vm_size="Standard_D4s_v3" if candidate == "vm" else None, timeout_seconds=30, az_path="az")
        return preflight.Request(**{**base, **overrides})

    def test_every_query_is_scoped_to_the_requested_subscription_and_region(self):
        for candidate in preflight.CANDIDATES:
            for check in preflight.plan_checks(self.request(candidate)):
                argv = check.argv
                self.assertEqual(argv[0], "az")
                if "--subscription" in argv:
                    self.assertEqual(argv[argv.index("--subscription") + 1], SUB, check.id)
                else:
                    self.assertTrue(any(a.startswith(f"{preflight.ARM}/subscriptions/{SUB}/") for a in argv), check.id)
                if check.id not in ("account_context", "region_available") and not check.id.startswith("provider_"):
                    self.assertTrue(any(REGION in a for a in argv), check.id)
                    self.assertIn("region_available", check.depends_on)
                if check.id != "account_context":
                    self.assertIn("account_context", check.depends_on)
                self.assertNotIn("account set", " ".join(argv))
                if "rest" in argv:
                    self.assertEqual(argv[argv.index("--method") + 1], "get")
                    self.assertIn("api-version=", argv[argv.index("--url") + 1])

    def test_no_forbidden_lifecycle_command_can_be_planned_or_forged(self):
        for candidate in preflight.CANDIDATES:
            for check in preflight.plan_checks(self.request(candidate)):
                command = [t for t in check.argv[1:] if not t.startswith("-")][:2]
                self.assertFalse(preflight.FORBIDDEN_TOKENS.intersection(command), check.id)
                preflight.assert_allowlisted(check.argv)
        for forged in (["az", "container", "create", "--name", "x"], ["az", "provider", "register", "-n", "Microsoft.App"],
                       ["az", "account", "set", "-s", SUB], ["az", "login"], ["az", "extension", "add", "-n", "containerapp"],
                       ["az", "rest", "--method", "delete", "--url", f"{preflight.ARM}/subscriptions/{SUB}/x?api-version=1"],
                       ["az", "rest", "--method", "get", "--url", "https://evil.example/x?api-version=1"],
                       ["az", "rest", "--url", f"{preflight.ARM}/subscriptions/{SUB}/locations?api-version=2022-12-01"],
                       ["az", "rest", "--method", "get", "--url", f"{preflight.ARM}/subscriptions/{SUB}/resources?api-version=2021-04-01"]):
            with self.assertRaises(AssertionError, msg=forged):
                preflight.assert_allowlisted(forged)
        stop_region = self.request("vm", region="stop")
        self.assertTrue(any("stop" in c.argv for c in preflight.plan_checks(stop_region)))


@unittest.skipUnless(os.name == "posix", "the live executor is POSIX-only by design")
class ExecutorTests(unittest.TestCase):
    """Real subprocess boundary, exercised with the Python interpreter instead of the Azure CLI."""

    def test_arguments_pass_without_shell_expansion_and_time_and_output_are_bounded(self):
        run = preflight.subprocess_executor
        literal = run([sys.executable, "-c", "import sys; print(sys.argv[1])", "$HOME `id` ; touch x"], 20)
        self.assertEqual((literal.exit_code, literal.stdout.strip()), (0, "$HOME `id` ; touch x"))
        slow = run([sys.executable, "-c", "import time; time.sleep(30)"], 1)
        self.assertTrue(slow.timed_out)
        self.assertEqual(preflight.classify_failure(slow).reason, "timeout")
        big = run([sys.executable, "-c", "import sys; sys.stdout.write('x' * 8192); sys.stdout.flush(); import time; "
                   "time.sleep(30)"], 20, limit=4096)
        self.assertEqual((big.oversized, big.exit_code, len(big.stdout) <= 4096), (True, None, True))
        self.assertEqual(preflight.classify_failure(big).reason, "oversized_output")
        launch = run([os.path.join(tempfile.gettempdir(), "horizon-preflight-missing-az-binary")], 5)
        self.assertEqual((launch.launch_failed, preflight.classify_failure(launch).reason), (True, "launch_failed"))
        failed = run([sys.executable, "-c", "import sys; sys.stderr.write('ERROR: AADSTS700082 expired'); sys.exit(1)"], 20)
        self.assertEqual((failed.exit_code, preflight.classify_failure(failed).reason), (1, "authentication_required"))

    @unittest.skipUnless(os.path.isdir("/proc"), "grandchild liveness probe reads /proc")
    def test_timeout_kills_the_whole_process_group_including_wrapper_children(self):
        wrapper = ("import subprocess, sys; child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)']); "
                   "print(child.pid, flush=True); child.wait()")
        result = preflight.subprocess_executor([sys.executable, "-c", wrapper], 1)
        self.assertTrue(result.timed_out)
        grandchild = int(result.stdout.strip())

        def alive(pid):
            try:
                return "zombie" not in pathlib.Path(f"/proc/{pid}/status").read_text()
            except OSError:
                return False

        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and alive(grandchild):
            time.sleep(0.05)
        self.assertFalse(alive(grandchild))


class InterpretationTests(Harness):
    def test_successful_preflight_leaves_qualification_gates_unverified(self):
        for candidate in preflight.CANDIDATES:
            extra = ["--vm-size", "Standard_D4s_v3"] if candidate == "vm" else []
            code, report = self.report(candidate, happy_fixture(candidate), extra)
            self.assertEqual(code, 0, candidate)
            self.assertEqual(report["status"], "no_blockers_observed")
            self.assertEqual(report["blockers"], [])
            self.assertTrue(all(c["outcome"] == "observed_ok" for c in report["checks"]), candidate)
            self.assertEqual({c["role"] for c in report["checks"]}, {"required", "supporting"})
            gates = " ".join(report["unverified_gates"])
            for word in ("ext4", "managed identity", "180-second", "PC is off", "SSH", "create permission", "capacity"):
                self.assertIn(word, gates)
            self.assertNotIn("ready", report["status"])
            self.assertEqual(report["subscription"], "<subscription>")
            self.assertEqual(report["schema_version"], preflight.REPORT_SCHEMA_VERSION)
            self.assertIsNotNone(report["observed_at"])
        self.popen.assert_not_called()

    def test_account_context_is_compared_with_the_requested_subscription(self):
        fixture = happy_fixture("aci")
        fixture["account_context"] = account(sub=OTHER_UUID)
        code, report = self.report("aci", fixture)
        self.assertEqual((code, report["status"]), (1, "blocked"))
        self.assertEqual(self.check(report, "account_context")["reason"], "subscription_mismatch")
        fixture["account_context"] = account(state="Disabled")
        self.assertEqual(self.check(self.report("aci", fixture)[1], "account_context")["reason"],
                         "subscription_not_enabled")
        fixture["account_context"] = {"exit_code": 1, "stdout": "",
                                      "stderr": f"ERROR: The subscription of '{SUB}' doesn't exist in cloud 'AzureCloud'."}
        self.assertEqual(self.check(self.report("aci", fixture)[1], "account_context")["reason"],
                         "subscription_not_visible")
        fixture["account_context"] = {"exit_code": 1, "stdout": "", "stderr": "ERROR: Please run 'az login' to setup account."}
        report = self.report("aci", fixture)[1]
        self.assertEqual(self.check(report, "account_context")["reason"], "authentication_required")
        self.assertTrue(all(c["reason"] == "prerequisite_failed" for c in report["checks"][1:]))
        self.assertEqual(report["status"], "blocked")

    def test_region_and_provider_states(self):
        fixture = happy_fixture("aci")
        fixture["region_available"] = {"exit_code": 0, "stdout": [{"name": "westeurope", "type": "Region"}]}
        report = self.report("aci", fixture)[1]
        self.assertEqual(self.check(report, "region_available")["reason"], "region_unavailable")
        self.assertEqual(self.check(report, "aci_regional_quota")["reason"], "prerequisite_failed")
        self.assertEqual(self.check(report, "provider_containerinstance")["outcome"], "observed_ok")
        fixture["region_available"] = {"exit_code": 0, "stdout": [{"name": REGION, "type": "Region", "regionType": "Logical"}]}
        self.assertEqual(self.check(self.report("aci", fixture)[1], "region_available")["reason"], "region_unavailable")
        fixture = happy_fixture("aci")
        for state, outcome, reason in (("NotRegistered", "blocked", "unregistered_provider"),
                                       ("Registering", "unknown", "registration_in_progress"),
                                       ("Registered", "observed_ok", None)):
            fixture["provider_containerinstance"] = {"exit_code": 0, "stdout": {
                "namespace": "Microsoft.ContainerInstance", "registrationState": state}}
            check = self.check(self.report("aci", fixture)[1], "provider_containerinstance")
            self.assertEqual((check["outcome"], check["reason"]), (outcome, reason), state)
        fixture["provider_containerinstance"] = {"exit_code": 0, "stdout": {
            "namespace": "Microsoft.Compute", "registrationState": "Registered"}}
        self.assertEqual(self.check(self.report("aci", fixture)[1], "provider_containerinstance")["reason"],
                         "contradictory_response")
        fixture["provider_containerinstance"] = {"exit_code": 1, "stdout": "", "stderr":
                                                 'ERROR: (AuthorizationFailed) The client does not have authorization'}
        check = self.check(self.report("aci", fixture)[1], "provider_containerinstance")
        self.assertEqual((check["reason"], check["details"]["error_code"]), ("insufficient_permission", "AuthorizationFailed"))

    def test_quota_sufficient_exhausted_missing_unknown_and_contradictory(self):
        cores = ("StandardCores", 10, 100)
        cases = [
            (usage(("ContainerGroups", 3, 100), cores), "observed_ok", None),
            (usage(("ContainerGroups", 100, 100), cores), "blocked", "quota_exhausted"),
            (usage(("ContainerGroups", 3, 100), ("StandardCores", 99, 100)), "blocked", "quota_exhausted"),
            (usage(("ContainerGroups", 0, -1), cores), "observed_ok", None),
            (usage(cores), "unknown", "quota_entry_missing"),
            (usage(("ContainerGroups", 101, 100), cores), "unknown", "contradictory_response"),
            (usage(("ContainerGroups", 101, 100), ("StandardCores", 100, 100)), "blocked", "quota_exhausted"),
            ({"exit_code": 0, "stdout": [{"name": {"value": "ContainerGroups"}, "currentValue": "3", "limit": 100}]},
             "unknown", "malformed_response"),
            ({"exit_code": 0, "stdout": "not json {"}, "unknown", "malformed_response"),
            ({"exit_code": 0, "stdout": [{"name": "ContainerGroups", "currentValue": 3, "limit": 100}]}, "unknown",
             "malformed_response"),
            ({"exit_code": 0, "stdout": [{"name": {"value": "ContainerGroups"}, "currentValue": 3, "limit": 100},
                                         "trailing-garbage"]}, "unknown", "malformed_response"),
            ({"exit_code": 0, "stdout": {"value": []}}, "unknown", "malformed_response"),
            ({"timed_out": True}, "unknown", "timeout"),
            ({"oversized": True}, "unknown", "oversized_output"),
            ({"exit_code": 1, "stderr": "Not Found({\"error\":{\"code\":\"NoRegisteredProviderFound\"}})"}, "unknown",
             "unsupported_query"),
            ({"exit_code": 1, "stderr": "Conflict({\"error\":{\"code\":\"MissingSubscriptionRegistration\"}})"}, "blocked",
             "unregistered_provider"),
            ({"exit_code": 1, "stderr": "ERROR: (MissingSubscriptionRegistration) Forbidden to use namespace"}, "blocked",
             "unregistered_provider"),
            ({"exit_code": 1, "stderr": "ERROR: read 403 bytes before the connection reset"}, "unknown", "command_failed"),
            ({"exit_code": 1, "stderr": "Too Many Requests({\"error\":{\"code\":\"TooManyRequests\"}})"}, "unknown",
             "throttled"),
            (None, "unknown", "not_executed"),
        ]
        for entry, outcome, reason in cases:
            fixture = happy_fixture("aci")
            fixture["aci_regional_quota"] = entry
            if entry is None:
                del fixture["aci_regional_quota"]
            code, report = self.report("aci", fixture)
            check = self.check(report, "aci_regional_quota")
            self.assertEqual((check["outcome"], check["reason"]), (outcome, reason), entry)
            self.assertEqual(code, {"observed_ok": 0, "blocked": 1, "unknown": 2}[outcome])
            if outcome == "observed_ok":
                self.assertEqual(check["details"]["capacity"], "unverified")

    def test_aci_capabilities_require_linux_public_headroom(self):
        fixture = happy_fixture("aci")
        fixture["aci_regional_capabilities"] = {"exit_code": 0, "stdout": [ACI_CAPS["stdout"][0]]}
        self.assertEqual(self.check(self.report("aci", fixture)[1], "aci_regional_capabilities")["reason"],
                         "no_linux_public_capability")
        fixture["aci_regional_capabilities"] = ACI_CAPS
        check = self.check(self.report("aci", fixture, ["--cpu-cores", "8"])[1], "aci_regional_capabilities")
        self.assertEqual(check["reason"], "request_exceeds_regional_maximum")
        small = {**ACI_CAPS["stdout"][1], "capabilities": {"maxCpu": 1, "maxMemoryInGB": 1}}
        fixture["aci_regional_capabilities"] = {"exit_code": 0, "stdout": [small, ACI_CAPS["stdout"][1]]}
        self.assertEqual(self.check(self.report("aci", fixture)[1], "aci_regional_capabilities")["outcome"], "observed_ok")

    def test_vm_sku_drives_family_quota_and_restrictions(self):
        extra = ["--vm-size", "Standard_D4s_v3"]
        fixture = happy_fixture("vm")
        code, report = self.report("vm", fixture, extra)
        quota = self.check(report, "vm_regional_quota")
        self.assertEqual(code, 0)
        self.assertEqual(quota["details"]["standardDSv3Family"]["headroom"], 8)
        self.assertEqual(self.check(report, "vm_sku_availability")["details"]["zone_restrictions"],
                         ["NotAvailableForSubscription"])
        fixture["vm_regional_quota"] = usage(("cores", 8, 10), ("standardDSv3Family", 0, 8))
        self.assertEqual(self.check(self.report("vm", fixture, extra)[1], "vm_regional_quota")["reason"], "quota_exhausted")
        fixture = happy_fixture("vm")
        fixture["vm_sku_availability"] = {"exit_code": 0, "stdout": [{
            **VM_SKU["stdout"][1], "restrictions": [{"type": "Location", "reasonCode": "NotAvailableForSubscription"}]}]}
        report = self.report("vm", fixture, extra)[1]
        self.assertEqual(self.check(report, "vm_sku_availability")["reason"], "sku_restricted")
        self.assertEqual(self.check(report, "vm_regional_quota")["reason"], "prerequisite_failed")
        fixture["vm_sku_availability"] = {"exit_code": 0, "stdout": [VM_SKU["stdout"][0]]}
        self.assertEqual(self.check(self.report("vm", fixture, extra)[1], "vm_sku_availability")["reason"],
                         "sku_unavailable_in_region")
        for bad in (None, "0", "-1", "four", True):
            fixture["vm_sku_availability"] = {"exit_code": 0, "stdout": [{
                **VM_SKU["stdout"][1], "capabilities": [{"name": "vCPUs", "value": bad}]}]}
            self.assertEqual(self.check(self.report("vm", fixture, extra)[1], "vm_sku_availability")["reason"],
                             "malformed_response", bad)

    def test_container_apps_quota_uses_requested_cores(self):
        fixture = happy_fixture("container-apps")
        fixture["container_apps_regional_quota"] = usage(("ManagedEnvironmentCount", 1, 10),
                                                         ("ManagedEnvironmentCores", 19, 20))
        report = self.report("container-apps", fixture)[1]
        self.assertEqual(self.check(report, "container_apps_regional_quota")["reason"], "quota_exhausted")
        self.assertEqual(self.report("container-apps", fixture, ["--cpu-cores", "1"])[0], 0)


class OutputSafetyTests(Harness):
    def test_secrets_and_identifiers_are_redacted_from_stdout_stderr_and_report(self):
        fixture = happy_fixture("aci")
        fixture["aci_regional_quota"] = {"exit_code": 1, "stdout": "", "stderr":
                                         f"ERROR: (AuthorizationFailed) token {SECRET} for operator@example.test on "
                                         f"/subscriptions/{SUB}/resourceGroups/private-rg id {OTHER_UUID}"}
        report_path = os.path.join(self.tmp.name, "report.json")
        code, out, err = self.run_main(["--candidate", "aci", *BASE, "--report", report_path], fixture)
        written = pathlib.Path(report_path).read_text(encoding="utf-8")
        self.assertEqual(code, 1)
        for text in (out, err, written):
            for leaked in (SUB, OTHER_UUID, SECRET, "operator@example.test", "private-rg", "AuthorizationFailed The"):
                self.assertNotIn(leaked, text)
        self.assertEqual(stat.S_IMODE(os.stat(report_path).st_mode), 0o600)
        parsed = json.loads(written)
        self.assertEqual(parsed["schema"], preflight.REPORT_SCHEMA)
        self.assertEqual(parsed["status"], "blocked")
        self.assertIn("aci_regional_quota: insufficient_permission", parsed["blockers"])
        self.assertEqual(preflight.redact(f"sub {SUB} other {OTHER_UUID} mail a.b@c.io {SECRET}", SUB),
                         "sub <subscription> other <uuid> mail <email> <token>")

    def test_existing_report_file_is_refused_before_execution(self):
        report_path = os.path.join(self.tmp.name, "existing.json")
        pathlib.Path(report_path).write_text("keep me", encoding="utf-8")
        code, out, err = self.run_main(["--candidate", "aci", *BASE, "--report", report_path], happy_fixture("aci"))
        self.assertEqual((code, out), (3, ""))
        self.assertIn("refusing to overwrite", err)
        self.assertEqual(pathlib.Path(report_path).read_text(encoding="utf-8"), "keep me")

    def test_fixture_must_be_an_object_and_unknown_checks_stay_unknown(self):
        code, _, err = self.run_main(["--candidate", "aci", *BASE], fixture=[1, 2])
        self.assertEqual(code, 3)
        self.assertIn("JSON object", err)
        code, report = self.report("aci", {})
        self.assertEqual((code, report["status"]), (2, "unknown"))
        self.assertTrue(all(c["reason"] == "not_executed" for c in report["checks"]))


if __name__ == "__main__":
    unittest.main()
