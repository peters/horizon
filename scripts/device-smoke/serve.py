#!/usr/bin/env python3
"""Own a disposable application/X11 lab and read-only noVNC viewer until Ctrl-C."""
import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import re
import sys
import tempfile
import time

import sandbox


# The directory the host X server keeps its socket and keymap files in.
TMP_DIR = Path('/tmp')


def bound_port(process, path, pattern):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f'Listener exited before readiness: {path}')
        match = re.search(pattern, path.read_text() if path.exists() else '', re.MULTILINE)
        if match:
            return int(match.group(1))
        time.sleep(0.05)
    raise RuntimeError(f'Listener did not report its bound port: {path}')


WEB_PROXY = """
import socket, sys
from pathlib import Path
from websockify import WebSocketProxy
with socket.socket() as listener:
    listener.bind(('127.0.0.1', 0))
    listener.listen(100)
    proxy = WebSocketProxy(listen_fd=listener.fileno(), web=sys.argv[1],
                          target_host='127.0.0.1', target_port=int(sys.argv[2]))
    Path(sys.argv[3]).write_text(str(listener.getsockname()[1]))
    proxy.start_server()
"""


PRESET_RESUME_MODES = ('fresh', 'last')

# The snake_case names of `PanelKind` (`crates/horizon-core/src/panel.rs`). A file that
# names another kind makes Horizon refuse the whole config, which starts the default
# board instead of the board the procedure tests.
PANEL_KINDS = frozenset({
    'shell', 'ssh', 'codex', 'claude', 'open_code', 'gemini', 'kilo_code', 'pi', 'grok',
    'command', 'editor', 'git_changes', 'usage', 'browser', 'device', 'setup_board',
})


def resume_error(place, resume):
    """Why a `resume` value is not one that Horizon reads, or None.

    `PanelResume` renames its unit variants to `fresh` and `last`, and its `session`
    variant keeps a session id, so only the map form `{"session": {"session_id": …}}`
    loads.
    """
    if resume is None or resume in PRESET_RESUME_MODES:
        return None
    if isinstance(resume, dict) and list(resume) == ['session']:
        session = resume['session']
        session_id = session.get('session_id') if isinstance(session, dict) else None
        if isinstance(session_id, str) and session_id:
            return None
        return f'{place}.resume.session needs a session_id'
    modes = ', '.join(PRESET_RESUME_MODES)
    return (
        f'{place}.resume must be {modes}, or '
        f'{{"session": {{"session_id": "<id>"}}}}; received {resume!r}')


def presets_resume_error(index, resume):
    """Why a `resume` value of a preset is not one that Horizon reads, or None."""
    return resume_error(f'presets[{index}]', resume)


def load_presets(path):
    """The New Workspace presets of the fixture file, checked before Horizon reads them.

    Horizon starts with its own defaults when the config does not load, which would show
    an empty board instead of the presets the procedure tests.
    """
    presets = json.loads(path.read_text())
    if not isinstance(presets, list):
        raise ValueError(f'{path}: the presets must be a JSON list of presets')
    for index, preset in enumerate(presets):
        if not isinstance(preset, dict) or not preset.get('name') or not preset.get('kind'):
            raise ValueError(f'{path}: presets[{index}] needs a name and a kind')
        problem = presets_resume_error(index, preset.get('resume'))
        if problem:
            raise ValueError(f'{path}: {problem}')
    return presets


