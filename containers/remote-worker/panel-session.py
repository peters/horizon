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

MAX_REQUEST_BYTES = 512 * 1024


class SessionError(Exception):
    """Messages contain no command, filesystem path or subprocess output."""


def unique_fields(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise SessionError("structured session request contains duplicate fields")
        result[key] = value
    return result


def execute_request(service, stream):
    """One bounded UTF-8 request on stdin; task arguments never enter a shell string."""
    raw = stream.read(MAX_REQUEST_BYTES + 1)
    if len(raw) > MAX_REQUEST_BYTES:
        raise SessionError("structured session request exceeds its limit")
    try:
        request = json.loads(raw.decode("utf-8"), object_pairs_hook=unique_fields)
    except (ValueError, RecursionError) as error:
        raise SessionError("structured session request is invalid") from error
    common = {"version", "operation", "runtime", "panel"}
    if (
        not isinstance(request, dict)
        or type(request.get("version")) is not int or request["version"] != 1
        or not isinstance(request.get("runtime"), str)
        or not isinstance(request.get("panel"), str)
    ):
        raise SessionError("structured session request is invalid")
    if request.get("operation") == "status" and set(request) == common:
        return service.status(request["runtime"], request["panel"])
    prepared = request.get("operation") == "start-prepared"
    expected = common | {"directory", "argv"} | ({"repository"} if prepared else set())
    if request.get("operation") not in ("start", "verify", "start-prepared") or set(request) != expected:
        raise SessionError("unsupported structured session request")
    if (
        not isinstance(request["directory"], str)
        or not isinstance(request["argv"], list)
        or not all(isinstance(argument, str) for argument in request["argv"])
    ):
        raise SessionError("structured task intent is invalid")
    if prepared:
        return service.start_prepared(request["runtime"], request["panel"], request["directory"],
                                      request["argv"], request["repository"])
    operation = service.verify if request["operation"] == "verify" else service.start
    return operation(request["runtime"], request["panel"], request["directory"], request["argv"])


def repository_selection(operation, selection):
    """Only the fixed trusted core helper canonicalizes and inspects this selection."""
    if operation not in ("setup-binding", "setup-checkout"):
        raise SessionError("unsupported repository inspection")
    encoded = json.dumps(selection, ensure_ascii=False).encode()
    if len(encoded) > 34 * 1024:
        raise SessionError("prepared repository selection exceeds its limit")
    result = subprocess.run(["/usr/local/bin/horizon-repository", operation], input=encoded,
                            capture_output=True, timeout=120, check=False, close_fds=True,
                            cwd="/", env={"PATH": "/usr/bin:/bin", "LC_ALL": "C"})
    if result.returncode or result.stderr or len(result.stdout) > 16 * 1024 or not result.stdout.endswith(b"\n"):
        raise SessionError("prepared repository inspection did not complete")
    value = json.loads(result.stdout, object_pairs_hook=unique_fields)
    if (not isinstance(value, dict) or set(value) != {"version", "binding_sha256", "runtime", "root", "reason"}
            or type(value["version"]) is not int or value["version"] != 1 or value["reason"] is not None
            or not isinstance(value["runtime"], str) or not isinstance(value["binding_sha256"], str)
            or not re.fullmatch(r"[0-9a-f]{64}", value["binding_sha256"])
            or operation == "setup-binding" and value["root"] is not None):
        raise SessionError("prepared repository inspection is invalid")
    return value


def private_directory(path):
    path.mkdir(mode=0o700, exist_ok=True)
    check_directory(path)


def check_directory(path):
    metadata = path.lstat()
    if (not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != os.geteuid()
            or stat.S_IMODE(metadata.st_mode) & 0o077):
        raise SessionError("session state directory is not private")


def sync_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


class PanelSessions:
    def __init__(self, repository, state, sockets, config, wrapper, *, require_existing_state=False):
        self.repository = Path(repository)
        self.state = Path(state)
        self.sockets = Path(sockets)
        self.config = Path(config)
        self.wrapper = str(wrapper)
        self.require_existing_state = require_existing_state

    def check_state_parent(self):
        if self.require_existing_state:
            check_directory(self.state.parent)

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
        self.check_state_parent()
        check_directory(self.state)
        check_directory(path.parent)
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as stream:
            metadata = os.fstat(stream.fileno())
            if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.geteuid()
                    or stat.S_IMODE(metadata.st_mode) & 0o077):
                raise SessionError("session marker is not private")
            raw = stream.read(4097)
        if len(raw) > 4096:
            raise SessionError("session marker exceeds its limit")
        marker = json.loads(raw, object_pairs_hook=unique_fields)
        expected = {"version", "runtime", "panel", "nonce", "intent"}
        if isinstance(marker, dict) and marker.get("version") == 2:
            expected.add("repository")
        if (
            not isinstance(marker, dict) or set(marker) != expected
            or type(marker["version"]) is not int or marker["version"] not in (1, 2)
            or marker["runtime"] != runtime or marker["panel"] != panel
            or not isinstance(marker["nonce"], str)
            or not re.fullmatch(r"[0-9a-f]{32}", marker["nonce"])
            or not isinstance(marker["intent"], str)
            or not re.fullmatch(r"[0-9a-f]{64}", marker["intent"])
            or marker["version"] == 2 and (not isinstance(marker["repository"], str)
                                            or not re.fullmatch(r"[0-9a-f]{64}", marker["repository"]))
        ):
            raise SessionError("session marker identity is invalid")
        return marker

    def publish_marker(self, runtime, panel, marker):
        path = self.marker_path(runtime, panel)
        self.check_state_parent()
        if self.require_existing_state:
            check_directory(self.state)
        else:
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

    @staticmethod
    def intent_digest(directory, arguments):
        if not arguments or len(arguments) > 257 or not arguments[0] or arguments[0].startswith("-"):
            raise SessionError("invalid task command")
        if any("\0" in argument for argument in arguments) or sum(len(item.encode()) for item in arguments) > 65536:
            raise SessionError("task command exceeds its limit")
        requested = Path(directory)
        if "\0" in directory or requested.is_absolute() or ".." in requested.parts:
            raise SessionError("task directory must remain inside the repository")
        payload = json.dumps([str(requested), arguments], ensure_ascii=True).encode()
        return hashlib.sha256(payload).hexdigest()

    def launch_intent(self, directory, arguments):
        intent = self.intent_digest(directory, arguments)
        repository = self.repository.resolve(strict=True)
        working_directory = (repository / directory).resolve(strict=True)
        if not working_directory.is_dir() or not working_directory.is_relative_to(repository):
            raise SessionError("task directory must remain inside the repository")
        return working_directory, intent

    def start(self, runtime, panel, directory, arguments):
        self.identities(runtime, panel)
        working_directory, intent = self.launch_intent(directory, arguments)
        return self._start_at(runtime, panel, working_directory, arguments, intent)

    def start_prepared(self, runtime, panel, directory, arguments, selection):
        self.identities(runtime, panel)
        intent = self.intent_digest(directory, arguments)
        binding = repository_selection("setup-binding", selection)
        if binding["runtime"] != runtime:
            raise SessionError("prepared repository runtime does not match the task")
        try:
            marker = self.read_marker(runtime, panel)
        except FileNotFoundError:
            marker = None
        if marker is not None:
            self.match_intent(marker, intent, binding["binding_sha256"])
            return self.status(runtime, panel)
        inspected = repository_selection("setup-checkout", selection)
        root = inspected["root"]
        if (inspected["runtime"] != runtime or inspected["binding_sha256"] != binding["binding_sha256"]
                or not isinstance(root, dict) or set(root) != {"path", "device", "inode"}
                or not isinstance(root["path"], str) or not Path(root["path"]).is_absolute()
                or any(type(root[key]) is not int or root[key] < 0 for key in ("device", "inode"))):
            raise SessionError("prepared repository root is invalid")
        repository = Path(root["path"])
        descriptor = os.open(repository, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            held = os.fstat(descriptor)
            if (held.st_uid != os.geteuid() or stat.S_IMODE(held.st_mode) != 0o700 or held.st_nlink == 0
                    or (held.st_dev, held.st_ino) != (root["device"], root["inode"])):
                raise SessionError("prepared repository root changed")
            working_directory = (repository / directory).resolve(strict=True)
            if not working_directory.is_dir() or not working_directory.is_relative_to(repository):
                raise SessionError("task directory must remain inside the repository")
            named = repository.lstat()
            if (named.st_dev, named.st_ino) != (held.st_dev, held.st_ino):
                raise SessionError("prepared repository root changed")
            return self._start_at(runtime, panel, working_directory, arguments, intent, binding["binding_sha256"])
        finally:
            os.close(descriptor)

    @staticmethod
    def match_intent(marker, intent, repository):
        if marker["intent"] != intent or marker.get("repository") != repository:
            raise SessionError("panel launch intent differs from its retained task")

    def _start_at(self, runtime, panel, working_directory, arguments, intent, repository=None):
        _, name = self.identities(runtime, panel)
        try:
            marker = self.read_marker(runtime, panel)
        except FileNotFoundError:
            marker = None
        if marker is None:
            if self.tmux(runtime, "has-session", "-t", f"={name}", check=False).returncode == 0:
                raise SessionError("an unowned session already uses this panel identity")
            marker = {"version": 2 if repository else 1, "runtime": runtime, "panel": panel,
                      "nonce": uuid.uuid4().hex, "intent": intent}
            if repository is not None:
                marker["repository"] = repository
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
        self.match_intent(marker, intent, repository)
        return self.status(runtime, panel)

    def verify(self, runtime, panel, directory, arguments):
        marker = self.read_marker(runtime, panel)
        intent = self.intent_digest(directory, arguments)
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
    operations = parser.add_subparsers(dest="operation", required=True)
    operations.add_parser("request", help="read one versioned start/status/verify request from stdin")
    for operation in ("start", "status", "attach"):
        command = operations.add_parser(operation)
        command.add_argument("runtime")
        command.add_argument("panel")
        command.add_argument("arguments", nargs=argparse.REMAINDER)
    arguments = parser.parse_args()
    service = PanelSessions("/workspace/horizon", "/workspace/.horizon-worker/panels", "/run/horizon/panels",
                            "/etc/horizon/tmux.conf", "/usr/local/bin/horizon-agent-session",
                            require_existing_state=True)
    try:
        if arguments.operation == "request":
            result = execute_request(service, sys.stdin.buffer)
        elif arguments.operation == "start":
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
