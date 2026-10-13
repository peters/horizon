"""Install Horizon's ChatGPT login as the worker's Codex login, and print the login kept.

The worker keeps its own login instead when that login is a later renewal of the same
account: ChatGPT refresh tokens are single use, so the renewal must not be replaced by the
copy it spent. Horizon adopts the printed login when it is newer than its own.
"""
from datetime import datetime
import json
import os
from pathlib import Path
import re
import sys
import tempfile

LIMIT = 64 * 1024
STAMP = re.compile(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.(\d{1,9}))?(Z|[+-]\d\d:\d\d)")


def login(data):
    """The account and renewal time of a ChatGPT login, or None for anything else."""
    try:
        auth = json.loads(data)
    except ValueError:
        return None
    if not isinstance(auth, dict) or auth.get("auth_mode") != "chatgpt":
        return None
    tokens = auth.get("tokens")
    if not isinstance(tokens, dict):
        return None
    if not all(isinstance(tokens.get(name), str) and tokens[name] for name in ("access_token", "refresh_token", "account_id")):
        return None
    return tokens["account_id"], refreshed(auth.get("last_refresh"))


def refreshed(value):
    """An RFC 3339 time as (seconds, nanoseconds), or None; Codex writes nanoseconds."""
    match = STAMP.fullmatch(value) if isinstance(value, str) else None
    if match is None:
        return None
    whole, fraction, zone = match.groups()
    try:
        stamp = datetime.fromisoformat(whole + ("+00:00" if zone == "Z" else zone))
    except ValueError:
        return None
    return int(stamp.timestamp()), int((fraction or "0").ljust(9, "0"))


def renews(current, incoming):
    """Whether the worker's login is a later renewal of the incoming login's account."""
    if current is None or current[0] != incoming[0] or current[1] is None:
        return False
    return incoming[1] is None or current[1] > incoming[1]


def install(home, incoming_bytes):
    """Installs the incoming login unless the worker's renews it; returns the login kept."""
    incoming = login(incoming_bytes)
    if len(incoming_bytes) > LIMIT or incoming is None:
        raise ValueError("Invalid ChatGPT login")
    directory = home / ".codex"
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    path = directory / "auth.json"
    try:
        with path.open("rb") as current_file:
            current_bytes = current_file.read(LIMIT + 1)
    except FileNotFoundError:
        current_bytes = None
    if current_bytes is not None and len(current_bytes) <= LIMIT and renews(login(current_bytes), incoming):
        return current_bytes
    fd, pending = tempfile.mkstemp(dir=directory, prefix=".auth-")
    try:
        with os.fdopen(fd, "wb") as output:
            output.write(incoming_bytes)
            output.flush()
            os.fsync(output.fileno())
        os.replace(pending, path)
        flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0)
        handle = os.open(directory, flags)
        try:
            os.fsync(handle)
        finally:
            os.close(handle)
    finally:
        Path(pending).unlink(missing_ok=True)
    return incoming_bytes


if __name__ == "__main__":
    os.umask(0o077)
    kept = install(Path(sys.argv[1]), sys.stdin.buffer.read(LIMIT + 1))
    sys.stdout.buffer.write(kept)
