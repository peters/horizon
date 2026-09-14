"""Linux process-local orphan adoption for a standalone smoke supervisor."""

import ctypes
import os
from pathlib import Path
import signal
import time


def adopt_orphans():
    # PR_SET_CHILD_SUBREAPER affects only this supervisor and its descendants.
    # It retains ownership when a crashed Horizon leaves a separately sessioned
    # PTY child; process-group membership alone cannot cover that case.
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "cannot adopt smoke descendants")


def children():
    path = Path(f"/proc/self/task/{os.getpid()}/children")
    return [int(value) for value in path.read_text().split()]


def cleanup_descendants():
    """Reap only this process's children, including newly adopted orphans."""
    deadline = time.monotonic() + 10
    graceful_until = time.monotonic() + 5
    while True:
        current = children()
        if not current:
            return True
        if time.monotonic() >= deadline:
            return False
        for pid in current:
            try:
                waited, _status = os.waitpid(pid, os.WNOHANG)
                if waited:
                    continue
                # The unreaped direct child cannot have its PID reused. pidfd
                # keeps signalling attached to that exact process regardless.
                descriptor = os.pidfd_open(pid)
                try:
                    requested = signal.SIGTERM if time.monotonic() < graceful_until else signal.SIGKILL
                    signal.pidfd_send_signal(descriptor, requested)
                finally:
                    os.close(descriptor)
            except (ChildProcessError, ProcessLookupError):
                continue
        time.sleep(0.05)
