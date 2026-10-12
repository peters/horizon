"""Private session bus and writable namespace for the isolated desktop fixture."""
import contextlib
import os
from pathlib import Path
import signal
import stat
import subprocess
import time


BUS_SOCKET_NAME = 'bus'
APPARMOR_ACCESS = Path('/sys/kernel/security/apparmor/.access')
X11_SOCKET_DIR = Path('/tmp/.X11-unix')
HOST_SESSION_KEYS = (
    'DBUS_SESSION_BUS_ADDRESS',
    'DBUS_STARTER_ADDRESS',
    'DBUS_STARTER_BUS_TYPE',
)
STRIPPED_KEYS = (
    'HORIZON',
    'WAYLAND_DISPLAY',
    'DISPLAY',
    'CODEX_HOME',
    'GROK_HOME',
    'PYTHONPATH',
    'XAUTHORITY',
) + HOST_SESSION_KEYS


def runtime_dest(uid=None):
    return f'/run/user/{os.getuid() if uid is None else uid}'


def bus_address(uid=None):
    return f'unix:path={runtime_dest(uid)}/{BUS_SOCKET_NAME}'


def diagnose_apparmor(bound):
    info = {
        'query_path': str(APPARMOR_ACCESS),
        'present': APPARMOR_ACCESS.is_file(),
        'sandbox_query_bind': bound,
    }
    if not info['present']:
        info['status'] = 'absent'
        info['note'] = 'Host has no AppArmor query interface; no bind applied.'
    elif bound:
        info['status'] = 'query_bind'
        info['note'] = (
            'Sandbox bind of the host AppArmor query file so policy checks '
            'reach the kernel LSM. Policy load and remove stay read-only. '
            'Host enforcement is unchanged and the developer session bus is not used.'
        )
    else:
        info['status'] = 'unbound'
        info['note'] = (
            'AppArmor query file is present but not writable in the sandbox; '
            'session-bus clients fail with Failed to query AppArmor policy: '
            'Read-only file system.'
        )
    return info


def dbus_daemon_command(address):
    return ['dbus-daemon', '--session', f'--address={address}',
            '--nofork', '--nopidfile', '--nosyslog']


def start(command, **popen_args):
    """Start a sandboxed command in a process group of its own; stop it with `end`.

    bwrap's --die-with-parent does not end the command bwrap runs when bwrap itself
    is terminated, so terminating bwrap alone leaves a bus daemon, and the services
    it activated, running for good. The process group reaches all of them.
    """
    return subprocess.Popen(command, start_new_session=True, **popen_args)


def _group_runs(group):
    """Whether a process of `group` other than a zombie still runs."""
    for stat_file in Path('/proc').glob('[0-9]*/stat'):
        try:
            fields = stat_file.read_text().rsplit(')', 1)[1].split()
        except (OSError, IndexError):
            continue
        if len(fields) > 2 and fields[2] == str(group) and fields[0] != 'Z':
            return True
    return False


def end(process, timeout=5):
    """End everything in the process group `start` gave `process`.

    The group gets SIGTERM, then SIGKILL for whatever still runs after `timeout`.
    `process` is reaped last, so its group id names no other group meanwhile, and a
    group already ended and reaped is left alone.
    """
    group = process.pid
    if process.returncode is not None and not _group_runs(group):
        return
    with contextlib.suppress(ProcessLookupError):
        os.killpg(group, signal.SIGTERM)
    deadline = time.monotonic() + timeout
    while _group_runs(group) and time.monotonic() < deadline:
        time.sleep(0.05)
    with contextlib.suppress(ProcessLookupError):
        os.killpg(group, signal.SIGKILL)
    process.wait()


def wait_for_unix_socket(path, process, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(
                f'dbus-daemon exited before the session socket was ready: {process.returncode}'
            )
        try:
            if stat.S_ISSOCK(path.stat().st_mode):
                return
        except FileNotFoundError:
            pass
        time.sleep(0.05)
    raise RuntimeError(f'session bus socket was not created: {path}')


class Sandbox:
    def __init__(self, state, data, private_home, runtime, private_tmp, runtime_dest_path,
                 address, host_socket, namespace, host_env, sandbox_env, apparmor):
        self.state = state
        self.data = data
        self.private_home = private_home
        self.runtime = runtime
        self.private_tmp = private_tmp
        self.runtime_dest = runtime_dest_path
        self.bus_address = address
        self.host_socket = host_socket
        self.namespace = namespace
        self.host_env = host_env
        self.sandbox_env = sandbox_env
        self.apparmor = apparmor


def prepare(state, environ=None, bind_apparmor_query=True, extra_ro_binds=()):
    """Create private dirs, bwrap prefix, host env, and sandbox env.

    `state` must already exist. Host processes keep XDG_RUNTIME_DIR on the
    fixture path. Namespaced processes see the standard `/run/user/<uid>` path
    backed by that same directory, plus a private writable `/tmp`.
    `extra_ro_binds` are re-mounted after that `/tmp` overlay so documented
    executable and tools paths under `/tmp` stay visible.
    """
    state = Path(state).resolve()
    data = state / 'data'
    private_home = data / 'home'
    runtime = data / 'runtime'
    private_tmp = data / 'tmp'
    data.mkdir()
    private_home.mkdir(mode=0o700)
    runtime.mkdir(mode=0o700)
    private_tmp.mkdir(mode=0o700)
    dest = runtime_dest()
    address = bus_address()
    access_bind = []
    if bind_apparmor_query and APPARMOR_ACCESS.is_file():
        access_bind = ['--bind', str(APPARMOR_ACCESS), str(APPARMOR_ACCESS)]
    namespace = [
        'bwrap', '--die-with-parent',
        '--ro-bind', '/', '/',
        '--dev-bind', '/dev', '/dev',
        '--dir', str(Path(dest).parent),
        '--dir', dest,
        '--bind', str(runtime), dest,
        '--bind', str(private_tmp), '/tmp',
        '--bind', str(state), str(state),
        '--bind', str(private_home), str(Path.home()),
    ]
    seen = {str(state), str(private_home)}
    for path in extra_ro_binds:
        resolved = str(Path(path).resolve())
        if resolved in seen:
            continue
        seen.add(resolved)
        namespace += ['--ro-bind', resolved, resolved]
    namespace += ['--ro-bind-try', str(X11_SOCKET_DIR), str(X11_SOCKET_DIR)]
    namespace += access_bind
    namespace += ['--chdir', str(data), '--']
    base = dict(environ if environ is not None else os.environ)
    for key in STRIPPED_KEYS:
        base.pop(key, None)
    host_env = dict(base)
    host_env.update(
        XDG_DATA_HOME=str(data),
        XDG_CONFIG_HOME=str(data / 'config'),
        XDG_CACHE_HOME=str(data / 'cache'),
        XDG_RUNTIME_DIR=str(runtime),
        HISTFILE='/dev/null',
    )
    sandbox_env = dict(host_env)
    sandbox_env['XDG_RUNTIME_DIR'] = dest
    sandbox_env['DBUS_SESSION_BUS_ADDRESS'] = address
    return Sandbox(
        state=state,
        data=data,
        private_home=private_home,
        runtime=runtime,
        private_tmp=private_tmp,
        runtime_dest_path=dest,
        address=address,
        host_socket=runtime / BUS_SOCKET_NAME,
        namespace=namespace,
        host_env=host_env,
        sandbox_env=sandbox_env,
        apparmor=diagnose_apparmor(bound=bool(access_bind)),
    )
