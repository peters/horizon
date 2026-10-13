"""Install Horizon's ChatGPT login as the worker's Codex login, and print the login kept.

With empty input, print the worker's login without changing it.

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
import time

LIMIT = 64 * 1024
# How long a write that Codex started on the replaced file is given to finish.
SETTLE = 0.2
ATTEMPTS = 5
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


def bounded(handle):
    """The whole file behind `handle`, or None when it is too large to be a login."""
    handle.seek(0)
    data = handle.read(LIMIT + 1)
    return data if len(data) <= LIMIT else None


def replace(directory, path, data):
    """Replaces `path` with `data` in one durable rename."""
    fd, pending = tempfile.mkstemp(dir=directory, prefix=".auth-")
    try:
        with os.fdopen(fd, "wb") as output:
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
        os.replace(pending, path)
        handle = os.open(directory, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(handle)
        finally:
            os.close(handle)
    finally:
        Path(pending).unlink(missing_ok=True)


def keep(current, data):
    """Returns the worker's own login, made private if it was not."""
    try:
        os.fchmod(current.fileno(), 0o600)
    except OSError:
        pass
    return data


def look(home):
    """The worker's login, unchanged, or nothing when it has no ChatGPT login."""
    try:
        with (home / ".codex" / "auth.json").open("rb") as current:
            data = bounded(current)
            if data is None or login(data) is None:
                return b""
            return keep(current, data)
    except FileNotFoundError:
        return b""


def install(home, incoming_bytes, settle=lambda: time.sleep(SETTLE)):
    """Installs the incoming login unless the worker's renews it; returns the login kept.

    Without an incoming login, only returns the worker's login: Horizon takes a later
    renewal from it without giving a login it may no longer use.

    Codex rewrites its login in place when it renews it. A renewal written while this runs
    is never lost: a file that changed before the rename is read again, and a renewal that
    Codex writes into the replaced file within the settle time is put back.
    """
    if not incoming_bytes:
        return look(home)
    incoming = login(incoming_bytes)
    if len(incoming_bytes) > LIMIT or incoming is None:
        raise ValueError("Invalid ChatGPT login")
    directory = home / ".codex"
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    path = directory / "auth.json"
    for _ in range(ATTEMPTS):
        try:
            current = path.open("rb")
        except FileNotFoundError:
            current = None
        try:
            before = bounded(current) if current else None
            if before is not None and renews(login(before), incoming):
                return keep(current, before)
            if current is not None and bounded(current) != before:
                continue
            replace(directory, path, incoming_bytes)
            if current is None:
                return incoming_bytes
            # Watched for the whole settle time: Codex may start its write late, and it
            # truncates before it writes, so only a complete changed login ends the wait.
            after = before
            for _ in range(ATTEMPTS):
                settle()
                after = bounded(current)
                if after != before and after is not None and login(after) is not None:
                    break
            if after != before and after is not None and renews(login(after), incoming):
                replace(directory, path, after)
                return after
            return incoming_bytes
        finally:
            if current is not None:
                current.close()
    raise RuntimeError("The Codex login kept changing while Horizon installed it")


if __name__ == "__main__":
    os.umask(0o077)
    kept = install(Path(sys.argv[1]), sys.stdin.buffer.read(LIMIT + 1))
    sys.stdout.buffer.write(kept)