def panel_error(index, panel):
    """Why Horizon would refuse this panel, or None.

    The fixture earns its place only when the board it intends is the board Horizon
    shows, so the checks follow `TerminalConfig` (`crates/horizon-core/src/config.rs`).
    """
    place = f'panels[{index}]'
    if not isinstance(panel, dict):
        return f'{place} must be an object'
    if not isinstance(panel.get('name'), str) or not panel.get('name'):
        return f'{place} needs a name that is a string'
    kind = panel.get('kind', 'shell')
    if kind not in PANEL_KINDS:
        return f'{place} kind {kind!r} is not a panel kind: {sorted(PANEL_KINDS)}'
    for key in ('command', 'cwd'):
        if panel.get(key) is not None and not isinstance(panel[key], str):
            return f'{place} {key} must be a string'
    args = panel.get('args', [])
    if not isinstance(args, list) or not all(isinstance(arg, str) for arg in args):
        return f'{place} args must be a list of strings'
    for key in ('position', 'size'):
        value = panel.get(key)
        if value is None:
            continue
        if (not isinstance(value, list) or len(value) != 2
                or not all(isinstance(number, (int, float)) and not isinstance(number, bool)
                           for number in value)):
            return f'{place} {key} must be two numbers'
    if 'resume' in panel:
        return resume_error(place, panel['resume'])
    return None


def load_panels(path):
    """Extra workspace panels of the fixture file, checked before Horizon reads them.

    A procedure needs a panel the fixture does not place by default, such as an agent
    panel that runs a scripted client. Horizon starts with its own defaults when the
    config does not load, so the fixture refuses the file first.
    """
    panels = json.loads(path.read_text())
    if not isinstance(panels, list):
        raise ValueError(f'{path}: the panels must be a JSON list of panels')
    for index, panel in enumerate(panels):
        problem = panel_error(index, panel)
        if problem:
            raise ValueError(f'{path}: {problem}')
    return panels


def extra_column(placed):
    """The x where a panel of the file starts, to the right of every placed panel.

    The default board is not always the two default terminals, so the column is worked
    out from the panels the fixture actually places.
    """
    right = 40
    for panel in placed:
        position, size = panel.get('position'), panel.get('size')
        if (isinstance(position, list) and isinstance(size, list)
                and len(position) == 2 and len(size) == 2):
            right = max(right, position[0] + size[0])
    return right + 40


def application_document(cwd, device_address=None, firefox_system_access=False, presets=None, extra_panels=None,
                         browser_host=None, browser_host_profile=None):
    """Disposable Horizon config. System access and a browser host stay off unless requested."""
    fixture = {
        'version': 11, 'window': {'width': 1480, 'height': 900},
        'appearance': {'theme': 'dark'},
        'workspaces': [{'name': 'Disposable VNC debug', 'cwd': cwd,
            'terminals': [
                {'name': 'Live render heartbeat', 'command': '/usr/bin/python3',
                 'args': ['-u', '-c', "import time\nprint('HORIZON DEBUG / noVNC LIVE VIEW')\nfor i in range(3600):\n print('Live frame heartbeat:', i, flush=True); time.sleep(1)"],
                 'position': [40, 60], 'size': [550, 420]},
                {'name': 'Device input test', 'command': '/bin/bash',
                 'args': ['--noprofile', '--norc'],
                 'position': [630, 60], 'size': [550, 420]}]}]}
    if device_address:
        fixture['workspaces'][0]['terminals'] = [
            {'name': 'Native device view', 'kind': 'device', 'command': device_address,
             'position': [40, 60], 'size': [1150, 780]}]
    if firefox_system_access:
        fixture['browser'] = {'backend': 'firefox', 'firefox_system_access': True}
        if not device_address:
            fixture['workspaces'][0]['terminals'] = [
                {'name': 'Google sign-in', 'kind': 'browser',
                 'command': 'https://accounts.google.com/ServiceLogin?hl=en',
                 'position': [40, 60], 'size': [700, 640]},
                {'name': 'X login', 'kind': 'browser',
                 'command': 'https://x.com/i/flow/login',
                 'position': [760, 60], 'size': [680, 640]},
                {'name': 'Live render heartbeat', 'command': '/usr/bin/python3',
                 'args': ['-u', '-c', "import time\nprint('HORIZON DEBUG / noVNC LIVE VIEW')\nfor i in range(3600):\n print('Live frame heartbeat:', i, flush=True); time.sleep(1)"],
                 'position': [40, 720], 'size': [550, 140]},
            ]
    if browser_host:
        # A browser host that a browser panel follows, so a procedure proves the
        # panel route without a hand edit of the private settings of the fixture.
        entry = {'ssh_target': browser_host}
        if browser_host_profile:
            entry['profile'] = browser_host_profile
        fixture['browser_host'] = entry
    if presets is not None:
        # The New Workspace menu of the fixture, so a procedure can test an agent that
        # has several presets without changing the real menu of the person.
        fixture['presets'] = presets
    if extra_panels:
        # Panels the fixture opens at start, so a procedure that needs an agent panel or
        # another command keeps one viewer and no manual click.
        placed = fixture['workspaces'][0]['terminals']
        column = extra_column(placed)
        for index, panel in enumerate(extra_panels):
            entry = dict(panel)
            entry.setdefault('position', [column, 60 + 500 * index])
            entry.setdefault('size', [330, 440])
            placed.append(entry)
    return fixture


