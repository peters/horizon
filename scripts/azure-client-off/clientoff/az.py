"""Bounded `az` calls without a shell; every mutation names the exact resource."""
from __future__ import annotations

import base64
import json
import subprocess
import time
from typing import Any, Dict, List, Optional
from .manifest import CLI_STEP_SECONDS, PUBLIC_IP_NAME, RUN_COMMAND_SECONDS, TAG_IMAGE_REF, WORKER_CONTAINER, utc_now


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

    def append_container_authorized_key(self, group: str, name: str, line: str) -> Optional[Any]:
        """Append one `authorized_keys` line inside the worker container through the ARM
        run-command channel; the line travels base64-encoded so no quoting layer can alter it."""
        encoded = base64.b64encode((line.rstrip("\n") + "\n").encode("ascii")).decode("ascii")
        script = (f"docker exec {WORKER_CONTAINER} sh -c 'umask 077 && mkdir -p /root/.ssh && "
                  f"echo {encoded} | base64 -d >> /root/.ssh/authorized_keys'")
        return self.run(["vm", "run-command", "invoke", "-g", group, "-n", name, "--command-id", "RunShellScript",
                         "--scripts", script], mutating=True, timeout=RUN_COMMAND_SECONDS)

    def remove_container_authorized_key(self, group: str, name: str, line: str) -> Optional[Any]:
        """Remove every `authorized_keys` line equal to `line` inside the worker container
        through the ARM run-command channel; the line travels base64-encoded and is
        matched whole and literally, so nothing else in the file is touched."""
        encoded = base64.b64encode(line.rstrip("\n").encode("ascii")).decode("ascii")
        keys = "/root/.ssh/authorized_keys"
        # The filtered copy goes to a unique same-directory temporary created by mktemp
        # (never a predictable path a planted symlink could redirect); grep exits 1 when
        # no line remains (still a success here) and 2 on an error, in which case the
        # temporary is removed and the original file is left exactly as it was; the
        # rename replaces the file atomically.
        script = (f"docker exec {WORKER_CONTAINER} sh -c 'umask 077 && k=$(echo {encoded} | base64 -d) && "
                  f"t=$(mktemp /root/.ssh/.authorized_keys.XXXXXX) && "
                  f"{{ grep -vxF \"$k\" {keys} > \"$t\"; rc=$?; [ $rc -le 1 ] || {{ rm -f \"$t\"; exit $rc; }}; }} && "
                  f"mv -f \"$t\" {keys}'")
        return self.run(["vm", "run-command", "invoke", "-g", group, "-n", name, "--command-id", "RunShellScript",
                         "--scripts", script], mutating=True, timeout=RUN_COMMAND_SECONDS)

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
