#!/usr/bin/env python3
"""Explicit one-shot task start and non-creating attachment inside one worker."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import uuid


class SessionError(Exception):
    """Messages contain no command, filesystem path or subprocess output."""


def private_directory(path):
    path.mkdir(mode=0o700, exist_ok=True)
    check_directory(path)


def check_directory(path):
    metadata = path.lstat()
    if not stat.S_ISDIR(metadata.st_mode) or stat.S_IMODE(metadata.st_mode) & 0o077:
        raise SessionError("session state directory is not private")


def sync_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


class PanelSessions:
    def __init__(self, repository, state, sockets, config, wrapper):
        self.repository = Path(repository)
        self.state = Path(state)
        self.sockets = Path(sockets)
        self.config = Path(config)
        self.wrapper = str(wrapper)

    def identities(self, runtime, panel):
        try:
            parsed = uuid.UUID(runtime)
        except ValueError as error:
            raise SessionError("invalid runtime identity") from error
        if str(parsed) != runtime or parsed.int == 0:
            raise SessionError("invalid runtime identity")
        if not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", panel):
            raise SessionError("invalid panel identity")
        return self.sockets / f"{runtime}.sock", f"horizon-panel-{panel}"

    def tmux(self, runtime, *arguments, check=True):
        socket, _ = self.identities(runtime, "validation")
        try:
            check_directory(self.sockets)
        except FileNotFoundError:
            pass
        environment = dict(os.environ)
        environment.pop("TMUX", None)
        # tmux parses trailing semicolons as command separators even without a
        # shell. Escape that parser layer for every argument, including cwd.
        escaped = [argument[:-1] + "\\;" if argument.endswith(";") else argument for argument in arguments]
        creation = [] if arguments[0] == "new-session" else ["-N"]
        result = subprocess.run(
            ["tmux", *creation, "-S", str(socket), "-f", str(self.config), *escaped],
            stdin=subprocess.DEVNULL, capture_output=True, timeout=5,
            env=environment, check=False,
        )
        if check and result.returncode:
            raise SessionError("session operation did not complete")
        return result

    def marker_path(self, runtime, panel):
        self.identities(runtime, panel)
        return self.state / runtime / f"{panel}.json"

    def read_marker(self, runtime, panel):
        path = self.marker_path(runtime, panel)
        check_directory(self.state)
        check_directory(path.parent)
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as stream:
            metadata = os.fstat(stream.fileno())
            if not stat.S_ISREG(metadata.st_mode) or stat.S_IMODE(metadata.st_mode) & 0o077:
                raise SessionError("session marker is not private")
            raw = stream.read(4097)
        if len(raw) > 4096:
            raise SessionError("session marker exceeds its limit")
        marker = json.loads(raw)
        expected = {"version", "runtime", "panel", "nonce", "intent"}
        if (
            not isinstance(marker, dict) or set(marker) != expected
            or type(marker["version"]) is not int or marker["version"] != 1
            or marker["runtime"] != runtime or marker["panel"] != panel
            or not isinstance(marker["nonce"], str)
            or not re.fullmatch(r"[0-9a-f]{32}", marker["nonce"])
            or not isinstance(marker["intent"], str)
            or not re.fullmatch(r"[0-9a-f]{64}", marker["intent"])
        ):
            raise SessionError("session marker identity is invalid")
        return marker

    def publish_marker(self, runtime, panel, marker):
        path = self.marker_path(runtime, panel)
        private_directory(self.state)
        private_directory(path.parent)
        sync_directory(self.state.parent)
        sync_directory(self.state)
        candidate = None
        try:
            with tempfile.NamedTemporaryFile(dir=path.parent, prefix=".claim-", delete=False) as stream:
                candidate = Path(stream.name)
                stream.write(json.dumps(marker, sort_keys=True).encode())
                stream.flush()
                os.fsync(stream.fileno())
            try:
                os.link(candidate, path, follow_symlinks=False)
                won = True
            except FileExistsError:
                won = False
            sync_directory(path.parent)
            return won
        finally:
            if candidate is not None:
                candidate.unlink()

    def launch_intent(self, directory, arguments):
        if not arguments or len(arguments) > 257 or not arguments[0] or arguments[0].startswith("-"):
            raise SessionError("invalid task command")
        if any("\0" in argument for argument in arguments) or sum(len(item.encode()) for item in arguments) > 65536:
            raise SessionError("task command exceeds its limit")
        requested = Path(directory)
        if requested.is_absolute() or ".." in requested.parts:
            raise SessionError("task directory must remain inside the repository")
        repository = self.repository.resolve(strict=True)
        working_directory = (repository / requested).resolve(strict=True)
        if not working_directory.is_dir() or not working_directory.is_relative_to(repository):
            raise SessionError("task directory must remain inside the repository")
        payload = json.dumps([str(requested), arguments], ensure_ascii=True).encode()
        return working_directory, hashlib.sha256(payload).hexdigest()

    def start(self, runtime, panel, directory, arguments):
        _, name = self.identities(runtime, panel)
        working_directory, intent = self.launch_intent(directory, arguments)
        try:
            marker = self.read_marker(runtime, panel)
        except FileNotFoundError:
            marker = None
        if marker is None:
            if self.tmux(runtime, "has-session", "-t", f"={name}", check=False).returncode == 0:
                raise SessionError("an unowned session already uses this panel identity")
            marker = {"version": 1, "runtime": runtime, "panel": panel, "nonce": uuid.uuid4().hex, "intent": intent}
            if self.publish_marker(runtime, panel, marker):
                private_directory(self.sockets)
                # The wrapper plus program always supplies multiple arguments to tmux,
                # so task argv is executed directly, never reconstructed as shell text.
                self.tmux(
                    runtime, "new-session", "-d", "-s", name, "-c", str(working_directory).replace("#", "##"),
                    "-e", f"HORIZON_PANEL_INSTANCE={marker['nonce']}", "--", self.wrapper, *arguments,
                )
            else:
                marker = self.read_marker(runtime, panel)
        if marker["intent"] != intent:
            raise SessionError("panel launch intent differs from its retained task")
        return self.status(runtime, panel)

    def status(self, runtime, panel):
        _, name = self.identities(runtime, panel)
        marker = self.read_marker(runtime, panel)
        observed = self.tmux(runtime, "show-environment", "-t", f"={name}", "HORIZON_PANEL_INSTANCE", check=False)
        if observed.returncode:
            return {"state": "unavailable", "panel": panel}
        expected = f"HORIZON_PANEL_INSTANCE={marker['nonce']}\n".encode()
        if observed.stdout != expected:
            raise SessionError("session ownership does not match the retained task")
        result = self.tmux(runtime, "list-panes", "-s", "-t", f"={name}", "-F", "#{pane_dead}|#{pane_dead_status}|#{pane_pid}")
        fields = result.stdout.decode("ascii").strip().split("|")
        if len(fields) != 3 or fields[0] not in ("0", "1") or not fields[2].isdigit():
            raise SessionError("session status is invalid")
        exited = fields[0] == "1"
        if fields[1] and not fields[1].isdigit():
            raise SessionError("session exit status is invalid")
        return {"state": "exited" if exited else "running", "panel": panel, "pid": int(fields[2]),
                "exit_status": int(fields[1]) if exited and fields[1] else None}

    def attach(self, runtime, panel):
        if self.status(runtime, panel)["state"] == "unavailable":
            raise SessionError("retained task is unavailable; attachment never starts a replacement")
        socket, name = self.identities(runtime, panel)
        os.environ.pop("TMUX", None)
        os.execvp("tmux", ["tmux", "-N", "-S", str(socket), "-f", str(self.config),
                           "attach-session", "-t", f"={name}"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("start", "status", "attach"))
    parser.add_argument("runtime")
    parser.add_argument("panel")
    parser.add_argument("arguments", nargs=argparse.REMAINDER)
    arguments = parser.parse_args()
    service = PanelSessions("/workspace/horizon", "/var/lib/horizon/panels", "/run/horizon/panels",
                            "/etc/horizon/tmux.conf", "/usr/local/bin/horizon-agent-session")
    try:
        if arguments.operation == "start":
            if len(arguments.arguments) < 3 or arguments.arguments[1] != "--":
                raise SessionError("start requires a relative directory, --, and a program")
            result = service.start(arguments.runtime, arguments.panel, arguments.arguments[0], arguments.arguments[2:])
        elif arguments.arguments:
            raise SessionError("unexpected session arguments")
        elif arguments.operation == "attach":
            service.attach(arguments.runtime, arguments.panel)
            return
        else:
            result = service.status(arguments.runtime, arguments.panel)
        print(json.dumps(result, sort_keys=True))
    except SessionError as error:
        print(f"panel session: {error}", file=sys.stderr)
        sys.exit(1)
    except (OSError, ValueError, subprocess.TimeoutExpired):
        print("panel session: operation failed; no replacement task was requested", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
