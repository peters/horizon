"""Remove unbound worker API credentials without deleting ChatGPT or subscription logins."""
import json
from pathlib import Path
import sys


def clear_api_auth(root, anthropic, openai, workspace):
    if anthropic:
        (root / "credentials/anthropic-api-key").unlink(missing_ok=True)
        (root / "credentials/anthropic-api-key.new").unlink(missing_ok=True)
    if workspace:
        (root / "credentials/anthropic-workspace").unlink(missing_ok=True)
    if openai:
        path = root / "home/.codex/auth.json"
        try:
            auth = json.loads(path.read_text())
        except FileNotFoundError:
            return
        if not isinstance(auth, dict):
            raise ValueError("Invalid agent authentication record")
        # A ChatGPT login can carry an API key from its own token exchange; it stays.
        mode = auth.get("auth_mode")
        if mode == "apikey" or (mode is None and auth.get("OPENAI_API_KEY") and not auth.get("tokens")):
            path.unlink()


if __name__ == "__main__":
    clear_api_auth(Path("/workspace"), *(flag == "1" for flag in sys.argv[1:]))
