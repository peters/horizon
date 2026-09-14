"""Offline tests for the remote browser spike's credential and transport rules (#628).

No network, no credentials, no device time: these pin the safety properties the
Horizon-internal client must keep, using a local plaintext HTTP listener only to
prove that the spike refuses it.
"""
from __future__ import annotations

import http.server
import json
import os
import pathlib
import sys
import tempfile
import threading
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))

import spike  # noqa: E402


class VersionResolutionTest(unittest.TestCase):
    def test_exact_and_patch_resolution_match(self) -> None:
        self.assertTrue(spike.version_matches("18", "18"))
        self.assertTrue(spike.version_matches("18", "18.6"))
        self.assertTrue(spike.version_matches("16.0", "16.0"))
        self.assertTrue(spike.version_matches("16.0", "16.0.1"))

    def test_other_major_or_prefix_collision_does_not_match(self) -> None:
        self.assertFalse(spike.version_matches("18", "17.6"))
        self.assertFalse(spike.version_matches("1", "18.6"))
        self.assertFalse(spike.version_matches("16.0", "16.1"))


class NetrcAuthTest(unittest.TestCase):
    def test_basic_header_is_built_from_the_hub_host_entry_only(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "netrc")
            with open(path, "w", encoding="utf-8") as handle:
                handle.write("machine hub.example.net login alice password s3cret\n")
            os.chmod(path, 0o600)
            header = spike.load_auth(path, "hub.example.net")
            self.assertTrue(header.startswith("Basic "))
            self.assertNotIn("s3cret", header)
            with self.assertRaises(SystemExit):
                spike.load_auth(path, "other.example.net")

    def test_missing_file_is_an_actionable_exit_without_the_path_contents(self) -> None:
        with self.assertRaises(SystemExit) as raised:
            spike.load_auth("/nonexistent/netrc", "hub.example.net")
        self.assertIn("netrc unusable", str(raised.exception))


class TransportPolicyTest(unittest.TestCase):
    def test_plain_http_hub_is_rejected_before_any_request(self) -> None:
        with self.assertRaises(SystemExit):
            spike.Transport("http://127.0.0.1:9/wd/hub", spike.DEFAULT_API, "Basic x")

    def test_userinfo_and_query_strings_are_rejected(self) -> None:
        with self.assertRaises(SystemExit):
            spike.Transport("https://user:key@hub.example.net/wd/hub", spike.DEFAULT_API, "Basic x")
        with self.assertRaises(SystemExit):
            spike.Transport("https://hub.example.net/wd/hub?key=1", spike.DEFAULT_API, "Basic x")

    def test_credentials_never_leave_the_configured_origins(self) -> None:
        transport = spike.Transport("https://hub.example.net/wd/hub", "https://api.example.net", "Basic x")
        with self.assertRaises(RuntimeError):
            transport.request("https://artifacts.example.net/video.mp4", "GET", None, 1)
        with self.assertRaises(RuntimeError):
            transport.request("https://hub.example.net.evil.test/wd/hub/status", "GET", None, 1)

    def test_redirects_are_not_followed(self) -> None:
        received: list = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):  # noqa: N802
                received.append(self.path)
                self.send_response(302)
                self.send_header("Location", "http://127.0.0.1:1/elsewhere")
                self.end_headers()

            def log_message(self, *_args):  # noqa: D401
                return

        server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            handler = spike.NoRedirect()
            self.assertIsNone(handler.redirect_request(None, None, 302, "Found", {}, "https://x.test/"))
            transport = spike.Transport("https://hub.example.net/wd/hub", "https://api.example.net", "Basic x")
            transport.hub_origin = f"http://127.0.0.1:{server.server_address[1]}"
            response = transport.request(f"{transport.hub_origin}/session", "GET", None, 5)
            self.assertEqual(response.get("status"), 302)
            self.assertEqual(received, ["/session"])
        finally:
            server.shutdown()
            server.server_close()


class OutcomeTest(unittest.TestCase):
    def test_webdriver_error_codes_are_surfaced_typed(self) -> None:
        run = spike.Spike.__new__(spike.Spike)
        self.assertEqual(run.is_error({"status": 404, "body": {"value": {"error": "unknown command"}}}), "unknown command")
        self.assertEqual(run.is_error({"status": None, "error": "TimeoutError"}), "TimeoutError")
        self.assertIsNone(run.is_error({"status": 200, "body": {"value": None}}))
        self.assertEqual(run.is_error({"status": 502, "body": {}}), "http_502")

    def test_report_steps_are_json_and_carry_no_authorization(self) -> None:
        step = spike.Step("new_session", "unknown", 1, {"note": "allocation-unknown; not retried"})
        encoded = json.dumps(spike.dataclasses.asdict(step))
        self.assertNotIn("Basic ", encoded)
        self.assertIn("allocation-unknown", encoded)


if __name__ == "__main__":
    unittest.main()
