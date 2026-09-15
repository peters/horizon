#!/usr/bin/env python3
"""Credential-free filesystem-sandbox qualification for the installed worker CLI."""

import json
import os
from pathlib import Path
import shutil
import selectors
import signal
import subprocess
import sys
import tempfile
import time

from processes import adopt_orphans, cleanup_descendants


PROFILE = ":workspace"
CANARY = r"""
import errno, pathlib, sys
workspace, outside = map(pathlib.Path, sys.argv[1:])
(workspace / "allowed").write_text("sandbox write\n")
assert (workspace / "allowed").read_text() == "sandbox write\n"
assert (outside / "existing").read_text() == "unchanged\n"
for path in (outside / "existing", outside / "new"):
    try:
        path.write_text("unexpected write\n")
    except OSError as error:
        if error.errno not in (errno.EACCES, errno.EPERM, errno.EROFS):
            raise
    else:
        raise RuntimeError("outside write was allowed")
print("WORKER_SANDBOX_CANARY_PASSED")
"""


def command(argv, environment, directory, timeout=30):
    deadline = time.monotonic() + timeout
    with subprocess.Popen(
        argv, cwd=directory, env=environment, stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
    ) as process:
        try:
            result = bytearray()
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                while True:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0 or not selector.select(remaining):
                        raise RuntimeError("sandbox command timed out")
                    chunk = os.read(process.stdout.fileno(), 4097 - len(result))
                    if not chunk:
                        break
                    result.extend(chunk)
                    if len(result) > 4096:
                        raise RuntimeError("sandbox command returned excessive output")
            code = process.wait(timeout=max(0, deadline - time.monotonic()))
            if code:
                raise RuntimeError("sandbox command failed; runtime is not qualified")
            return result.decode("utf-8").strip()
        finally:
            if process.poll() is None:
                process.kill()
            process.wait()


def qualify(root, executable):
    workspace = root / "workspace"
    outside = root / "outside"
    home = root / "home"
    for path in (workspace, outside, home):
        path.mkdir(mode=0o700)
    (home / ".codex").mkdir(mode=0o700)
    # /var/tmp intentionally differs from the writable /tmp convenience grant.
    # Both canaries must be writable by this exact UID before sandboxing.
    (workspace / "allowed").write_text("baseline\n")
    (outside / "existing").write_text("unchanged\n")
    (outside / "new").write_text("baseline\n")
    (outside / "new").unlink()
    environment = {
        "PATH": "/usr/local/bin:/usr/bin:/bin",
        "LANG": "C.UTF-8",
        "HOME": str(home),
        "CODEX_HOME": str(home / ".codex"),
        "TMPDIR": "/tmp",
    }
    version = command([executable, "--version"], environment, workspace)
    output = command([
        executable, "sandbox", "-P", PROFILE, "-C", str(workspace), "--",
        sys.executable, "-I", "-c", CANARY, str(workspace), str(outside),
    ], environment, workspace)
    if output != "WORKER_SANDBOX_CANARY_PASSED":
        raise RuntimeError("sandbox canary did not complete")
    if (workspace / "allowed").read_text() != "sandbox write\n":
        raise RuntimeError("workspace write was not retained")
    if (outside / "existing").read_text() != "unchanged\n" or (outside / "new").exists():
        raise RuntimeError("outside files changed")
    return {"passed": True, "profile": PROFILE, "agent_version": version,
            "uid": os.getuid(), "checks": ["workspace_write", "outside_overwrite_denied",
                                          "outside_create_denied", "outside_bytes_preserved"]}


def main():
    result = {"passed": False, "profile": PROFILE}
    def interrupted(_number, _frame):
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        raise RuntimeError("sandbox qualification interrupted")

    previous_handlers = {}
    try:
        for number in (signal.SIGTERM, signal.SIGINT):
            previous_handlers[number] = signal.getsignal(number)
            signal.signal(number, interrupted)
        adopt_orphans()
        executable = shutil.which("codex", path="/usr/local/bin:/usr/bin:/bin")
        if not executable:
            raise RuntimeError("worker agent CLI is missing")
        with tempfile.TemporaryDirectory(prefix="horizon-agent-sandbox-", dir="/var/tmp") as directory:
            try:
                result = qualify(Path(directory), executable)
            finally:
                for number in previous_handlers:
                    signal.signal(number, signal.SIG_IGN)
                if not cleanup_descendants():
                    raise RuntimeError("sandbox descendant cleanup failed")
    except RuntimeError as error:
        result = {"passed": False, "profile": PROFILE, "reason": str(error)}
    except (OSError, UnicodeError, subprocess.TimeoutExpired):
        # Never emit arbitrary CLI output, environment or private paths as proof.
        result = {"passed": False, "profile": PROFILE,
                  "reason": "worker agent sandbox qualification failed"}
    finally:
        for number, handler in previous_handlers.items():
            signal.signal(number, handler)
    print(json.dumps(result, sort_keys=True))
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
