#!/usr/bin/env python3
"""Temporary Linux speech diagnostics for the unchanged catalog candidate.

The watchdog retains test concurrency and assertions. It captures only owned
processes, regular backtraces and allowlisted counters. It never reads process
environment or command lines. It prints no argument or local values and takes
no memory dump or core file. Raw output stays in the ordinary Actions log;
artifact upload includes only diagnostics and stacks.
"""

import argparse
import json
import os
from pathlib import Path
import re
import selectors
import signal
import subprocess
import sys
import time

ANSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
COMPLETED = re.compile(r"^test ([A-Za-z0-9_:]+) \.\.\. (?:ok|FAILED|ignored)(?:\s|$)")
TEST_START = re.compile(r"^running [0-9]+ tests?$")


class BookkeepingError(RuntimeError):
    pass


class Progress:
    def __init__(self):
        self.last_completed = None
        self.last_progress = None
        self.completed = 0

    def line(self, text, now):
        text = ANSI.sub("", text).strip()
        if TEST_START.fullmatch(text) and self.last_progress is None:
            self.last_progress = now
        completed = COMPLETED.match(text)
        if completed:
            self.last_completed = completed.group(1)
            self.last_progress = now
            self.completed += 1

    def stalled(self, now, idle_seconds):
        return self.last_progress is not None and now - self.last_progress >= idle_seconds


def identity(pid):
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        return {"pid": pid, "state": fields[0], "parent": int(fields[1]),
                "start": int(fields[19])}
    except (OSError, ValueError, IndexError):
        return None


class OwnedProcesses:
    def __init__(self, root_pid):
        self.records = {}
        self._retain(identity(root_pid))

    def _retain(self, item):
        if item is None:
            return
        try:
            fd = os.pidfd_open(item["pid"])
        except ProcessLookupError:
            return
        except OSError as error:
            raise BookkeepingError(f"pidfd_open failed with errno {error.errno}") from error
        current = identity(item["pid"])
        if current is None or current["start"] != item["start"]:
            os.close(fd)
            return
        self.records[item["pid"]] = (item["start"], fd)

    def current(self, pid):
        item = identity(pid)
        recorded = self.records.get(pid)
        if item and recorded and item["start"] == recorded[0] and item["state"] != "Z":
            return item
        return None

    def observe(self):
        self.prune()
        # Retain identities while the parent chain still proves ownership.
        samples = [identity(int(p.name)) for p in Path("/proc").iterdir() if p.name.isdigit()]
        while True:
            added = False
            for item in samples:
                if item and item["pid"] not in self.records and self.current(item["parent"]):
                    self._retain(item)
                    added |= item["pid"] in self.records
            if not added:
                return

    def prune(self):
        for pid in list(self.records):
            if self.current(pid) is None:
                _, fd = self.records.pop(pid)
                os.close(fd)

    def send(self, pid, sig):
        if self.current(pid) is None:
            return
        try:
            signal.pidfd_send_signal(self.records[pid][1], sig)
        except ProcessLookupError:
            pass

    def stop(self):
        # Cleanup must remain possible after observation itself fails.
        try:
            self.observe()
        except BookkeepingError:
            pass
        for pid in list(self.records):
            self.send(pid, signal.SIGTERM)
            self.send(pid, signal.SIGCONT)
        deadline = time.monotonic() + 2
        while any(self.current(pid) for pid in self.records) and time.monotonic() < deadline:
            time.sleep(0.05)
        for pid in list(self.records):
            self.send(pid, signal.SIGKILL)

    def close(self):
        for _, fd in self.records.values():
            os.close(fd)


def counters(path, allowed):
    try:
        return {key: value.strip() for line in Path(path).read_text().splitlines()
                if ":" in line for key, value in [line.split(":", 1)] if key in allowed}
    except OSError:
        return {"unavailable": True}


def tool_version(command):
    try:
        output = subprocess.run(command, check=False, capture_output=True, text=True, timeout=3)
        return output.stdout.splitlines()[:1]
    except (OSError, subprocess.TimeoutExpired):
        return ["unavailable"]


def stack_command(pid):
    # Root ptrace is confined to a stopped, identity-checked owned test process.
    return ["sudo", "-n", "timeout", "--kill-after=2s", "15s", "gdb", "-nx", "-nh", "--batch",
            "-ex", "set auto-load off", "-ex", "set debuginfod enabled off",
            "-ex", "set print frame-arguments none", "-ex", "set print entry-values no",
            "-ex", f"attach {pid}",
            "-ex", "thread apply all bt 64", "-ex", "detach", "-ex", "quit"]


