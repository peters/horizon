"""Agent browser tools reach the control service, and root never follows the agent's symlinks.

On a worker with agent isolation, agent sessions and their browser MCP run as UID 10001.
The two-user tests run the built control service and browser MCP with real ownership, so
they need root or passwordless sudo, setpriv and the binaries: HORIZON_TEST_CLOUD_WORKER,
and HORIZON_TEST_BROWSER or a horizon-browser beside it. The CI cloud worker step runs
after the workspace tests, which build horizon-browser for the browser CLI tests. The
real-browser test also needs unshare and Google Chrome or Chromium on the system PATH;
the CI Ubuntu runner and the worker image have Google Chrome.
"""
import importlib.machinery
import importlib.util
import http.server
import json
import os
from pathlib import Path
import runpy
import select
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import threading
from types import SimpleNamespace
import unittest
from unittest import mock


AGENT = 10001
# Another owner than the account that runs the tests, for simulated ownership.
OTHER = os.getuid() + 1
WORKER = os.environ.get('HORIZON_TEST_CLOUD_WORKER')


def built_browser():
    if os.environ.get('HORIZON_TEST_BROWSER'):
        return os.environ['HORIZON_TEST_BROWSER']
    beside = Path(WORKER).with_name('horizon-browser') if WORKER else None
    return str(beside) if beside and beside.is_file() else None


BROWSER = built_browser()
# The service and the browser MCP run with this PATH; Chromium must be on it, not only in a snap.
SYSTEM_PATH = '/usr/local/bin:/usr/bin:/bin'
CHROMIUM = next(filter(None, (shutil.which(name, path=SYSTEM_PATH) for name in
                              ('google-chrome', 'google-chrome-stable', 'chromium', 'chromium-browser'))), None)
SESSION = 'lane'
SUPERVISE = runpy.run_path(str(Path(__file__).with_name('horizon-worker-supervise')))
_loader = importlib.machinery.SourceFileLoader('tailnet_lane', str(Path(__file__).with_name('horizon-worker-tailnet')))
TAILNET = importlib.util.module_from_spec(importlib.util.spec_from_loader(_loader.name, _loader))
_loader.exec_module(TAILNET)
DENIED = 'the operating system denied access to a host coordination file'


def root_available():
    # CI runners grant passwordless sudo; the two-user tests then run there as root.
    if os.geteuid() == 0:
        return True
    return bool(shutil.which('sudo')) and subprocess.run(
        ['sudo', '-n', 'true'], capture_output=True, timeout=20, check=False).returncode == 0


def as_agent(command):
    return ['setpriv', f'--reuid={AGENT}', f'--regid={AGENT}', '--clear-groups', '--no-new-privs',
            '--bounding-set=-all', '--', *command]


