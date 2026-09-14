"""Bounded probe execution for the read-only host preflight.

Process supervision only: spawn, timeout, output caps, and descendant
reaping. Probe command selection and report rendering live in preflight.py.
"""
import json
import os
import select
import signal
import subprocess
import time

MAX_PROBE_OUTPUT_BYTES = 65536
MAX_WATCHDOG_PAYLOAD = MAX_PROBE_OUTPUT_BYTES * 12 + 4096


def decode_probe_output(data):
    """Decode probe bytes with replacement so non-UTF-8 cannot crash preflight."""
    if data is None:
        return ""
    if isinstance(data, bytes):
        return data.decode("utf-8", errors="replace")
    return str(data)


def _kill_descendants(pid):
    """SIGKILL children recorded in Linux procfs, depth-first."""
    children = []
    try:
        with open("/proc/%d/task/%d/children" % (pid, pid), "r", encoding="ascii") as handle:
            children = [int(part) for part in handle.read().split() if part.isdigit()]
    except OSError:
        children = []
    for child in children:
        _kill_descendants(child)
        try:
            os.kill(child, signal.SIGKILL)
        except OSError:
            pass


def _terminate_probe(proc):
    """Kill the probe and its descendants, then reap."""
    _kill_descendants(proc.pid)
    try:
        proc.kill()
    except OSError:
        pass
    try:
        proc.wait(timeout=1)
    except (subprocess.TimeoutExpired, OSError):
        try:
            proc.kill()
        except OSError:
            pass


def bounded_communicate(proc, timeout, max_bytes):
    """Read stdout/stderr up to max_bytes. Kill the probe if it exceeds that.

    Returns (stdout, stderr, overflow_or_None). Raises TimeoutExpired.
    Overflow and timeout kill the whole process group so descendants cannot
    outlive the bounded probe.
    """
    deadline = time.monotonic() + timeout
    buckets = {proc.stdout: bytearray(), proc.stderr: bytearray()}
    open_fds = [proc.stdout, proc.stderr]
    for fd in open_fds:
        os.set_blocking(fd.fileno(), False)
    overflow = False
    try:
        while open_fds:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                _terminate_probe(proc)
                raise subprocess.TimeoutExpired(proc.args, timeout)
            ready, _, _ = select.select(open_fds, [], [], remaining)
            for fd in ready:
                chunk = fd.read(4096)
                if chunk is None:
                    continue
                if chunk == b"":
                    open_fds.remove(fd)
                    continue
                buckets[fd].extend(chunk)
                if len(buckets[proc.stdout]) + len(buckets[proc.stderr]) > max_bytes:
                    overflow = True
                    _terminate_probe(proc)
                    open_fds = []
                    break
        remaining = deadline - time.monotonic()
        if proc.poll() is None:
            if remaining <= 0:
                _terminate_probe(proc)
                raise subprocess.TimeoutExpired(proc.args, timeout)
            try:
                proc.wait(timeout=remaining)
            except subprocess.TimeoutExpired:
                _terminate_probe(proc)
                raise subprocess.TimeoutExpired(proc.args, timeout)
        stdout = bytes(buckets[proc.stdout])
        stderr = bytes(buckets[proc.stderr])
        if overflow:
            return stdout, stderr, "probe output exceeded %d bytes" % max_bytes
        return stdout, stderr, None
    finally:
        for fd in (proc.stdout, proc.stderr):
            if fd is not None:
                try:
                    fd.close()
                except OSError:
                    pass


def _execute_probe(argv, timeout):
    # Stay in the watchdog session so the parent can killpg the watchdog
    # and reap a hung PATH lookup *and* any probe that already started.
    proc = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            shell=False)
    try:
        stdout, stderr, overflow = bounded_communicate(proc, timeout, MAX_PROBE_OUTPUT_BYTES)
    except BaseException:
        _terminate_probe(proc)
        _kill_session_except_self()
        raise
    if overflow:
        _kill_session_except_self()
        return {"exit_code": 1, "stdout": "", "stderr": overflow, "output_exceeded": True}
    return {"exit_code": proc.returncode,
            "stdout": decode_probe_output(stdout),
            "stderr": decode_probe_output(stderr)}


def _reap_child(pid, timeout=1.0):
    """Wait up to `timeout` seconds for an owned child. Never block forever."""
    deadline = time.monotonic() + timeout
    while True:
        try:
            waited, _status = os.waitpid(pid, os.WNOHANG)
        except OSError:
            return
        if waited == pid:
            return
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return
        time.sleep(min(0.02, remaining))