def application_command(app, config, saved_session):
    """The command line of the candidate in the fixture.

    A saved session keeps its session file in the private home of the fixture, which
    starting a cloud needs: a cloud card belongs to a saved session. The default stays
    ephemeral, which writes no session file.
    """
    session = '--new-session' if saved_session else '--ephemeral'
    return [str(app), '--config', str(config), session]


def require_writable_tmp(directory=None):
    """Stop before the display starts when the host `/tmp` refuses a write.

    The fixture starts its X server on the host, and Xvfb keeps its keymap files in
    `/tmp`. A full or quota-limited `/tmp` otherwise fails inside xkbcomp with
    "Cannot close ... (not enough space?)", which reads like an X or keyboard defect.
    """
    directory = TMP_DIR if directory is None else Path(directory)
    try:
        # An unnamed file: no path stays behind, and a leftover file or symbolic
        # link under /tmp from another run or user cannot make this fail.
        with tempfile.TemporaryFile(dir=directory) as probe:
            # Larger than the keymap the X server compiles there, so a nearly full
            # filesystem fails here as well as an exhausted quota does.
            probe.write(b'\0' * (64 * 1024))
    except OSError as error:
        raise RuntimeError(
            f'{directory} refused a write ({error.strerror or error}). The fixture starts its X server '
            'on the host, and Xvfb keeps its keymap files there, so the display cannot start. '
            'Free space on that filesystem or check its per-user quota, and keep the fixture '
            'state and any frozen binary on a filesystem with space.'
        ) from None