def capture(scope, progress, reason, output, workspace):
    observation_error = None
    try:
        scope.observe()
    except BookkeepingError as error:
        observation_error = str(error)
    capture_deadline = time.monotonic() + 60
    records = []
    for pid in sorted(scope.records):
        item = scope.current(pid)
        if item is None:
            continue
        proc = Path(f"/proc/{pid}")
        record = {**item, "counters": counters(proc / "status", {"Threads", "VmRSS", "VmHWM"})}
        try:
            exe = (proc / "exe").resolve(strict=True)
            record["executable"] = exe.name
            threads = []
            for task in (proc / "task").iterdir():
                try:
                    threads.append({"tid": int(task.name), "wait": (task / "wchan").read_text().strip()})
                except (OSError, ValueError):
                    pass
            record["threads"] = threads
            if exe.is_relative_to(workspace / "target") and re.fullmatch(r"horizon-[0-9a-f]+", exe.name):
                if capture_deadline - time.monotonic() < 20:
                    record["stack_exit"] = "total stack capture budget exhausted"
                    records.append(record)
                    continue
                scope.send(pid, signal.SIGSTOP)
                stop_deadline = time.monotonic() + 1
                while (stopped := scope.current(pid)) and stopped["state"] not in {"T", "t"}:
                    if time.monotonic() >= stop_deadline:
                        break
                    time.sleep(0.01)
                stopped = scope.current(pid)
                if stopped is None or stopped["state"] not in {"T", "t"}:
                    record["stack_exit"] = "owned process did not stop"
                    records.append(record)
                    continue
                if (proc / "exe").resolve(strict=True) != exe:
                    record["stack_exit"] = "owned executable changed before capture"
                    records.append(record)
                    continue
                with (output / f"stacks-{pid}.txt").open("w") as stacks:
                    try:
                        result = subprocess.run(stack_command(pid), stdout=stacks,
                                                stderr=subprocess.STDOUT,
                                                timeout=20,
                                                check=False)
                        record["stack_exit"] = result.returncode
                    except (OSError, subprocess.TimeoutExpired) as error:
                        stacks.write(f"Stack capture unavailable: {type(error).__name__}\n")
                        record["stack_exit"] = "unavailable"
        except OSError:
            record["executable"] = "unavailable"
        records.append(record)
    evidence = {"reason": reason, "completed_tests": progress.completed,
                "last_completed_test": progress.last_completed, "cpu_count": os.cpu_count(),
                "effective_uid": os.geteuid(), "bookkeeping_error": observation_error,
                "memory": counters("/proc/meminfo",
                {"MemTotal", "MemAvailable", "SwapTotal", "SwapFree"}),
                "tools": {"rustc": tool_version(["rustc", "--version"]),
                          "cargo": tool_version(["cargo", "--version"]),
                          "gdb": tool_version(["gdb", "--version"])}, "processes": records}
    (output / "diagnostics.json").write_text(json.dumps(evidence, indent=2) + "\n")


def run(command, output, idle_seconds, total_seconds, workspace, capture_fn=capture, stream=None):
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    progress = Progress()
    started = time.monotonic()
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True)
    try:
        scope = OwnedProcesses(process.pid)
    except BookkeepingError as error:
        # The unreaped Popen child still has its original PID. Without a root
        # pidfd, stop only that direct child; descendant ownership is unknown.
        if process.poll() is None:
            process.kill()
        process.wait(timeout=5)
        process.stdout.close()
        (output / "diagnostics.json").write_text(json.dumps({"reason": str(error),
                "cleanup_scope": "direct owned child; descendant identities unavailable"}) + "\n")
        return 125
    reader = selectors.DefaultSelector()
    reader.register(process.stdout, selectors.EVENT_READ)
    pending = b""
    stream = stream or sys.stdout.buffer
    try:
        with (output / "shard-output.log").open("wb") as raw:
            while True:
                scope.observe()
                for key, _ in reader.select(timeout=0.1):
                    chunk = os.read(key.fileobj.fileno(), 65536)
                    if chunk:
                        raw.write(chunk)
                        raw.flush()
                        stream.write(chunk)
                        stream.flush()
                        pending += chunk
                        lines = pending.split(b"\n")
                        pending = lines.pop()[-65536:]
                        for line in lines:
                            progress.line(line.decode(errors="replace"), time.monotonic())
                    else:
                        reader.unregister(key.fileobj)
                status = process.poll()
                if status is not None:
                    scope.stop()
                    # Drain final buffered output after all known writers stop.
                    while reader.get_map():
                        ready = reader.select(timeout=0.1)
                        if not ready:
                            break
                        for key, _ in ready:
                            chunk = os.read(key.fileobj.fileno(), 65536)
                            if chunk:
                                raw.write(chunk)
                                stream.write(chunk)
                                stream.flush()
                            else:
                                reader.unregister(key.fileobj)
                    return status
                now = time.monotonic()
                reason = "job deadline" if now - started >= total_seconds else f"no completed test for {idle_seconds:g} seconds"
                if now - started >= total_seconds or progress.stalled(now, idle_seconds):
                    capture_fn(scope, progress, reason, output, workspace)
                    stream.write(f"::error::Owned speech shard stopped: {reason}; {progress.completed} tests completed.\n".encode())
                    stream.flush()
                    scope.stop()
                    process.wait(timeout=5)
                    return 124
    except BookkeepingError as error:
        capture_fn(scope, progress, str(error), output, workspace)
        stream.write(f"::error::Owned shard bookkeeping failed: {error}.\n".encode())
        stream.flush()
        return 125
    finally:
        if process.poll() is None:
            scope.stop()
            process.wait(timeout=5)
        reader.close()
        process.stdout.close()
        scope.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--deadline-unix", type=float, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if sys.platform != "linux" or not hasattr(signal, "pidfd_send_signal"):
        parser.error("The diagnostic watchdog requires Linux pidfds")
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("A shard command is required")
    remaining = args.deadline_unix - time.time()
    if remaining <= 0:
        parser.error("The diagnostic job deadline expired before launch")
    return run(command, args.output, 300, remaining, Path(__file__).resolve().parents[1])


if __name__ == "__main__":
    sys.exit(main())