def _kill_process_group(pid):
    try:
        os.killpg(pid, signal.SIGKILL)
    except OSError:
        try:
            os.kill(pid, signal.SIGKILL)
        except OSError:
            pass
    _reap_child(pid)


def _kill_session_except_self():
    """SIGKILL other members of this process group (orphans holding pipes)."""
    me = os.getpid()
    try:
        pgid = os.getpgrp()
    except OSError:
        return
    try:
        names = os.listdir("/proc")
    except OSError:
        return
    for name in names:
        if not name.isdigit():
            continue
        pid = int(name)
        if pid == me:
            continue
        try:
            if os.getpgid(pid) == pgid:
                os.kill(pid, signal.SIGKILL)
        except OSError:
            pass


def _watchdog_execute_probe(argv, timeout):
    """Run the probe in a child so PATH lookup / `Popen` cannot hang the checker.

    The deadline starts before executable resolution. On expiry the child
    session is killed (probe process group included).
    """
    read_fd, write_fd = os.pipe()
    pid = os.fork()
    if pid == 0:
        os.close(read_fd)
        try:
            os.setsid()
        except OSError as exc:
            payload = {"kind": "os",
                       "detail": "could not create process session: %s" % str(exc)[:200]}
            try:
                os.write(write_fd, json.dumps(payload).encode("utf-8"))
            except OSError:
                pass
            try:
                os.close(write_fd)
            except OSError:
                pass
            os._exit(0)
        try:
            try:
                result = _execute_probe(argv, timeout)
                if result.get("output_exceeded"):
                    _kill_session_except_self()
                payload = {"kind": "ok", "result": result}
            except FileNotFoundError:
                payload = {"kind": "fnf"}
            except subprocess.TimeoutExpired:
                _kill_session_except_self()
                payload = {"kind": "timeout"}
            except OSError as exc:
                _kill_session_except_self()
                payload = {"kind": "os", "detail": str(exc)[:400]}
            os.write(write_fd, json.dumps(payload).encode("utf-8"))
        finally:
            try:
                os.close(write_fd)
            except OSError:
                pass
            os._exit(0)
    os.close(write_fd)
    deadline = time.monotonic() + timeout
    chunks = bytearray()
    cleaned = False
    try:
        os.set_blocking(read_fd, False)
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                _kill_process_group(pid)
                cleaned = True
                raise subprocess.TimeoutExpired(argv, timeout)
            ready, _, _ = select.select([read_fd], [], [], remaining)
            if not ready:
                _kill_process_group(pid)
                cleaned = True
                raise subprocess.TimeoutExpired(argv, timeout)
            try:
                chunk = os.read(read_fd, 65536)
            except BlockingIOError:
                continue
            if chunk == b"":
                break
            chunks.extend(chunk)
            if len(chunks) > MAX_WATCHDOG_PAYLOAD:
                _kill_process_group(pid)
                cleaned = True
                raise subprocess.TimeoutExpired(argv, timeout)
        if not chunks:
            _kill_process_group(pid)
            cleaned = True
            raise subprocess.TimeoutExpired(argv, timeout)
        try:
            payload = json.loads(chunks.decode("utf-8"))
        except (ValueError, UnicodeDecodeError, RecursionError) as exc:
            _kill_process_group(pid)
            cleaned = True
            raise OSError("probe watchdog returned malformed result") from exc
        kind = payload.get("kind") if isinstance(payload, dict) else None
        if kind == "ok" and isinstance(payload.get("result"), dict):
            result = payload["result"]
            if result.get("output_exceeded"):
                _kill_process_group(pid)
            else:
                _reap_child(pid)
            cleaned = True
            return result
        if kind == "fnf":
            _reap_child(pid)
            cleaned = True
            raise FileNotFoundError(argv[0] if argv else "probe")
        _kill_process_group(pid)
        cleaned = True
        if kind == "timeout":
            raise subprocess.TimeoutExpired(argv, timeout)
        raise OSError(payload.get("detail", "probe could not run") if isinstance(payload, dict)
                      else "probe could not run")
    except BaseException:
        if not cleaned:
            _kill_process_group(pid)
        raise
    finally:
        try:
            os.close(read_fd)
        except OSError:
            pass


def default_executor(argv, timeout):
    if hasattr(os, "fork"):
        return _watchdog_execute_probe(argv, timeout)
    return _execute_probe(argv, timeout)
