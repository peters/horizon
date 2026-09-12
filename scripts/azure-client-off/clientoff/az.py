"""Bounded `az` calls without a shell; every mutation names the exact resource."""
from __future__ import annotations

import base64
import json
import subprocess
import time
from typing import Any, Dict, List, Optional
from .manifest import CLI_STEP_SECONDS, PUBLIC_IP_NAME, RUN_COMMAND_SECONDS, TAG_IMAGE_REF, WORKER_CONTAINER, utc_now


AUTHORIZED_KEYS_EDITOR = """import base64, os, stat, sys
action, line = sys.argv[1], base64.b64decode(sys.argv[2]).decode("ascii")
flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
os.umask(0o077)


def write_all(fd, data):
    view = memoryview(data)
    while view:
        written = os.write(fd, view)
        view = view[written:]


try:
    root = os.open("/root", flags)
    try:
        os.mkdir(".ssh", 0o700, dir_fd=root)
    except FileExistsError:
        pass
    ssh = os.open(".ssh", flags, dir_fd=root)
    os.close(root)
    # The existing file is read through a descriptor that follows no symlink and is
    # accepted only as a plain regular file with a single link: a FIFO or device would
    # block or divert the write, a hard link would reach another file.
    entries = []
    try:
        fd = os.open("authorized_keys", os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=ssh)
    except FileNotFoundError:
        fd = None
    if fd is not None:
        with os.fdopen(fd, "rb") as handle:
            info = os.fstat(handle.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
                sys.exit(3)
            entries = [entry for entry in handle.read().split(b"\\n") if entry]
    if action == "append":
        entries.append(line.encode("ascii"))
    else:
        entries = [entry for entry in entries if entry != line.encode("ascii")]
    # Both edits go through a fresh same-directory temporary written in full, synced,
    # and renamed over the key file atomically, so no partial or in-place write exists.
    name = ".authorized_keys." + base64.b16encode(os.urandom(6)).decode("ascii")
    tmp = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=ssh)
    try:
        write_all(tmp, b"\\n".join(entries) + (b"\\n" if entries else b""))
        os.fsync(tmp)
    finally:
        os.close(tmp)
    os.replace(name, "authorized_keys", src_dir_fd=ssh, dst_dir_fd=ssh)
    os.close(ssh)
except OSError as error:
    sys.stderr.write("authorized_keys editor: %s\\n" % error)
    sys.exit(2)
"""


class Az:
    """Bounded `az` calls without a shell; every mutation names the exact resource."""

    def __init__(self, subscription: str, dry_run: bool = False) -> None:
        self.subscription, self.dry_run, self.journal = subscription, dry_run, []
        # An absolute phase deadline (monotonic seconds) once a phase sets one: every
        # call is cut off at it and none starts past it, so no read, probe, mutation or
        # poll can outlive the window the manifest reserved for the phase.
        self.deadline: Optional[float] = None

    def left(self) -> Optional[float]:
        """Seconds left until the phase deadline, or None when no deadline is set."""
        return None if self.deadline is None else self.deadline - time.monotonic()

    def run(self, args: List[str], mutating: bool = False, timeout: float = CLI_STEP_SECONDS) -> Optional[Any]:
        command = ["az", *args, "--subscription", self.subscription, "-o", "json"]
        self.journal.append({"at": utc_now().isoformat(), "mutating": mutating, "args": args})
        if self.dry_run and mutating:
            return None
        if timeout < 1:
            return None  # no budget left for this call: it is not made
        left = self.left()
        if left is not None:
            if left < 1:
                return None  # past the phase deadline: nothing more is asked of ARM
            timeout = min(timeout, left)
        try:
            completed = subprocess.run(command, capture_output=True, text=True, timeout=timeout, check=False)
        except (subprocess.TimeoutExpired, OSError):
            # A missing or unexecutable `az` is an unreadable answer, like a timeout.
            return None
        if completed.returncode != 0 or not completed.stdout.strip():
            return None
        try:
            return json.loads(completed.stdout)
        except json.JSONDecodeError:
            return None

    def power_state(self, group: str, name: str, timeout: int = CLI_STEP_SECONDS) -> Optional[str]:
        view = self.run(["vm", "get-instance-view", "-g", group, "-n", name], timeout=timeout)
        if not isinstance(view, dict):
            return None
        instance_view = view.get("instanceView")
        statuses = instance_view.get("statuses") if isinstance(instance_view, dict) else None
        if not isinstance(statuses, list):
            return None
        # Every entry must be well formed and exactly one may carry a power state: a
        # view naming two states, or one that cannot be read, is no state at all.
        codes = []
        for status in statuses:
            code = status.get("code") if isinstance(status, dict) else None
            if not isinstance(code, str):
                return None
            if code.startswith("PowerState/"):
                codes.append(code)
        return codes[0] if len(codes) == 1 else None

    def edit_container_authorized_keys(self, group: str, name: str, line: str, action: str) -> Optional[Any]:
        """Append or remove exactly one `authorized_keys` line inside the worker container
        through the ARM run-command channel. The editor runs as python3 in the container,
        opens `/root/.ssh` with `O_DIRECTORY|O_NOFOLLOW` and the key file relative to that
        directory descriptor with `O_NOFOLLOW`, accepts it only as a single-link regular
        file (no symlink, hard link, FIFO or device can divert or block a root write), and
        writes both edits in full to a fresh same-directory temporary that replaces the
        file atomically. Program and line travel base64-encoded so no quoting layer can
        alter them."""
        encoded_program = base64.b64encode(AUTHORIZED_KEYS_EDITOR.encode("ascii")).decode("ascii")
        encoded_line = base64.b64encode(line.rstrip("\n").encode("ascii")).decode("ascii")
        script = (f"docker exec {WORKER_CONTAINER} sh -c 'echo {encoded_program} | base64 -d | "
                  f"python3 - {action} {encoded_line}'")
        return self.run(["vm", "run-command", "invoke", "-g", group, "-n", name, "--command-id", "RunShellScript",
                         "--scripts", script], mutating=True, timeout=RUN_COMMAND_SECONDS)

    def append_container_authorized_key(self, group: str, name: str, line: str) -> Optional[Any]:
        return self.edit_container_authorized_keys(group, name, line, "append")

    def remove_container_authorized_key(self, group: str, name: str, line: str) -> Optional[Any]:
        return self.edit_container_authorized_keys(group, name, line, "remove")

    def vm_identity(self, group: str, name: str) -> Dict[str, Optional[str]]:
        vm = self.run(["vm", "show", "-g", group, "-n", name])
        group_info = self.run(["group", "show", "-n", group])
        # The endpoint is read from ARM every sample and never trusted from the worker file.
        address = self.run(["network", "public-ip", "show", "-g", group, "-n", PUBLIC_IP_NAME])
        tags = (vm or {}).get("tags") if isinstance(vm, dict) else None
        return {"b_group_id": (group_info or {}).get("id") if isinstance(group_info, dict) else None,
                "b_vm_id": (vm or {}).get("id") if isinstance(vm, dict) else None,
                "b_instance_id": (vm or {}).get("vmId") if isinstance(vm, dict) else None,
                "b_image_ref": tags.get(TAG_IMAGE_REF) if isinstance(tags, dict) else None,
                "b_host": (address or {}).get("ipAddress") if isinstance(address, dict) else None}
