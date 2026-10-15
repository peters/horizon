"""Reconnecting applies unbound API credentials while retaining ChatGPT and subscription logins."""
import json
from pathlib import Path
import runpy
import tempfile
import unittest

SCRIPT = (Path(__file__).resolve().parents[2] / "crates/horizon-core/src/"
          "cloud_runtime/deployment/clear_agent_auth.py")
clear = runpy.run_path(str(SCRIPT))["clear_api_auth"]


class AgentAuthRemovalTests(unittest.TestCase):
    def test_unbound_keys_are_removed_but_subscription_login_survives(self):
        for auth in ({"auth_mode": "apikey", "OPENAI_API_KEY": "synthetic"},
                     {"OPENAI_API_KEY": "synthetic"},
                     # Unbound keys beside tokens in a record that is not a ChatGPT login.
                     {"auth_mode": "oauth", "OPENAI_API_KEY": "synthetic",
                      "tokens": {"access_token": "synthetic"}},
                     {"OPENAI_API_KEY": "synthetic", "tokens": {"access_token": "synthetic"}},
                     {"auth_mode": "chatgpt", "OPENAI_API_KEY": None,
                      "tokens": {"access_token": "synthetic-subscription"}},
                     # A ChatGPT login whose token exchange also gave it an API key.
                     {"auth_mode": "chatgpt", "OPENAI_API_KEY": "synthetic-exchanged",
                      "tokens": {"access_token": "synthetic-subscription"}}):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                credentials = root / "credentials"
                credentials.mkdir()
                key = credentials / "anthropic-api-key"
                staging = credentials / "anthropic-api-key.new"
                workspace = credentials / "anthropic-workspace"
                token = credentials / "claude-oauth-token"
                key.write_text("synthetic")
                token.write_text("synthetic-setup-token")
                staging.write_text("synthetic-interrupted-upload")
                workspace.write_text("synthetic-workspace")
                login = root / "home/.codex/auth.json"
                login.parent.mkdir(parents=True)
                login.write_text(json.dumps(auth))
                original = login.read_bytes()
                clear(root, False, False, False, False)
                self.assertTrue(key.exists() and workspace.exists() and token.exists())
                self.assertTrue(staging.exists())
                self.assertEqual(login.read_bytes(), original)
                clear(root, True, True, True, True)
                clear(root, True, True, True, True)
                self.assertFalse(key.exists() or workspace.exists() or token.exists())
                self.assertFalse(staging.exists())
                if auth.get("auth_mode") == "chatgpt":
                    self.assertEqual(login.read_bytes(), original)
                else:
                    self.assertFalse(login.exists())


if __name__ == "__main__":
    unittest.main()