class BrowserRootHandoffTests(unittest.TestCase):
    """At worker start, root gives a browser runtime root with root-owned entries to the agent, never a link."""

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.temp = Path(temporary.name)
        self.workspace = self.temp / 'workspace'
        (self.workspace / 'home').mkdir(parents=True)
        self.browser_root = self.workspace / 'home/.horizon'

    def hand_off(self, owners=None):
        def path(*parts):
            value = Path(*parts)
            return self.workspace / value.relative_to('/workspace') if value.is_relative_to('/workspace') else value
        real = os.lstat
        owners = {str(path): uid for path, uid in (owners or {}).items()}

        def lstat(name):
            # Simulated owners: only root can make files for another account.
            info = real(name)
            uid = owners.get(os.path.normpath(name), info.st_uid)
            return SimpleNamespace(st_uid=uid, st_mode=info.st_mode)
        with mock.patch.object(TAILNET, 'Path', side_effect=path), mock.patch.object(TAILNET, 'USER', os.getuid()), \
                mock.patch.object(TAILNET.os, 'lstat', side_effect=lstat), \
                mock.patch.object(TAILNET.subprocess, 'run') as run:
            TAILNET.handoff_browser_root()
        return [call.args[0] for call in run.call_args_list]

    def chown(self):
        return [['/usr/bin/chown', '-R', '--no-dereference', f'{os.getuid()}:{os.getuid()}', str(self.browser_root)]]

    def test_an_earlier_root_owned_browser_root_is_handed_over_without_following_links(self):
        (self.browser_root / 'cloud-browser-history').mkdir(parents=True)
        self.assertEqual(self.hand_off({self.browser_root: OTHER}), self.chown())

    def test_a_root_owned_entry_inside_an_agent_owned_root_is_handed_over(self):
        history = self.browser_root / 'cloud-browser-history'
        (history / 'remote-holds').mkdir(parents=True)
        self.assertEqual(self.hand_off(), [], 'the agent already owns every entry')
        self.assertEqual(self.hand_off({(history / 'remote-holds'): OTHER}), self.chown())

    def test_a_symlink_is_the_agents_own_and_root_leaves_it(self):
        protected = self.temp / 'protected'
        protected.mkdir()
        (protected / 'secret').write_text('unchanged')
        self.browser_root.symlink_to(protected)
        self.assertEqual(self.hand_off({protected: OTHER, (protected / 'secret'): OTHER}), [])
        self.assertTrue(self.browser_root.is_symlink())
        self.assertEqual((protected / 'secret').read_text(), 'unchanged')

    def test_a_symlink_inside_the_root_is_not_followed(self):
        protected = self.temp / 'protected'
        protected.mkdir()
        self.browser_root.mkdir()
        (self.browser_root / 'link').symlink_to(protected)
        self.assertEqual(self.hand_off({protected: OTHER}), [], 'the target of a link is never examined')

    def test_nothing_to_hand_off_without_a_browser_runtime_root(self):
        self.assertEqual(self.hand_off(), [])

    def test_worker_start_hands_off_the_browser_root_after_isolation_and_before_any_agent(self):
        calls = []

        class Exec(Exception):
            pass

        def record(name):
            def call(*_args):
                calls.append(name)
                if name == 'agent':
                    raise Exec()
            return call
        with mock.patch.object(TAILNET.sys, 'argv', ['horizon-worker-tailnet', 'isolate']), \
                mock.patch.multiple(TAILNET, private_directory=mock.DEFAULT, prepare_agents=record('prepare_agents'),
                                    handoff_browser_root=record('handoff_browser_root'), agent=record('agent')):
            with self.assertRaises(Exec):
                TAILNET.main()
        self.assertEqual(calls, ['prepare_agents', 'handoff_browser_root', 'agent'])


@unittest.skipUnless(WORKER and BROWSER, 'set HORIZON_TEST_CLOUD_WORKER and HORIZON_TEST_BROWSER to the built binaries')
@unittest.skipUnless(shutil.which('setpriv') and root_available(),
                     'needs root or passwordless sudo to run the services as the worker agent')
