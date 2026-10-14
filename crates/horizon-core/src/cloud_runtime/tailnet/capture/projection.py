import fcntl
import json
import os
import selectors
import signal
import stat
import subprocess
import sys
import time

LIMIT = 65536
DEADLINE = time.monotonic() + 10
child = None


def refuse(*_args):
    raise ValueError()


def unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            refuse()
        result[key] = value
    return result


def directory(path, private=False, trusted=False):
    current = os.open('/', os.O_RDONLY | os.O_DIRECTORY)
    try:
        for component in path.strip('/').split('/'):
            following = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=current)
            os.close(current)
            current = following
            info = os.fstat(current)
            if trusted and (info.st_uid != 0 or stat.S_IMODE(info.st_mode) & 0o022):
                refuse()
        info = os.fstat(current)
        if private and (info.st_uid != 0 or stat.S_IMODE(info.st_mode) != 0o700):
            refuse()
        return current
    except BaseException:
        os.close(current)
        raise


def file(parent, name, mode=None):
    opened = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=parent)
    try:
        info = os.fstat(opened)
        actual = stat.S_IMODE(info.st_mode)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or actual & 0o022 or actual & 0o7000:
            refuse()
        if mode is not None and actual != mode:
            refuse()
        return opened
    except BaseException:
        os.close(opened)
        raise


def main():
    global child
    if os.geteuid() != 0 or len(sys.argv) != 2:
        refuse()
    expected = sys.argv[1]
    if not expected or len(expected) > 64 or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-' for c in expected):
        refuse()
    state = directory('/workspace/.horizon-tailnet', private=True, trusted=True)
    runtime = directory('/run/horizon-tailnet', private=True, trusted=True)
    cli_parent = directory('/usr/local/bin', trusted=True)
    executable = file(cli_parent, 'tailscale')
    if not os.fstat(executable).st_mode & stat.S_IXUSR:
        refuse()
    lock = file(runtime, 'control.lock')
    while True:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            break
        except BlockingIOError:
            if time.monotonic() >= DEADLINE:
                refuse()
            time.sleep(0.02)
    selection = file(state, 'selection', 0o600)
    if os.read(selection, 65) != expected.encode('ascii'):
        refuse()
    socket = os.stat('tailscaled.sock', dir_fd=runtime, follow_symlinks=False)
    if not stat.S_ISSOCK(socket.st_mode) or socket.st_uid != 0 or stat.S_IMODE(socket.st_mode) & 0o600 != 0o600:
        refuse()
    args = ['/usr/local/bin/tailscale', '--socket=/run/horizon-tailnet/tailscaled.sock', 'status', '--json', '--peers=false']
    child = subprocess.Popen(args, executable='/proc/self/fd/' + str(executable), pass_fds=(executable,),
                             stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                             start_new_session=True, env={'PATH': '/usr/local/bin:/usr/bin:/bin'})
    data = bytearray()
    with selectors.DefaultSelector() as ready:
        ready.register(child.stdout, selectors.EVENT_READ)
        while True:
            remaining = DEADLINE - time.monotonic()
            if remaining <= 0 or not ready.select(remaining):
                refuse()
            part = os.read(child.stdout.fileno(), min(4096, LIMIT + 1 - len(data)))
            if not part:
                break
            data.extend(part)
            if len(data) > LIMIT:
                refuse()
    if child.wait(timeout=max(0.01, DEADLINE - time.monotonic())) != 0:
        refuse()
    report = json.loads(data, object_pairs_hook=unique)
    own = report.get('Self') if isinstance(report, dict) else None
    if report.get('BackendState') != 'Running' or not isinstance(own, dict) or own.get('Tags') != ['tag:horizon-worker']:
        refuse()
    identity = own.get('ID')
    if not isinstance(identity, str) or not identity or len(identity) > 256 or any(not ('!' <= c <= '~') for c in identity):
        refuse()
    print(json.dumps({'version': 1, 'tailnet_id': expected, 'device_id': identity}, separators=(',', ':')))


signal.signal(signal.SIGHUP, refuse)
signal.signal(signal.SIGTERM, refuse)
try:
    main()
except BaseException:
    sys.exit(1)
finally:
    if child is not None:
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait(timeout=1)
