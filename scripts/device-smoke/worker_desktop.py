"""Run a frozen debug Horizon on a cloud worker's desktop with private state until stopped.

`serve.py --worker-desktop` starts this. The worker desktop (`:99`) is shared by every session
on the worker; a person watches it read-only through Add desktop viewer on the cloud card.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

from serve import application_document
import x11_windows

CHECKOUT = Path(__file__).resolve().parents[2]
WORKER_DISPLAY = ':99'
# The worker gives each agent session these; a debug Horizon that kept them would act as a
# worker client and reach the worker's browser service, devices and GPU lock.
WORKER_IDENTITY_KEYS = (
    'HORIZON',
    'HORIZON_BROWSER_ACTOR',
    'HORIZON_BROWSER_HOST_INSTANCE',
    'HORIZON_BROWSER_ROOT',
    'HORIZON_DEVICE_ACTOR',
    'HORIZON_CLOUD_ACTOR',
    'HORIZON_GPU_LOCK',
)
CREDENTIAL_KEYS = ('ANTHROPIC_API_KEY', 'ANTHROPIC_WORKSPACE_ID', 'ANTHROPIC_CUSTOM_HEADERS')
STRIPPED_KEYS = WORKER_IDENTITY_KEYS + CREDENTIAL_KEYS + ('WAYLAND_DISPLAY',)
REPLACED_KEYS = ('DISPLAY', 'HORIZON_HOME', 'HORIZON_AGENT_INTEGRATIONS', 'HISTFILE', 'CODEX_HOME', 'GROK_HOME',
                 'XDG_DATA_HOME', 'XDG_CONFIG_HOME', 'XDG_CACHE_HOME')
STOP_SECONDS = 15


def child_environment(environ, state, display):
    """The debug Horizon's environment and the names removed from `environ` (never values).

    HOME stays, so terminals in the debug app keep the worker home. Horizon's state goes to
    HORIZON_HOME, and HORIZON_AGENT_INTEGRATIONS=off stops it from installing agent skills,
    plugins, MCP registrations and a desktop entry in that home, which the worker's own agents
    use. The Codex and Grok homes, the XDG directories and the shell history of its terminals
    go to the state directory too, so the agents and tools that the debug app starts keep
    their files there.
    """
    removed = sorted(key for key in STRIPPED_KEYS if key in environ)
    env = {key: value for key, value in environ.items() if key not in STRIPPED_KEYS + REPLACED_KEYS}
    env.update(DISPLAY=display,
               HORIZON_HOME=str(state / 'horizon-home'),
               HORIZON_AGENT_INTEGRATIONS='off',
               HISTFILE=str(state / 'shell-history'),
               CODEX_HOME=str(state / 'agents' / 'codex'),
               GROK_HOME=str(state / 'agents' / 'grok'),
               XDG_DATA_HOME=str(state / 'xdg' / 'data'),
               XDG_CONFIG_HOME=str(state / 'xdg' / 'config'),
               XDG_CACHE_HOME=str(state / 'xdg' / 'cache'))
    return env, removed


def lower_priority():
    """Run in the build child only. The worker container lacks CAP_SYS_NICE, so a priority
    lowered here can never be raised again; the agent's own shell must keep its priority."""
    os.nice(19)
    try:
        os.sched_setscheduler(0, os.SCHED_IDLE, os.sched_param(0))
    except (AttributeError, OSError):
        pass


def build_command():
    command = ['cargo', 'build', '--locked', '-p', 'horizon-ui', '--bin', 'horizon']
    # The idle I/O class needs no capability.
    return (['ionice', '-c', '3'] if shutil.which('ionice') else []) + command


def observed_priority(pid):
    """The priority the build child actually runs at, as the manifest reports it."""
    try:
        idle = os.sched_getscheduler(pid) == os.SCHED_IDLE
        return {'nice': os.getpriority(os.PRIO_PROCESS, pid), 'scheduler': 'idle' if idle else 'other'}
    except (AttributeError, OSError):
        return {'nice': None, 'scheduler': None}


def build(target_dir, log_path):
    env = dict(os.environ, CARGO_TARGET_DIR=str(target_dir))
    started = time.monotonic()
    with log_path.open('wb') as log:
        process = subprocess.Popen(build_command(), cwd=CHECKOUT, env=env, stdin=subprocess.DEVNULL,
                                   stdout=log, stderr=subprocess.STDOUT, preexec_fn=lower_priority,
                                   start_new_session=True)
        priority = observed_priority(process.pid)
        try:
            code = process.wait()
        except BaseException:
            end_group(process)
            raise
    if code != 0:
        raise SystemExit(f'The build failed with exit code {code}; read {log_path}')
    return {'target_dir': str(target_dir), 'log': str(log_path),
            'seconds': round(time.monotonic() - started), **priority,
            'io_class': 'idle' if shutil.which('ionice') else 'default'}


def sha256(path):
    digest = hashlib.sha256()
    with open(path, 'rb') as source:
        for block in iter(lambda: source.read(1 << 20), b''):
            digest.update(block)
    return digest.hexdigest()


def freeze(binary, state):
    """Keep the binary as `<state>/bin/horizon` and record its hash; return how it was kept.

    A hard link writes nothing: on a worker, copying the 1 GB debug binary to the workspace volume
    stalls the browser service's own writes there. The linker writes the next build to a new file,
    and Linux refuses writes to a running executable, so the linked file stays this build.
    """
    directory = state / 'bin'
    directory.mkdir(mode=0o700)
    frozen = directory / 'horizon'
    try:
        os.link(binary, frozen)
        method = 'link'
    except OSError:
        shutil.copyfile(binary, frozen)
        frozen.chmod(0o700)
        method = 'copy'
    digest = sha256(frozen)
    (directory / 'SHA256SUMS').write_text(f'{digest}  horizon\n')
    return frozen, digest, method


