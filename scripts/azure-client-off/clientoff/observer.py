"""Observer C's only reach into worker B: the descriptor gates, the forced reader installed behind a restricted key, and the pinned read."""
from __future__ import annotations

import base64
import json
import os
import re
import selectors
import subprocess
import tempfile
import time
from typing import Any, Dict, List, Optional
from .manifest import HOST_KEY_RE, INSTANCE_ID_RE, PROGRESS_PATH_RE, PROGRESS_READ_LIMIT, routable


# The adapter names the worker VM inside its group; any other VM in it is not B.
WORKER_VM_NAME = "worker"
WORKER_FIELDS = ("vm_name", "port", "host_key", "observer_key_path", "progress_path", "group_id", "vm_id", "instance_id",
                 "host")


def validate_worker(worker: Any) -> List[str]:
    """Fail-closed schema for the observer's worker descriptor, checked before A is stopped."""
    if not isinstance(worker, dict):
        return ["worker descriptor is not an object"]
    problems = [f"missing {field}" for field in WORKER_FIELDS if field not in worker]
    if problems:
        return problems
    if worker["vm_name"] != WORKER_VM_NAME:
        problems.append(f"vm_name must be {WORKER_VM_NAME!r}, the adapter's fixed worker VM name")
    if type(worker["port"]) is not int or not 1 <= worker["port"] <= 65535:  # noqa: E721
        problems.append("port is not a TCP port")
    if not isinstance(worker["host_key"], str) or not HOST_KEY_RE.fullmatch(worker["host_key"]):
        problems.append("host_key is not a single Ed25519 host key line")
    if not isinstance(worker["observer_key_path"], str) or not os.path.isfile(worker["observer_key_path"]):
        problems.append("observer_key_path is not a readable file")
    if not isinstance(worker["progress_path"], str) or not PROGRESS_PATH_RE.fullmatch(worker["progress_path"]):
        problems.append("progress_path is not a plain path under /workspace")
    checkpoint = worker.get("checkpoint_path")
    if checkpoint is not None and (not isinstance(checkpoint, str) or not PROGRESS_PATH_RE.fullmatch(checkpoint)):
        problems.append("checkpoint_path is not a plain path under /workspace")
    elif checkpoint is not None and checkpoint == worker.get("progress_path"):
        problems.append("checkpoint_path must be a separate worker-owned file, not the progress counter")
    for field in ("group_id", "vm_id"):
        if not isinstance(worker[field], str) or not worker[field]:
            problems.append(f"{field} (the identity recorded at baseline) is not a string")
    if not isinstance(worker["instance_id"], str) or not INSTANCE_ID_RE.fullmatch(worker["instance_id"]):
        problems.append("instance_id (the worker VM's vmId recorded at baseline) is not an instance identity")
    if not routable(worker["host"]):
        problems.append("host (the address recorded at baseline) is not a public IP address")
    return problems


READER_SOURCE = """import json, os, stat, sys
paths = json.loads(sys.argv[1])
root = sys.argv[2]
limit = int(sys.argv[3])


def read(path):
    if path is None:
        return None
    fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        parts = path[len(root):].strip("/").split("/")
        for index, part in enumerate(parts):
            last = index == len(parts) - 1
            # Non-blocking on the last component: a FIFO planted at the path would
            # otherwise block the open before the type check; a regular file is unaffected.
            flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NOCTTY | (os.O_NONBLOCK if last else os.O_DIRECTORY)
            next_fd = os.open(part, flags, dir_fd=fd)
            os.close(fd)
            fd = next_fd
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode):
            return None
        data = os.read(fd, limit + 1)
        if len(data) > limit:
            return None
        return data.decode("ascii", "replace"), (info.st_dev, info.st_ino)
    except OSError:
        return None
    finally:
        os.close(fd)


answers = {name: read(path) for name, path in paths.items()}
progress, checkpoint = answers.get("progress"), answers.get("checkpoint")
# A checkpoint file that is the counter file under another name (a hard link) is
# counter progress, never checkpoint proof.
if progress and checkpoint and progress[1] == checkpoint[1]:
    checkpoint = None
sys.stdout.write(json.dumps({"progress": progress[0] if progress else None,
                             "checkpoint": checkpoint[0] if checkpoint else None,
                             "paths": paths}))
"""


