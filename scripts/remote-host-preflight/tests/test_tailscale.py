"""Deterministic tests for the read-only Linux host preflight."""
from harness import *  # noqa: F401,F403
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


