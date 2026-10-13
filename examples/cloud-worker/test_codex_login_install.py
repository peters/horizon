"""Horizon's ChatGPT login replaces the worker's Codex login unless the worker renewed it."""
import json
from pathlib import Path
import runpy
import stat
import tempfile
import unittest

SCRIPT = (Path(__file__).resolve().parents[2] / "crates/horizon-core/src/"
          "cloud_runtime/codex_login/install.py")
install = runpy.run_path(str(SCRIPT))["install"]


def login(account="synthetic-account", refreshed="2026-10-01T08:00:00.123456789Z", token="synthetic-a"):
    return json.dumps({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": None,
        "tokens": {
            "id_token": "synthetic.id.token",
            "access_token": token,
            "refresh_token": f"{token}-refresh",
            "account_id": account,
        },
        "last_refresh": refreshed,
    }).encode()


class CodexLoginInstallTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.home = Path(self.directory.name)
        self.path = self.home / ".codex/auth.json"

    def tearDown(self):
        self.directory.cleanup()

    def existing(self, data):
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_bytes(data)

    def test_first_install_writes_a_private_login(self):
        incoming = login()
        self.assertEqual(install(self.home, incoming), incoming)
        self.assertEqual(self.path.read_bytes(), incoming)
        self.assertEqual(stat.S_IMODE(self.path.parent.stat().st_mode) & 0o077, 0)
        self.assertEqual(list(self.path.parent.glob(".auth-*")), [])

    def test_a_later_renewal_of_the_same_account_is_kept_and_returned(self):
        # Nanoseconds decide: Codex writes them, and Python's own parser does not read them.
        renewed = login(refreshed="2026-10-01T08:00:00.123456790Z", token="synthetic-b")
        self.existing(renewed)
        self.assertEqual(install(self.home, login()), renewed)
        self.assertEqual(self.path.read_bytes(), renewed)

    def test_an_older_same_or_other_login_is_replaced(self):
        incoming = login(refreshed="2026-10-05T08:00:00Z", token="synthetic-c")
        for current in (
            login(refreshed="2026-10-01T08:00:00Z"),
            login(refreshed="2026-10-05T08:00:00Z"),
            login(account="synthetic-other", refreshed="2026-10-09T08:00:00Z"),
            login(refreshed=None),
            json.dumps({"auth_mode": "apikey", "OPENAI_API_KEY": "synthetic"}).encode(),
            b"{half written",
        ):
            with self.subTest(current=current[:40]):
                self.existing(current)
                self.assertEqual(install(self.home, incoming), incoming)
                self.assertEqual(self.path.read_bytes(), incoming)

    def test_time_zones_compare_as_instants(self):
        self.existing(login(refreshed="2026-10-01T10:00:01+02:00", token="synthetic-b"))
        incoming = login(refreshed="2026-10-01T08:00:00Z")
        self.assertNotEqual(install(self.home, incoming), incoming)

    def test_invalid_input_changes_nothing(self):
        current = login()
        self.existing(current)
        for incoming in (
            b"",
            b"not json",
            json.dumps({"auth_mode": "apikey", "OPENAI_API_KEY": "synthetic"}).encode(),
            json.dumps({"auth_mode": "chatgpt", "tokens": {"access_token": "synthetic"}}).encode(),
            login() + b" " * (64 * 1024),
        ):
            with self.subTest(incoming=incoming[:40]):
                with self.assertRaises(ValueError):
                    install(self.home, incoming)
                self.assertEqual(self.path.read_bytes(), current)


if __name__ == "__main__":
    unittest.main()