def group_alive(group):
    """Whether a process of `group` is left that is not a zombie."""
    for entry in Path('/proc').iterdir():
        if not entry.name.isdigit():
            continue
        try:
            fields = (entry / 'stat').read_text().rsplit(')', 1)[1].split()
        except (OSError, IndexError):
            continue
        if fields[0] != 'Z' and int(fields[2]) == group:
            return True
    return False


def end_group(process, timeout=5):
    """End the process and whatever is left in its group, also after the leader exited."""
    deadline = time.monotonic() + timeout
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    while group_alive(process.pid) and time.monotonic() < deadline:
        process.poll()
        time.sleep(0.1)
    if group_alive(process.pid):
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    process.wait()


def stop(process, display):
    """Close the window as the window manager would, then end what is left of the group."""
    if process.poll() is None:
        try:
            asked = x11_windows.request_close(display, process.pid)
        except OSError:
            asked = 0
        try:
            if asked:
                process.wait(STOP_SECONDS)
        except subprocess.TimeoutExpired:
            pass
    end_group(process)


def wait_for_window(process, display, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline and process.poll() is None:
        try:
            if x11_windows.windows_of(display, process.pid):
                return True
        except OSError as error:
            raise RuntimeError(f'Cannot read the windows of {display}: {error}') from error
        time.sleep(0.5)
    return False


def default_paths(args, environ):
    session = environ.get('HORIZON_SESSION_DIR')
    if args.state is None:
        if not session:
            raise SystemExit('HORIZON_SESSION_DIR is not set: run this in a cloud worker session or pass --state')
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')
        args.state = Path(session) / 'worker-desktop' / f'{stamp}-{os.getpid()}'
    if args.horizon is None and args.target_dir is None:
        if not session:
            raise SystemExit('HORIZON_SESSION_DIR is not set: pass --horizon or --target-dir')
        # The shared checkout's target/ belongs to every session on the worker.
        args.target_dir = Path(session) / 'target' / 'device-smoke'
    return args


def parse(argv):
    parser = argparse.ArgumentParser(prog='serve.py --worker-desktop', description=__doc__)
    parser.add_argument('--worker-desktop', action='store_true', help='Run on the cloud worker desktop')
    parser.add_argument('--horizon', type=Path, help='Use this binary instead of building the checkout')
    parser.add_argument('--target-dir', type=Path, help='Cargo target directory for the build')
    parser.add_argument('--state', type=Path, help='New private state directory')
    parser.add_argument('--config', type=Path, help='Horizon config; the default is the disposable fixture')
    parser.add_argument('--display', default=WORKER_DISPLAY, help='X display of the worker desktop')
    parser.add_argument('--window-timeout', type=float, default=60, help='Seconds to wait for the window')
    return default_paths(parser.parse_args(argv), os.environ)


def main(argv=None):
    args = parse(sys.argv[1:] if argv is None else argv)
    args.state.mkdir(mode=0o700, parents=True, exist_ok=False)
    state = args.state.resolve()

    def interrupted(_signal, _frame):
        raise KeyboardInterrupt

    previous = {number: signal.getsignal(number) for number in (signal.SIGTERM, signal.SIGINT)}
    signal.signal(signal.SIGTERM, interrupted)
    process = None
    try:
        built = None
        binary = args.horizon
        if binary is None:
            built = build(args.target_dir.resolve(), state / 'build.log')
            binary = args.target_dir / 'debug' / 'horizon'
        frozen, digest, frozen_by = freeze(binary.resolve(strict=True), state)
        config = args.config.resolve(strict=True) if args.config else state / 'horizon.yaml'
        if args.config is None:
            config.write_text(json.dumps(application_document(str(state))))  # JSON is a YAML subset.
        env, removed = child_environment(os.environ, state, args.display)
        log_path = state / 'horizon.log'
        started_at = datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
        with log_path.open('wb') as log:
            process = subprocess.Popen([str(frozen), '--config', str(config), '--ephemeral'],
                                       cwd=state, env=env, stdin=subprocess.DEVNULL, stdout=log,
                                       stderr=subprocess.STDOUT, start_new_session=True)
        try:
            running = sha256(f'/proc/{process.pid}/exe')
        except OSError as error:
            raise RuntimeError(f'Horizon stopped at launch ({error}); read {log_path}') from error
        if running != digest:
            raise RuntimeError(f'The running executable {running} is not the frozen binary {digest}')
        if not wait_for_window(process, args.display, args.window_timeout):
            if process.poll() is not None:
                raise RuntimeError(f'Horizon exited with code {process.returncode} before its window showed; '
                                   f'read {log_path}')
            raise RuntimeError(f'No window of Horizon {process.pid} showed on {args.display} within '
                               f'{args.window_timeout:g} s; read {log_path}')
        manifest = {
            'display': args.display, 'pid': process.pid, 'launcher_pid': os.getpid(),
            'binary': str(frozen), 'frozen_by': frozen_by, 'sha256': digest, 'state': str(state),
            'horizon_home': env['HORIZON_HOME'], 'log': str(log_path), 'config': str(config),
            'started_at': started_at, 'build': built, 'removed_environment': removed,
            'window': True,
        }
        (state / 'worker-desktop.json').write_text(json.dumps(manifest, indent=2))
        print(json.dumps(manifest), flush=True)
        code = process.wait()
        if code != 0:
            print(f'Horizon exited with code {code}; read {log_path}', file=sys.stderr)
            return 1
        return 0
    except KeyboardInterrupt:
        return 0
    finally:
        # A second signal must not interrupt the stop.
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        try:
            if process is not None:
                stop(process, args.display)
        finally:
            for number, handler in previous.items():
                signal.signal(number, handler)