def main():
    if '--worker-desktop' in sys.argv[1:]:
        import worker_desktop
        return worker_desktop.main()
    require_writable_tmp()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--horizon', type=Path, required=True, help='Horizon debug binary for an isolated smoke')
    parser.add_argument('--tools', type=Path, help='Optional unpacked Debian tools root')
    parser.add_argument('--native-view', action='store_true', help='Expose direct VNC without noVNC/websockify')
    parser.add_argument('--device-address', help='Read-only native Device panel endpoint inside the Horizon fixture')
    parser.add_argument(
        '--firefox-system-access', action='store_true',
        help='Set browser.firefox_system_access in the fixture config. Firefox disclosure procedure only.')
    parser.add_argument('--state', type=Path, required=True, help='New private evidence directory')
    parser.add_argument(
        '--reuse-state', action='store_true',
        help='Reuse the private home and data that an earlier run left at --state, and keep them '
             'when this run stops, so a procedure can quit the candidate and start it again in '
             'the same state. Use only a state path that this person owns.')
    parser.add_argument(
        '--saved-session', action='store_true',
        help='Keep the session of the fixture in its private home, which starting a cloud needs')
    parser.add_argument(
        '--presets',
        type=Path,
        help='JSON list of New Workspace presets for the fixture config, such as two presets of one agent')
    parser.add_argument(
        '--browser-host',
        help='SSH target of a browser host for the fixture, such as an SSH config alias, so the '
             'remote browser panel procedure opens its panels there')
    parser.add_argument(
        '--browser-host-profile',
        help='Profile of that browser host whose pages the fixture opens; needs --browser-host')
    parser.add_argument(
        '--panels',
        type=Path,
        help='JSON list of extra workspace panels the fixture opens at start, such as an '
             'agent panel that runs a scripted MCP client')
    parser.add_argument(
        '--cloud-settings',
        metavar='DIR',
        help='Cloud settings folder copied to ~/.horizon/cloud in the private home at 0700, '
             'so a procedure that rents, stops or deletes a cloud machine has the account '
             'settings of the fixture')
    parser.add_argument(
        '--ssh-config',
        type=Path,
        help='SSH configuration of the fixture, copied to ~/.ssh/config in its private home at '
             '0600, so a host that the fixture names over SSH is reachable. Needs --ssh-identity')
    parser.add_argument(
        '--ssh-identity',
        type=Path,
        help='Private key of the fixture, copied to ~/.ssh/<its own file name> in its private '
             'home at 0600. Needs --ssh-config, which names it as IdentityFile')
    args = parser.parse_args()
    try:
        ssh_config, ssh_identity = sandbox.ssh_sources(args.ssh_config, args.ssh_identity)
    except ValueError as problem:
        parser.error(str(problem))
    if args.browser_host_profile and not args.browser_host:
        parser.error('--browser-host-profile needs --browser-host')
    args.state = sandbox.prepare_state(args.state, reuse=args.reuse_state).resolve()
    app = args.horizon.resolve(strict=True)
    extra_ro_binds = [app]
    if args.tools:
        extra_ro_binds.append(args.tools.resolve())
    box = sandbox.prepare(args.state, extra_ro_binds=extra_ro_binds, reuse=args.reuse_state)
    sandbox.seed_ssh(box.private_home, ssh_config, ssh_identity)
    cloud_files = sandbox.seed_cloud_settings(box.private_home, args.cloud_settings)
    data = box.data
    namespace = box.namespace
    host_env = box.host_env
    env = box.sandbox_env
    if args.tools:
        root = args.tools.resolve()
        path = f'{root}/usr/bin:' + host_env.get('PATH', '')
        ld_library = f'{root}/usr/lib/x86_64-linux-gnu'
        pythonpath = f'{root}/usr/lib/python3/dist-packages'
        for child_env in (host_env, env):
            child_env['PATH'] = path
            child_env['LD_LIBRARY_PATH'] = ld_library
            child_env['PYTHONPATH'] = pythonpath
        web = root / 'usr/share/novnc'
    else:
        web = Path('/usr/share/novnc')
    for tool in ('Xvfb', 'openbox', 'x11vnc', 'bwrap', 'dbus-daemon'):
        if not shutil.which(tool, path=host_env['PATH']):
            raise SystemExit(f'Missing prerequisite: {tool}')
    if not args.native_view and not (web / 'vnc.html').is_file():
        raise SystemExit('Missing noVNC assets')
    children, logs = [], []

    def log_path(name):
        """The log file of this run: a reused state keeps the logs of the earlier runs.

        The first run writes `<name>.log`; a later run in the same state writes
        `<name>.2.log`, `<name>.3.log` and so on, so no run truncates the evidence of
        another.
        """
        path = args.state / f'{name}.log'
        run = 2
        while path.exists():
            path = args.state / f'{name}.{run}.log'
            run += 1
        return path

    def spawn(name, command, child_env=env):
        log = log_path(name).open('wb')
        logs.append(log)
        process = sandbox.start(namespace + command, env=child_env, stdout=log, stderr=log)
        children.append(process)
        return process

    def spawn_with_log(name, command, log, child_env=env):
        """Spawn with a log file that the caller keeps, such as one it reads from."""
        logs.append(log)
        process = sandbox.start(namespace + command, env=child_env, stdout=log, stderr=log)
        children.append(process)
        return process

    def stop(_signal, _frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, stop)
    try:
        # Xvfb chooses and locks an unused display atomically.
        read_fd, write_fd = os.pipe()
        log = log_path('xvfb').open('wb')
        logs.append(log)
        xvfb = sandbox.start(['Xvfb', '-displayfd', str(write_fd), '-screen', '0',
                                 '1600x1000x24', '-nolisten', 'tcp', '-extension', 'MIT-SHM'],
                                pass_fds=(write_fd,), stdout=log, stderr=log, env=host_env)
        children.append(xvfb)
        os.close(write_fd)
        import select
        if not select.select([read_fd], [], [], 10)[0]:
            raise RuntimeError('Xvfb startup timed out')
        with os.fdopen(read_fd) as pipe:
            number = pipe.readline().strip()
        if not number.isdigit():
            raise RuntimeError('Xvfb did not report a display')
        for child_env in (host_env, env):
            child_env['DISPLAY'] = ':' + number
            child_env['XDG_SESSION_TYPE'] = 'x11'
            child_env['LIBGL_ALWAYS_SOFTWARE'] = '1'
        spawn('dbus', sandbox.dbus_daemon_command(box.bus_address))
        sandbox.wait_for_unix_socket(box.host_socket, children[-1])
        spawn('openbox', ['openbox'])
        config = data / 'horizon.yaml'
        presets = load_presets(args.presets) if args.presets else None
        extra_panels = load_panels(args.panels) if args.panels else None
        fixture = application_document(
            str(data.resolve()), args.device_address, args.firefox_system_access, presets, extra_panels,
            args.browser_host, args.browser_host_profile)
        config.write_text(json.dumps(fixture))  # JSON is a YAML subset.
        # All client helpers see the same masked home and private XDG paths.
        application_process = spawn('horizon', application_command(app, config, args.saved_session))
        vnc_log = log_path('vnc').open('wb')
        vnc = spawn_with_log('vnc', ['x11vnc', '-norc', '-no6', '-display', env['DISPLAY'],
                                    '-localhost', '-autoport', '40000', '-viewonly', '-forever',
                                    '-shared', '-nopw', '-noxdamage', '-noshm'], vnc_log)
        vnc_port = bound_port(vnc, Path(vnc_log.name), r'^PORT=(\d+)$')
        url = None
        if not args.native_view:
            port_file = args.state / 'web-port'
            viewer = spawn('viewer', ['/usr/bin/python3', '-c', WEB_PROXY, str(web),
                                     str(vnc_port), str(port_file)])
            web_port = bound_port(viewer, port_file, r'^(\d+)$')
            url = f'http://127.0.0.1:{web_port}/vnc.html?autoconnect=true&resize=scale&view_only=true'
        manifest = {'display': env['DISPLAY'], 'viewer_url': url,
                    'vnc_address': f'127.0.0.1:{vnc_port}', 'app': str(app),
                    'pids': [p.pid for p in children], 'fixture': 'horizon-debug',
                    'session': 'saved' if args.saved_session else 'ephemeral',
                    'cloud_settings_files': cloud_files,
                    'session_bus': {'address': box.bus_address, 'apparmor': box.apparmor}}
        (args.state / 'target.json').write_text(json.dumps({'id': args.state.name, 'endpoint': {'kind': 'local_x11', 'display': env['DISPLAY']}}))
        (args.state / 'lab.json').write_text(json.dumps(manifest, indent=2))
        print(json.dumps(manifest), flush=True)
        while True:
            application_done = False
            for child in children:
                if child.poll() is not None:
                    if child is application_process and child.returncode == 0:
                        application_done = True
                    else:
                        raise RuntimeError(f'Lab process {child.pid} exited: {child.returncode}; inspect logs')
            if application_done:
                return
            time.sleep(0.5)
    except KeyboardInterrupt:
        pass
    finally:
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        # Expire the endpoint before its display number can be reused.
        (args.state / 'target.json').unlink(missing_ok=True)
        # Each child's whole process group, since ending bwrap alone leaves the
        # program it runs, and a bus daemon's activated services, running.
        for child in reversed(children):
            sandbox.end(child)
        for log in logs:
            log.close()
        # A reused state is what the next run starts from; a fresh one leaves nothing.
        if not args.reuse_state:
            shutil.rmtree(args.state / 'data', ignore_errors=True)


if __name__ == '__main__':
    sys.exit(main())