class BrowserLaneTests(unittest.TestCase):
    def setUp(self):
        if os.geteuid() != 0:
            return
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.root.chmod(0o755)
        self.home = self.root / 'home'
        self.home.mkdir(mode=0o700)
        os.chown(self.home, AGENT, AGENT)
        self.browser_root = self.home / '.horizon'
        # The agent must be able to run the binaries; a checkout under a private home is not enough.
        self.bin = self.root / 'bin'
        self.bin.mkdir(mode=0o755)
        for name, source in [('horizon-cloud-worker', WORKER), ('horizon-browser', BROWSER)]:
            shutil.copyfile(source, self.bin / name)
            (self.bin / name).chmod(0o755)
        self.environment = {'PATH': SYSTEM_PATH, 'HOME': str(self.home),
                            'HORIZON_BROWSER_ROOT': str(self.browser_root)}

    def nested(self):
        # Rerun this test as root. sudo resets the environment, so pass the binaries explicitly.
        name = f'{Path(__file__).stem}.{type(self).__name__}.{self._testMethodName}'
        nested = subprocess.run(['sudo', '-n', 'env', f'HORIZON_TEST_CLOUD_WORKER={WORKER}',
                                 f'HORIZON_TEST_BROWSER={BROWSER}', sys.executable, '-B', '-m', 'unittest', name],
                                cwd=Path(__file__).parent, capture_output=True, text=True, timeout=300)
        self.assertEqual(nested.returncode, 0, nested.stderr)
        self.assertNotIn('skipped', nested.stderr)

    def serve(self, agent, host, workspace=None):
        environment = dict(self.environment, **{SUPERVISE['HOST_INSTANCE_ENV']: host})
        command = [str(self.bin / 'horizon-cloud-worker'), 'serve']
        command = as_agent(command) if agent else command
        if workspace:
            command = self.in_workspace(workspace, command)
        log = (self.root / f'control-{host}.log').open('wb')
        self.addCleanup(log.close)
        # As the supervisor starts it: umask 077, its own session, the agent account with isolation.
        service = subprocess.Popen(command, env=environment, stdout=log,
                                   stderr=subprocess.STDOUT, start_new_session=True,
                                   preexec_fn=lambda: os.umask(0o077))
        self.addCleanup(self.stop, service)
        return service

    @staticmethod
    def stop(service):
        try:
            os.killpg(service.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        service.wait(timeout=10)

    def await_answer(self, service):
        deadline = time.monotonic() + 30
        while not SUPERVISE['control_answers']():
            self.assertIsNone(service.poll(), 'the control service exited')
            self.assertLess(time.monotonic(), deadline, 'the control service did not answer')
            time.sleep(.1)

    def in_workspace(self, workspace, command):
        # The service reads its sessions and capabilities from /workspace. A private mount
        # namespace shows it this test's workspace; a real /workspace is not changed.
        if not os.path.isdir('/workspace'):
            os.mkdir('/workspace', 0o755)
            self.addCleanup(os.rmdir, '/workspace')
        return ['unshare', '--mount', '--propagation', 'private', '--', 'sh', '-c',
                'mount --bind "$0" /workspace && exec "$@"', str(workspace), *command]

    def workspace(self):
        # As on a worker: the session of the agent and the capabilities of the cloud.
        workspace = self.root / 'workspace'
        (workspace / 'sessions' / SESSION).mkdir(parents=True)
        for path in [workspace, workspace / 'sessions', workspace / 'sessions' / SESSION]:
            path.chmod(0o755)
        (workspace / 'capabilities.json').write_text(json.dumps({'agents': ['claude'], 'browsers': ['chromium']}))
        return workspace

    def mcp(self, host):
        environment = dict(self.environment, HORIZON_BROWSER_ACTOR=f'horizon:cloud-{SESSION}',
                           HORIZON_BROWSER_HOST_INSTANCE=host)
        mcp = subprocess.Popen(as_agent([str(self.bin / 'horizon-browser'), 'mcp', '--connect']), env=environment,
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                               start_new_session=True, bufsize=0)
        self.addCleanup(mcp.stdout.close)
        self.addCleanup(mcp.stdin.close)
        self.addCleanup(self.stop, mcp)
        identifiers = iter(range(1, 1000))

        def send(message):
            mcp.stdin.write((json.dumps(message) + '\n').encode())
            mcp.stdin.flush()

        def request(method, params):
            identifier = next(identifiers)
            send({'jsonrpc': '2.0', 'id': identifier, 'method': method, 'params': params})
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                # A silent MCP must not hold the suite past the deadline. The pipe is unbuffered, so
                # no line waits in a Python buffer that select cannot see; the MCP writes whole lines.
                readable, _, _ = select.select([mcp.stdout], [], [], max(0, deadline - time.monotonic()))
                if not readable:
                    break
                line = mcp.stdout.readline()
                self.assertTrue(line, 'the browser MCP stopped before it answered')
                reply = json.loads(line)
                if reply.get('id') == identifier:
                    return reply['result']
            self.fail(f'{method} did not answer')

        request('initialize', {'protocolVersion': '2025-06-18', 'capabilities': {},
                               'clientInfo': {'name': 'browser-lane-test', 'version': '1'}})
        send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
        return lambda tool, arguments: request('tools/call', {'name': tool, 'arguments': arguments})

    def browser_create(self, host):
        return self.mcp(host)('browser_create', {'backend': 'firefox', 'visible': False, 'timeout_millis': 5000})

    @staticmethod
    def text(result):
        return ' '.join(part.get('text', '') for part in result['content'])

    def answer(self, result):
        self.assertFalse(result.get('isError'), self.text(result))
        return json.loads(self.text(result))

    def page(self):
        body = b'<!doctype html><title>Lane page</title><h1>Agent lane</h1>'

        class Page(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(200)
                self.send_header('Content-Type', 'text/html')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *_args):
                pass
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Page)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        return f'http://127.0.0.1:{server.server_address[1]}/'

    def watch_browser_errors(self):
        # Diagnostics only: the create result names no cause when a browser stops at start,
        # but the control service shows the browser's own error while it holds the browser.
        errors, done = set(), threading.Event()

        def watch():
            while not done.is_set():
                try:
                    with socket.create_connection(SUPERVISE['CONTROL_ENDPOINT'], timeout=1) as connection:
                        connection.settimeout(2)
                        connection.sendall(b'{"operation":"list"}\n')
                        with connection.makefile('rb') as stream:
                            reply = json.loads(stream.readline(1 << 20))
                    errors.update(browser['error'] for browser in reply.get('browsers', []) if browser.get('error'))
                except (OSError, ValueError):
                    pass
                done.wait(.05)
        watcher = threading.Thread(target=watch, daemon=True)
        watcher.start()

        def stop():
            done.set()
            watcher.join(timeout=5)
            return sorted(errors)
        return stop

    def browser_processes(self):
        # Every browser process of this test: its profile is in the agent's home.
        processes = {}
        for entry in Path('/proc').iterdir():
            try:
                arguments = (entry / 'cmdline').read_bytes().split(b'\0')
                status = (entry / 'status').read_text()
            except (FileNotFoundError, ProcessLookupError, NotADirectoryError, PermissionError):
                continue
            if any(argument.startswith(b'--user-data-dir=' + bytes(self.home)) for argument in arguments):
                processes[int(entry.name)] = dict(line.split(':\t', 1) for line in status.splitlines() if ':\t' in line)
        return processes

    def migrate(self):
        # The worker start handoff of horizon-worker-tailnet, with this test's paths, as root.
        real = Path

        def path(*parts):
            value = real(*parts)
            if value == real('/workspace/home/.horizon'):
                return self.browser_root
            return value
        with mock.patch.object(TAILNET, 'Path', side_effect=path):
            TAILNET.handoff_browser_root()

    def test_agent_browser_create_reaches_the_control_service_only_as_the_agent_lane(self):
        if os.geteuid() != 0:
            return self.nested()
        # Before: a root control service keeps a private browser runtime root (issue 1307).
        root_service = self.serve(agent=False, host='root-host')
        self.await_answer(root_service)
        refused = self.browser_create('root-host')
        self.assertTrue(refused.get('isError'))
        self.assertIn(DENIED, self.text(refused))
        self.stop(root_service)
        self.assertEqual(self.browser_root.stat().st_uid, 0)

        # After: the handoff gives the earlier root its agent owner, and the service runs as the agent.
        self.migrate()
        service = self.serve(agent=True, host='agent-host')
        self.await_answer(service)
        result = self.browser_create('agent-host')
        text = self.text(result)
        self.assertNotIn(DENIED, text)
        # The request asks for Firefox, which the default worker capabilities leave out, so no
        # browser starts on the test machine. The service's own refusal proves that the request
        # reached the control service and that its result came back to the agent.
        self.assertTrue(result.get('isError'))
        self.assertIn('browser_start_failed', text)
        self.assertIn('Requested browser is disabled by this cloud profile', text)
        for path in [self.browser_root, *self.browser_root.rglob('*')]:
            self.assertEqual(path.lstat().st_uid, AGENT, path)

    @unittest.skipUnless(CHROMIUM, 'needs Google Chrome or Chromium on the system PATH, as on the worker image')
    @unittest.skipUnless(shutil.which('unshare'), 'needs unshare to show the service its workspace')
    def test_agent_browser_create_starts_a_real_chromium_as_the_agent_lane(self):
        if os.geteuid() != 0:
            return self.nested()
        url = self.page()
        service = self.serve(agent=True, host='agent-host', workspace=self.workspace())
        self.addCleanup(subprocess.run, ['pkill', '-KILL', '-f', '--', f'--user-data-dir={self.home}'], check=False)
        self.await_answer(service)
        call = self.mcp('agent-host')

        browser_errors = self.watch_browser_errors()
        result = call('browser_create', {'backend': 'chromium', 'url': url, 'visible': False, 'timeout_millis': 60000})
        errors = browser_errors()
        self.assertFalse(result.get('isError'), f'{self.text(result)}; browser errors: {errors}; control log: '
                         f"{(self.root / 'control-agent-host.log').read_text(errors='replace')[-4000:]}")
        created = self.answer(result)
        panel = created['panel']['panel_id']
        self.assertEqual(created['panel']['owner'], f'horizon:cloud-{SESSION}')
        processes = self.browser_processes()
        self.assertTrue(processes, 'no browser process has a profile in the agent home')
        for pid, status in processes.items():
            # The same isolation as the service: the agent account, no new privileges, no capabilities.
            self.assertEqual(status['Uid'].split(), [str(AGENT)] * 4, pid)
            self.assertEqual(status['NoNewPrivs'].strip(), '1', pid)
            self.assertEqual(int(status['CapBnd'], 16), 0, pid)

        self.answer(call('browser_wait', {'panel_id': panel, 'selector': 'h1', 'state': 'visible'}))
        snapshot = self.answer(call('browser_snapshot', {'panel_id': panel}))
        self.assertEqual(snapshot['title'], 'Lane page')
        self.assertIn('Agent lane', json.dumps(snapshot['nodes']))

        self.assertTrue(self.answer(call('browser_close', {'panel_id': panel}))['closed'])
        self.assertEqual(self.answer(call('browser_list', {}))['panels'], [])
        deadline = time.monotonic() + 30
        while self.browser_processes():
            self.assertLess(time.monotonic(), deadline, 'the browser processes did not stop')
            time.sleep(.2)
        for path in [self.browser_root, *self.browser_root.rglob('*')]:
            self.assertEqual(path.lstat().st_uid, AGENT, path)

    def test_root_never_follows_a_symlink_that_the_agent_planted(self):
        if os.geteuid() != 0:
            return self.nested()
        protected = self.root / 'protected'
        protected.mkdir(mode=0o700)
        (protected / 'secret').write_text('unchanged')
        (protected / 'secret').chmod(0o600)
        before = sorted(path.name for path in protected.iterdir())
        subprocess.run(as_agent(['ln', '-s', str(protected), str(self.browser_root)]), check=True)

        self.migrate()
        self.assertTrue(self.browser_root.is_symlink())
        self.assertEqual(self.browser_root.lstat().st_uid, AGENT)
        self.assertEqual((protected.stat().st_uid, protected.stat().st_mode & 0o777), (0, 0o700))
        self.assertEqual((protected / 'secret').stat().st_uid, 0)

        # The service follows the agent's link with the agent's rights only, so it cannot start there.
        service = self.serve(agent=True, host='agent-host')
        self.assertNotEqual(service.wait(timeout=30), 0)
        self.assertIn('Permission denied', (self.root / 'control-agent-host.log').read_text())
        self.assertEqual(sorted(path.name for path in protected.iterdir()), before)
        self.assertEqual((protected / 'secret').read_text(), 'unchanged')


if __name__ == '__main__':
    unittest.main()
