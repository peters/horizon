#!/usr/bin/env python3
"""Own a disposable application/X11 lab and read-only noVNC viewer until Ctrl-C."""
import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import re
import subprocess
import time

import sandbox


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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--horizon', type=Path, required=True, help='Horizon debug binary for an isolated smoke')
    parser.add_argument('--tools', type=Path, help='Optional unpacked Debian tools root')
    parser.add_argument('--native-view', action='store_true', help='Expose direct VNC without noVNC/websockify')
    parser.add_argument('--device-address', help='Read-only native Device panel endpoint inside the Horizon fixture')
    parser.add_argument('--state', type=Path, required=True, help='New private evidence directory')
    args = parser.parse_args()
    args.state.mkdir(mode=0o700, parents=True, exist_ok=False)
    args.state = args.state.resolve()
    app = args.horizon.resolve(strict=True)
    box = sandbox.prepare(args.state)
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

    def spawn(name, command, child_env=env):
        log = (args.state / f'{name}.log').open('wb')
        logs.append(log)
        process = subprocess.Popen(namespace + command, env=child_env, stdout=log, stderr=log)
        children.append(process)
        return process

    def stop(_signal, _frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, stop)
    try:
        # Xvfb chooses and locks an unused display atomically.
        read_fd, write_fd = os.pipe()
        log = (args.state / 'xvfb.log').open('wb')
        logs.append(log)
        xvfb = subprocess.Popen(['Xvfb', '-displayfd', str(write_fd), '-screen', '0',
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
        fixture = {
            'version': 11, 'window': {'width': 1480, 'height': 900},
            'appearance': {'theme': 'dark'},
            'workspaces': [{'name': 'Disposable VNC debug', 'cwd': str(data.resolve()),
                'terminals': [
                    {'name': 'Live render heartbeat', 'command': '/usr/bin/python3',
                     'args': ['-u', '-c', "import time\nprint('HORIZON DEBUG / noVNC LIVE VIEW')\nfor i in range(3600):\n print('Live frame heartbeat:', i, flush=True); time.sleep(1)"],
                     'position': [40, 60], 'size': [550, 420]},
                    {'name': 'Device input test', 'command': '/bin/bash',
                     'args': ['--noprofile', '--norc'],
                     'position': [630, 60], 'size': [550, 420]}]}]}
        if args.device_address:
            fixture['workspaces'][0]['terminals'] = [
                {'name': 'Native device view', 'kind': 'device', 'command': args.device_address,
                 'position': [40, 60], 'size': [1150, 780]}]
        config.write_text(json.dumps(fixture))  # JSON is a YAML subset.
        # All client helpers see the same masked home and private XDG paths.
        application_process = spawn('horizon', [str(app), '--config', str(config), '--ephemeral'])
        vnc = spawn('vnc', ['x11vnc', '-norc', '-no6', '-display', env['DISPLAY'],
                           '-localhost', '-autoport', '40000', '-viewonly', '-forever',
                           '-shared', '-nopw', '-noxdamage', '-noshm'])
        vnc_port = bound_port(vnc, args.state / 'vnc.log', r'^PORT=(\d+)$')
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
        for child in reversed(children):
            if child.poll() is None:
                child.terminate()
        for child in reversed(children):
            if child.poll() is None:
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
        for log in logs:
            log.close()
        shutil.rmtree(args.state / 'data', ignore_errors=True)


if __name__ == '__main__':
    main()
