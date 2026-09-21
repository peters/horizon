"""Remove unbound worker API credentials without deleting subscription login state."""
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
        if auth.get("auth_mode") == "apikey" or auth.get("OPENAI_API_KEY"):
            path.unlink()


if __name__ == "__main__":
    clear_api_auth(Path("/workspace"), *(flag == "1" for flag in sys.argv[1:]))