def reader_command(progress_path: str, checkpoint_path: Optional[str], root: str = "/workspace") -> str:
    """The forced command for the observer key on B: a python reader that opens each
    path component with `O_NOFOLLOW` below `root`, reads a capped prefix of regular
    files only and prints one JSON object. Source and arguments travel base64-encoded,
    so the command holds no quotes and sshd/shell quoting cannot alter it; whatever
    command the client sends is ignored by sshd because of the forced command.
    """
    for path in (progress_path, checkpoint_path):
        if path is not None and (not path.startswith(root + "/") or any(part in (".", "..") for part in path.split("/"))):
            raise ValueError(f"observer paths must lie below {root}")
    paths = json.dumps({"progress": progress_path, "checkpoint": checkpoint_path})
    source = base64.b64encode(READER_SOURCE.encode("ascii")).decode("ascii")
    argument = base64.b64encode(paths.encode("ascii")).decode("ascii")
    return (f"python3 -c \"$(echo {source} | base64 -d)\" \"$(echo {argument} | base64 -d)\" "
            f"{root} {PROGRESS_READ_LIMIT}")


def observer_authorized_line(public_key: str, progress_path: str, checkpoint_path: Optional[str]) -> str:
    """The `authorized_keys` line that makes the observer key a read-only channel:
    `restrict` (no pty, port/agent/X11 forwarding or user rc) plus the forced reader.
    Double quotes inside the command are escaped the one way sshd dequotes them."""
    if not HOST_KEY_RE.fullmatch(public_key.strip()) or not PROGRESS_PATH_RE.fullmatch(progress_path):
        raise ValueError("observer key must be one Ed25519 line and the path must lie under /workspace")
    if checkpoint_path is not None and (not PROGRESS_PATH_RE.fullmatch(checkpoint_path) or checkpoint_path == progress_path):
        raise ValueError("checkpoint path must be a separate plain path under /workspace")
    command = reader_command(progress_path, checkpoint_path).replace('"', '\\"')
    return f'restrict,command="{command}" {public_key.strip()}'


def derived_public_key(private_key_path: str, budget_seconds: float = 30) -> Optional[str]:
    """The public half of an Ed25519 private key, derived with `ssh-keygen -y` under a
    bound; None when it cannot be derived (or no budget is left to derive it)."""
    if budget_seconds < 1:
        return None
    try:
        # No stdin and an empty passphrase: a protected key fails here instead of
        # prompting, and could never serve the BatchMode observer sessions anyway.
        completed = subprocess.run(["ssh-keygen", "-y", "-P", "", "-f", private_key_path], capture_output=True, text=True,
                                   timeout=min(30, budget_seconds), check=False, stdin=subprocess.DEVNULL)
    except (OSError, subprocess.TimeoutExpired):
        return None
    if completed.returncode != 0:
        return None
    key = " ".join(completed.stdout.strip().split("\n")[0].split()[:2])
    return key if HOST_KEY_RE.fullmatch(key) else None


def parse_counter(text: Any) -> Optional[int]:
    """Exactly one non-negative decimal integer, or nothing."""
    if not isinstance(text, str) or not re.fullmatch(r"\s*[0-9]{1,18}\s*", text):
        return None
    return int(text.strip())


# The remote port whose forwarding every observation requests, expecting sshd to deny it.
FORWARD_PROBE_PORT = 47331
# One observation may take this long; a phase hands over less when less is left.
OBSERVATION_SECONDS = 30
# An observation given less than this cannot say anything: it is not started.
OBSERVATION_MIN_SECONDS = 10


def read_observations(host: str, port: int, host_key: str, key_path: str, directory: str,
                      budget_seconds: float = OBSERVATION_SECONDS) -> Dict[str, Any]:
    """One pinned SSH session with the observer key and no command: the server's forced
    reader answers. Output is streamed under a cap and a deadline; anything unexpected
    is an unreadable sample, never a crash.

    `channel` tells the outcomes apart: `answered` (the forced reader replied and the
    session was denied both a pty and a remote port forward, as `restrict` demands),
    `unrestricted` (the forced reader replied but a denied capability was granted: a
    `command=` line without `restrict`), `refused` (the
    server explicitly refused the key: publickey authentication denied under the pinned
    host key), `unavailable` (transport, pin, timeout or any other failure: nothing is
    known either way). A pty and a remote port forward are requested on every session
    precisely so that their refusal proves the option set, not merely the forced
    command.
    """
    nothing: Dict[str, Any] = {"progress": None, "checkpoint": None, "channel": "unavailable", "paths": None}
    if not HOST_KEY_RE.fullmatch(host_key) or not routable(host) or budget_seconds < OBSERVATION_MIN_SECONDS:
        return nothing
    budget_seconds = min(float(OBSERVATION_SECONDS), budget_seconds)
    # The pin lives in a private per-observation file so concurrent runs in one
    # directory never share or overwrite it.
    try:
        with tempfile.NamedTemporaryFile("w", dir=directory, prefix="observer_known_hosts.", suffix=".tmp",
                                         delete=False, encoding="utf-8") as handle:
            # OpenSSH looks a default-port host up by its bare address; the bracketed
            # form is for any other port.
            handle.write((f"{host} {host_key}\n" if port == 22 else f"[{host}]:{port} {host_key}\n"))
            known_hosts = handle.name
    except OSError:
        return nothing
    try:
        return _observe(host, port, key_path, known_hosts, budget_seconds)
    finally:
        try:
            os.unlink(known_hosts)
        except OSError:
            pass


