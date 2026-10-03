"""Task-owned native desktop and real terminal pixels for whole-window evidence."""
import hashlib
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import sys
import time

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / 'scripts/device-smoke'))
import sandbox
from serve import bound_port


def fixture_output(counter):
    units = [255, 0] * 3 + [128]
    units += [255 if counter & (1 << bit) else 0 for bit in range(16)]
    units += [128] + [0, 255] * 3
    band = ''.join(f'\033[48;2;{v};{v};{v}m  ' for v in units) + '\033[0m'
    rows = ['WHOLE WINDOW / SYNTHETIC / NO AUDIO', '', band, band, '']
    for row in range(4):
        values = [30 if (column + row // 2) % 2 == 0 else 180 for column in range(8)]
        rows.append(''.join(f'\033[48;2;{v};{v};{v}m    ' for v in values) + '\033[0m')
    rows += ['', 'Stable text: Horizon window capture', f'Content counter: {counter:05d}']
    return '\033[H' + '\r\n'.join('\033[2K' + row for row in rows) + '\033[0m\033[J'


def emit_fixture():
    """Erase stale startup wrapping as terminal geometry settles."""
    print('\033[2J\033[?25l', end='', flush=True)
    counter = 0
    while True:
        print(fixture_output(counter), end='', flush=True)
        counter = (counter + 1) % 65536
        time.sleep(1 / 30)


def root_region(geometry):
    # Absolute client coordinates avoid counting WM reparent offsets twice.
    fields = dict(line.strip().split(':', 1) for line in geometry.splitlines() if ':' in line)
    return {key: int(fields[field]) for key, field in
            [('x', 'Absolute upper-left X'), ('y', 'Absolute upper-left Y'),
             ('width', 'Width'), ('height', 'Height')]}


def closing_screenshot(command):
    deadline = time.monotonic() + 10
    while True:
        result = subprocess.run(command, capture_output=True, text=True, timeout=15)
        receipt = json.loads(result.stdout)
        if receipt.get('ok') and result.returncode == 0:
            return receipt
        error = receipt.get('error', {})
        if ('device busy; another command is active' not in error.get('message', '')
                or time.monotonic() >= deadline):
            raise RuntimeError('closing screenshot failed: ' + str(receipt))
        time.sleep(0.1)


class Desktop:
    def __init__(self, root, binaries, tools=None):
        self.root = Path(root)
        self.children, self.logs = [], []
        self.application = None
        try:
            self.start(binaries, tools)
        except BaseException:
            self.close()
            raise

    def start(self, binaries, tools):
        self.box = sandbox.prepare(self.root, extra_ro_binds=[binaries, Path(__file__).parent, REPO / 'scripts/device-smoke', Path(sys.prefix),
                                                             *([tools] if tools else [])])
        self.namespace = self.box.namespace[:-3] + ['--proc', '/proc', '--bind', str(self.root), str(self.root)] + self.box.namespace[-3:]
        self.env, self.host_env = self.box.sandbox_env, self.box.host_env
        if tools:
            for env in [self.env, self.host_env]:
                env['PATH'] = str(tools / 'usr/bin') + ':' + env.get('PATH', '')
                env['LD_LIBRARY_PATH'] = str(tools / 'usr/lib/x86_64-linux-gnu')
        for name in ['Xvfb', 'openbox', 'x11vnc', 'bwrap', 'dbus-daemon', 'xdotool', 'xwininfo']:
            if not shutil.which(name, path=self.host_env.get('PATH')):
                raise RuntimeError(f'missing native fixture prerequisite: {name}')
        read_fd, write_fd = os.pipe()
        self.spawn('xvfb', ['Xvfb', '-displayfd', str(write_fd), '-screen', '0', '1600x1000x24',
                           '-nolisten', 'tcp', '-extension', 'MIT-SHM'], host=True, pass_fds=(write_fd,))
        os.close(write_fd)
        if not select.select([read_fd], [], [], 10)[0]:
            os.close(read_fd)
            raise RuntimeError('Xvfb readiness timeout')
        with os.fdopen(read_fd) as stream:
            number = stream.readline().strip()
        if not number.isdigit():
            raise RuntimeError('Xvfb did not allocate a display')
        for env in [self.env, self.host_env]:
            env.update(DISPLAY=':' + number, XDG_SESSION_TYPE='x11', LIBGL_ALWAYS_SOFTWARE='1')
        bus = self.spawn('dbus', sandbox.dbus_daemon_command(self.box.bus_address))
        sandbox.wait_for_unix_socket(self.box.host_socket, bus)
        self.spawn('openbox', ['openbox'])
        script = str(Path(__file__).with_name('whole_window.py'))
        def agent(role, position):
            return {'name': role.title() + ' controller', 'kind': 'codex', 'command': sys.executable,
                    'args': [script, 'agent', '--prepared', str(self.root), '--role', role],
                    'position': position, 'size': [230, 400]}
        fixture = {'version': 11, 'window': {'width': 1500, 'height': 920},
                   'appearance': {'theme': 'dark'}, 'workspaces': [
                       {'name': 'Whole-window benchmark', 'cwd': str(self.box.data), 'terminals': [
                           {'name': 'Grayscale workload', 'command': '/usr/bin/python3',
                            'args': [str(Path(__file__)), '--emit'], 'position': [40, 60], 'size': [1050, 570]},
                           agent('owner', [1130, 60])]},
                       {'name': 'Unapproved workspace', 'cwd': str(self.box.data),
                        'terminals': [agent('outsider', [40, 60])]}]}
        config = self.box.data / 'horizon.yaml'
        config.write_text(json.dumps(fixture))
        self.application = self.spawn('horizon', [str(binaries / 'horizon'), '--config', str(config), '--ephemeral'])
        vnc = self.spawn('vnc', ['x11vnc', '-norc', '-no6', '-display', self.env['DISPLAY'], '-localhost',
                                 '-autoport', '40000', '-viewonly', '-forever', '-shared', '-nopw', '-noxdamage', '-noshm'])
        port = bound_port(vnc, self.root / 'vnc.log', r'^PORT=(\d+)$')
        self.manifest = {'display': self.env['DISPLAY'], 'vnc_address': f'127.0.0.1:{port}',
                         'viewer_url': None, 'launcher_pid': self.application.pid,
                         'pids': [child.pid for child in self.children]}
        (self.root / 'lab.json').write_text(json.dumps(self.manifest, indent=2))
        (self.root / 'target.json').write_text(json.dumps({'id': self.root.name,
            'endpoint': {'kind': 'local_x11', 'display': self.env['DISPLAY']}}))

    def spawn(self, name, command, host=False, **kwargs):
        log = (self.root / f'{name}.log').open('wb')
        self.logs.append(log)
        process = subprocess.Popen(command if host else self.namespace + command,
                                   env=self.host_env if host else self.env, stdout=log, stderr=log,
                                   start_new_session=True, **kwargs)
        self.children.append(process)
        return process

    def close_normally(self, device):
        # Scope normal titlebar input to the verified isolated GUI.
        target = self.root / 'target.json'
        command = [str(device), '--target', str(target)]
        receipt = closing_screenshot(command + ['screenshot', str(self.root / 'close-before.png')])
        geometry = receipt['result']['geometry']
        if geometry['target_id'] != self.root.name:
            raise RuntimeError('closing screenshot returned another target')
        candidate = verify_candidate(self.application.pid, self.root / 'bin/horizon',
                                     self.root / 'data/horizon.yaml')
        windows = subprocess.check_output(['xdotool', 'search', '--onlyvisible', '--pid',
                  str(candidate['pid'])], env=self.host_env, text=True, timeout=10).split()
        if len(windows) != 1:
            raise RuntimeError('normal close requires one verified GUI window')
        window = subprocess.check_output(['xwininfo', '-id', windows[0]],
                    env={**self.host_env, 'LC_ALL': 'C'}, text=True, timeout=10)
        region = root_region(window)
        # This fixture owns the default Openbox titlebar; close it normally.
        at = {'x': region['x'] + region['width'] - 11, 'y': region['y'] - 11}
        if not (0 <= at['x'] < geometry['width'] and 0 <= at['y'] < geometry['height']):
            raise RuntimeError('owned titlebar close control is outside fresh screenshot')
        action = {'geometry': geometry, 'action': {'kind': 'click', 'at': at, 'button': 'left'}}
        result = subprocess.run(command + ['act', json.dumps(action)],
                                capture_output=True, text=True, timeout=15)
        if result.returncode or not json.loads(result.stdout).get('ok'):
            raise RuntimeError('normal close input failed: ' + result.stdout)
        self.application.wait(timeout=15)
        if self.application.returncode != 0:
            raise RuntimeError('Horizon did not close normally')

    def close(self):
        from bench import stop_group
        (self.root / 'target.json').unlink(missing_ok=True)
        failures = []
        for child in reversed(self.children):
            try:
                leaked = stop_group(child)
                if child is self.application and leaked:
                    failures.append('Horizon exited with descendants')
            except Exception as error:
                failures.append(str(error))
        for log in self.logs:
            log.close()
        if failures:
            raise RuntimeError('; '.join(failures))


def application_arguments(arguments, config):
    if '--browser-mcp' in arguments or '--ephemeral' not in arguments:
        return False
    if arguments.count('--config') != 1:
        return False
    at = arguments.index('--config') + 1
    return at < len(arguments) and Path(arguments[at]).resolve() == Path(config).resolve()


def verify_candidate(launcher, binary, config):
    from bench import process_tree
    digest = hashlib.sha256(Path(binary).read_bytes()).hexdigest()
    matches = []
    for pid in process_tree(launcher):
        try:
            executable = Path(f'/proc/{pid}/exe')
            arguments = Path(f'/proc/{pid}/cmdline').read_bytes().decode().rstrip('\0').split('\0')
            if (executable.resolve() == Path(binary).resolve()
                    and hashlib.sha256(executable.read_bytes()).hexdigest() == digest
                    and application_arguments(arguments, config)):
                ticks = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[19]
                matches.append({'pid': pid, 'start_ticks': ticks, 'binary_sha256': digest})
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            pass
    if len(matches) != 1:
        raise RuntimeError('exactly one verified Horizon child is required')
    return matches[0]


if __name__ == '__main__':
    emit_fixture()
