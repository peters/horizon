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
CLOUD_SETTINGS_FOLDER = '.horizon/cloud'
SSH_FOLDER = '.ssh'
SSH_CONFIG_NAME = 'config'
SSH_SYSTEM_INCLUDE = '/etc/ssh/ssh_config.d'
HOST_SESSION_KEYS = (
    'DBUS_SESSION_BUS_ADDRESS',
    'DBUS_STARTER_ADDRESS',
    'DBUS_STARTER_BUS_TYPE',
)
STRIPPED_KEYS = (
    'HORIZON',
    'HORIZON_HOME',
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


def process_state(pid):
    """The state letter and process group of `pid` from /proc, as bytes, or None
    once it is gone. Read as bytes: a process name need not be UTF-8, and it ends
    at the last `)`."""
    try:
        fields = Path(f'/proc/{pid}/stat').read_bytes().rsplit(b')', 1)[1].split()
    except (OSError, IndexError):
        return None
    return (fields[0], fields[2]) if len(fields) > 2 else None


def _group_runs(group):
    """Whether a process of `group` other than a zombie still runs."""
    wanted = str(group).encode()
    for entry in Path('/proc').iterdir():
        if not entry.name.isdigit():
            continue
        found = process_state(entry.name)
        if found is not None and found[1] == wanted and found[0] != b'Z':
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


def ssh_sources(config=None, identity=None):
    """The SSH configuration and identity that the fixture seeds, or `(None, None)`.

    The two files come together or not at all: a configuration without its key
    cannot answer, and a key without a configuration names no host. Each must be
    a regular file that exists, because a directory or a missing path is a
    mistake of the procedure and would seed a home that `ssh` cannot use.
    """
    if (config is None) != (identity is None):
        raise ValueError('the SSH configuration and the SSH identity are seeded together, or not at all')
    if config is None:
        return None, None
    config, identity = Path(config), Path(identity)
    for flag, path in (('--ssh-config', config), ('--ssh-identity', identity)):
        if not path.is_file():
            raise ValueError(f'{flag} must name a regular file that exists: {path}')
    if identity.name == SSH_CONFIG_NAME:
        raise ValueError(f'--ssh-identity must not be named {SSH_CONFIG_NAME}, which is the configuration of the fixture')
    return config, identity


def seed_ssh(private_home, config=None, identity=None):
    """Copy the SSH files of a procedure into the private home of the fixture.

    The app of the fixture runs with `HOME` bound to `private_home`, which holds
    no configuration, so `ssh` finds no alias and no key there. The copy stays
    private: the folder is 0700 and each file 0600, as `ssh` requires of a
    configuration that a person owns. The identity keeps its own file name, so a
    configuration that names `IdentityFile ~/.ssh/<name>` reads it.

    Returns the names of the files that the fixture seeded, in `~/.ssh`.
    """
    config, identity = ssh_sources(config, identity)
    if config is None:
        return []
    folder = Path(private_home) / SSH_FOLDER
    folder.mkdir(mode=0o700, exist_ok=True)
    folder.chmod(0o700)
    seeded = [(config, folder / SSH_CONFIG_NAME), (identity, folder / identity.name)]
    for source, target in seeded:
        target.write_bytes(source.read_bytes())
        target.chmod(0o600)
    return [target.name for _, target in seeded]


def seed_cloud_settings(private_home, source=None):
    """Copy a cloud settings folder into the private home of the fixture.

    The app of the fixture runs with `HOME` bound to `private_home`, so it reads
    its cloud settings from `<private_home>/.horizon/cloud`. A procedure that
    rents, stops or deletes needs the settings of the account there: the
    credentials, the identity and the image pins. The copy stays private
    (folders 0700, files 0600) and never leaves the fixture state.

    Returns the number of files that the fixture seeded.
    """
    if source is None:
        return 0
    source = Path(source).resolve()
    if not source.is_dir():
        raise ValueError(f'--cloud-settings must be a folder: {source}')
    target_root = Path(private_home) / CLOUD_SETTINGS_FOLDER
    seeded = 0
    for path in sorted(source.rglob('*')):
        relative = path.relative_to(source)
        target = target_root / relative
        if path.is_symlink() or not (path.is_file() or path.is_dir()):
            raise ValueError(f'--cloud-settings holds {relative}, which is not a regular file or folder')
        if path.is_dir():
            target.mkdir(mode=0o700, parents=True, exist_ok=True)
            target.chmod(0o700)
            continue
        target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        target.write_bytes(path.read_bytes())
        target.chmod(0o600)
        seeded += 1
    target_root.mkdir(mode=0o700, parents=True, exist_ok=True)
    target_root.chmod(0o700)
    return seeded


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
    # A user namespace shows every file that root owns as `nobody`, and OpenSSH
    # refuses a system configuration file that root does not own: the machine
    # include of the systemd SSH proxy failed every `ssh` of the fixture with
    # "Bad owner or permissions", before the connection started. An empty
    # folder takes its place, and the seeded `~/.ssh/config` of a procedure is
    # the configuration that the fixture reads. A machine without that folder
    # has nothing to hide, and bwrap cannot create one under a read-only root.
    if Path(SSH_SYSTEM_INCLUDE).is_dir():
        namespace += ['--tmpfs', SSH_SYSTEM_INCLUDE]
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