def _observe(host: str, port: int, key_path: str, known_hosts: str, budget_seconds: float) -> Dict[str, Any]:
    nothing: Dict[str, Any] = {"progress": None, "checkpoint": None, "channel": "unavailable", "paths": None}
    # A command is sent on purpose: with the restricted key the server must ignore it,
    # which the JSON answer (and never this command's output) proves on every sample.
    # `-tt` forces a pty request: under `restrict` sshd denies it ("PTY allocation
    # request failed") and the forced reader still answers; a `command=` line without
    # `restrict` grants it, which is the difference between a read-only channel and a
    # key that may forward ports or agents.
    command = ["ssh", "-F", "/dev/null", "-i", key_path, "-p", str(port), "-o", f"UserKnownHostsFile={known_hosts}",
               "-o", "GlobalKnownHostsFile=/dev/null", "-o", "StrictHostKeyChecking=yes", "-o", "BatchMode=yes",
               "-o", f"ConnectTimeout={max(1, min(10, int(budget_seconds)))}", "-o", "IdentitiesOnly=yes", "-tt",
               "-R", f"127.0.0.1:{FORWARD_PROBE_PORT}:127.0.0.1:1", f"root@{host}", "id"]
    try:
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    except OSError:
        return nothing
    # Both streams are read under the same byte cap and one deadline: a hostile or
    # failing endpoint can fill neither the memory nor the disk of the controller.
    streams = {process.stdout: b"", process.stderr: b""}
    limit = 4 * PROGRESS_READ_LIMIT
    try:
        deadline = time.monotonic() + budget_seconds
        with selectors.DefaultSelector() as selector:
            for stream in streams:
                selector.register(stream, selectors.EVENT_READ)
            while selector.get_map():
                left = deadline - time.monotonic()
                if left <= 0:
                    raise TimeoutError
                for key, _ in selector.select(timeout=left):
                    chunk = os.read(key.fileobj.fileno(), limit + 1 - len(streams[key.fileobj]))
                    if not chunk:
                        selector.unregister(key.fileobj)
                        continue
                    streams[key.fileobj] += chunk
                    if len(streams[key.fileobj]) > limit:
                        raise ValueError("output over the cap")
        returncode = process.wait(timeout=max(0.1, deadline - time.monotonic()))
    except (TimeoutError, ValueError, subprocess.TimeoutExpired, OSError):
        try:
            process.kill()
        except OSError:
            pass  # already gone: nothing left to stop
        process.wait()
        return nothing
    output = streams[process.stdout]
    diagnostics = streams[process.stderr].decode("utf-8", "replace")
    if returncode != 0:
        # ssh exits 255 for its own failures; only an explicit publickey refusal under
        # the pinned host key is knowledge that the key is not authorized.
        if returncode == 255 and "Permission denied (publickey" in diagnostics and "Host key verification failed" not in diagnostics:
            return dict(nothing, channel="refused")
        return nothing
    try:
        answer = json.loads(output.decode("ascii").replace("\r", ""))
    except (UnicodeDecodeError, json.JSONDecodeError):
        return nothing
    if not isinstance(answer, dict) or set(answer) != {"progress", "checkpoint", "paths"} \
            or not isinstance(answer["paths"], dict) or set(answer["paths"]) != {"progress", "checkpoint"}:
        return nothing  # the forced reader did not answer: the key is not restricted as required
    # `restrict` denies both the pty and the port forward; sshd offers no way to read
    # an authorized_keys option set from the client, so the denied capabilities are the
    # proof (agent and X11 forwarding are disabled server-wide in the worker image).
    restricted = "PTY allocation request failed" in diagnostics and "remote port forwarding failed" in diagnostics
    channel = "answered" if restricted else "unrestricted"
    return {"progress": parse_counter(answer.get("progress")), "checkpoint": parse_counter(answer.get("checkpoint")),
            "channel": channel, "paths": answer["paths"]}
