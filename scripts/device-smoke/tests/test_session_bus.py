"""Private D-Bus namespace and session-bus lifecycle for the isolated desktop fixture (#767)."""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import sandbox  # noqa: E402


def bind_at(namespace, dest):
    for index, item in enumerate(namespace):
        if item in ('--bind', '--ro-bind', '--ro-bind-try', '--dev-bind') and index + 2 < len(namespace):
            if namespace[index + 2] == dest:
                return index, item, namespace[index + 1]
    raise AssertionError(f'no bind to {dest}')


def bwrap_usable():
    if not shutil.which('bwrap'):
        return False
    result = subprocess.run(
        ['bwrap', '--die-with-parent', '--ro-bind', '/', '/', 'true'],
        capture_output=True, timeout=5)
    return result.returncode == 0


def host_bus_id():
    env = os.environ
    host_socket = Path(sandbox.runtime_dest()) / sandbox.BUS_SOCKET_NAME
    if not env.get('DBUS_SESSION_BUS_ADDRESS') and not host_socket.exists():
        return None
    result = subprocess.run(
        ['dbus-send', '--session', '--dest=org.freedesktop.DBus', '--print-reply',
         '/org/freedesktop/DBus', 'org.freedesktop.DBus.GetId'],
        env=env, text=True, capture_output=True, timeout=10)
    if result.returncode != 0:
        return None
    return result.stdout


@unittest.skipUnless(sys.platform.startswith('linux'), 'Linux isolated desktop fixture')
class NamespaceTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='horizon-dbus-ns-')
        self.addCleanup(self.directory.cleanup)
        self.box = sandbox.prepare(self.directory.name)

    def test_private_tmp_is_writable_and_state_is_rebound_after_it(self):
        tmp_index, tmp_flag, tmp_src = bind_at(self.box.namespace, '/tmp')
        state_index, state_flag, state_src = bind_at(self.box.namespace, str(self.box.state))
        self.assertEqual(tmp_flag, '--bind')
        self.assertEqual(state_flag, '--bind')
        self.assertEqual(Path(tmp_src), self.box.private_tmp)
        self.assertEqual(Path(state_src), self.box.state)
        self.assertLess(tmp_index, state_index)

    def test_session_bus_uses_standard_runtime_path_backed_by_fixture_runtime(self):
        self.assertIn('--dir', self.box.namespace)
        self.assertLess(
            self.box.namespace.index('--dir'),
            bind_at(self.box.namespace, self.box.runtime_dest)[0],
        )
        _, flag, src = bind_at(self.box.namespace, self.box.runtime_dest)
        self.assertEqual(flag, '--bind')
        self.assertEqual(Path(src), self.box.runtime)
        self.assertNotEqual(src, self.box.runtime_dest)
        self.assertEqual(self.box.bus_address, f'unix:path={self.box.runtime_dest}/{sandbox.BUS_SOCKET_NAME}')
        self.assertEqual(self.box.sandbox_env['DBUS_SESSION_BUS_ADDRESS'], self.box.bus_address)
        self.assertEqual(self.box.sandbox_env['XDG_RUNTIME_DIR'], self.box.runtime_dest)
        self.assertEqual(self.box.host_env['XDG_RUNTIME_DIR'], str(self.box.runtime))
        self.assertNotIn('DBUS_SESSION_BUS_ADDRESS', self.box.host_env)

    def test_host_session_bus_address_is_not_inherited(self):
        env = dict(os.environ)
        env['DBUS_SESSION_BUS_ADDRESS'] = 'unix:path=/definitely/developer/bus'
        env['DBUS_STARTER_ADDRESS'] = env['DBUS_SESSION_BUS_ADDRESS']
        env['DISPLAY'] = ':0'
        with tempfile.TemporaryDirectory(prefix='horizon-dbus-strip-') as raw:
            box = sandbox.prepare(raw, environ=env)
            self.assertNotIn('DBUS_SESSION_BUS_ADDRESS', box.host_env)
            self.assertNotIn('DBUS_STARTER_ADDRESS', box.sandbox_env)
            self.assertNotEqual(box.sandbox_env['DBUS_SESSION_BUS_ADDRESS'], env['DBUS_SESSION_BUS_ADDRESS'])
            self.assertNotIn('DISPLAY', box.host_env)
            self.assertNotIn('DISPLAY', box.sandbox_env)

    def test_apparmor_bind_is_the_query_file_only(self):
        if sandbox.APPARMOR_ACCESS.is_file():
            _, flag, src = bind_at(self.box.namespace, str(sandbox.APPARMOR_ACCESS))
            self.assertEqual(flag, '--bind')
            self.assertEqual(src, str(sandbox.APPARMOR_ACCESS))
            self.assertEqual(self.box.apparmor['status'], 'query_bind')
            self.assertTrue(self.box.apparmor['sandbox_query_bind'])
            with self.assertRaises(AssertionError):
                bind_at(self.box.namespace, '/sys/kernel/security')
            with self.assertRaises(AssertionError):
                bind_at(self.box.namespace, '/sys/kernel/security/apparmor/.remove')
        else:
            self.assertEqual(self.box.apparmor['status'], 'absent')
            self.assertFalse(self.box.apparmor['sandbox_query_bind'])

    def test_x11_socket_dir_is_rebound_read_only_after_private_tmp(self):
        tmp_index, _, _ = bind_at(self.box.namespace, '/tmp')
        x11_index, flag, src = bind_at(self.box.namespace, str(sandbox.X11_SOCKET_DIR))
        self.assertEqual(flag, '--ro-bind-try')
        self.assertEqual(src, str(sandbox.X11_SOCKET_DIR))
        self.assertLess(tmp_index, x11_index)

    def test_executables_under_tmp_are_rebound_after_private_tmp(self):
        with tempfile.TemporaryDirectory(prefix='horizon-smoke-bin-', dir='/tmp') as raw:
            binary = Path(raw) / 'horizon'
            binary.write_text('#!/bin/sh\necho rebound-ok\n')
            binary.chmod(0o755)
            with tempfile.TemporaryDirectory(prefix='horizon-dbus-extra-') as state:
                box = sandbox.prepare(state, extra_ro_binds=[binary])
                tmp_index, _, _ = bind_at(box.namespace, '/tmp')
                bin_index, flag, src = bind_at(box.namespace, str(binary))
                self.assertEqual(flag, '--ro-bind')
                self.assertEqual(src, str(binary))
                self.assertLess(tmp_index, bin_index)
                if not bwrap_usable():
                    self.skipTest('bwrap user namespace required')
                result = subprocess.run(
                    box.namespace + [str(binary)], env=box.sandbox_env,
                    text=True, capture_output=True, timeout=10)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn('rebound-ok', result.stdout)

    def test_dbus_daemon_overrides_the_system_listen_address(self):
        command = sandbox.dbus_daemon_command(self.box.bus_address)
        self.assertEqual(command[0], 'dbus-daemon')
        self.assertIn('--session', command)
        self.assertIn(f'--address={self.box.bus_address}', command)
        self.assertIn('--nofork', command)


@unittest.skipUnless(
    sys.platform.startswith('linux')
    and shutil.which('bwrap')
    and shutil.which('dbus-daemon')
    and shutil.which('dbus-send')
    and shutil.which('dbus-run-session')
    and shutil.which('dbus-monitor')
    and bwrap_usable(),
    'bwrap user namespace and D-Bus tools required',
)
class LiveSessionBusTests(unittest.TestCase):
    def start_box(self, bind_apparmor_query=True):
        directory = tempfile.TemporaryDirectory(prefix='horizon-dbus-live-')
        self.addCleanup(directory.cleanup)
        box = sandbox.prepare(directory.name, bind_apparmor_query=bind_apparmor_query)
        processes = []

        def close():
            for process in reversed(processes):
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()

        self.addCleanup(close)
        return box, processes

    def start_bus(self, box, processes):
        log = (box.state / 'dbus.log').open('wb')
        self.addCleanup(log.close)
        process = subprocess.Popen(
            box.namespace + sandbox.dbus_daemon_command(box.bus_address),
            env=box.sandbox_env, stdout=log, stderr=log)
        processes.append(process)
        try:
            sandbox.wait_for_unix_socket(box.host_socket, process)
        except RuntimeError as error:
            log.flush()
            raise RuntimeError(f'{error}: {(box.state / "dbus.log").read_text()}') from error
        return process

    def in_box(self, box, command, timeout=10, env=None):
        return subprocess.run(
            box.namespace + command, env=env or box.sandbox_env,
            text=True, capture_output=True, timeout=timeout)

    def test_session_bus_method_call_and_nested_dbus_run_session(self):
        box, processes = self.start_box()
        self.start_bus(box, processes)
        owned = self.in_box(
            box, ['dbus-send', '--session', '--dest=org.freedesktop.DBus', '--print-reply',
                  '/org/freedesktop/DBus', 'org.freedesktop.DBus.GetId'])
        self.assertEqual(owned.returncode, 0, owned.stderr)
        self.assertIn('string', owned.stdout)
        nested = self.in_box(
            box, ['dbus-run-session', '--', 'dbus-send', '--session', '--dest=org.freedesktop.DBus',
                  '--print-reply', '/org/freedesktop/DBus', 'org.freedesktop.DBus.GetId'])
        self.assertEqual(nested.returncode, 0, nested.stderr)
        self.assertIn('string', nested.stdout)
        self.assertNotEqual(owned.stdout, nested.stdout)
        host = host_bus_id()
        if host is not None:
            self.assertNotIn(owned.stdout.strip(), host)
            self.assertNotEqual(owned.stdout, host)

    def test_native_client_stays_connected_on_the_private_bus(self):
        box, processes = self.start_box()
        self.start_bus(box, processes)
        log_path = box.state / 'monitor.log'
        with log_path.open('wb') as log:
            monitor = subprocess.Popen(
                box.namespace + ['dbus-monitor', '--session'],
                env=box.sandbox_env, stdout=log, stderr=subprocess.STDOUT)
        processes.append(monitor)
        deadline = time.monotonic() + 5
        text = ''
        while time.monotonic() < deadline:
            text = log_path.read_text()
            if 'NameAcquired' in text:
                break
            self.assertIsNone(monitor.poll(), text)
            time.sleep(0.05)
        self.assertIn('NameAcquired', text)
        self.assertIsNone(monitor.poll(), text)
        reply = self.in_box(
            box, ['dbus-send', '--session', '--dest=org.freedesktop.DBus', '--print-reply',
                  '/org/freedesktop/DBus', 'org.freedesktop.DBus.GetId'])
        self.assertEqual(reply.returncode, 0, reply.stderr)
        self.assertIn('string', reply.stdout)

    def test_repeated_launch_expires_the_private_socket(self):
        sockets = []
        for _ in range(2):
            directory = tempfile.TemporaryDirectory(prefix='horizon-dbus-repeat-')
            box = sandbox.prepare(directory.name)
            process = subprocess.Popen(
                box.namespace + sandbox.dbus_daemon_command(box.bus_address),
                env=box.sandbox_env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                sandbox.wait_for_unix_socket(box.host_socket, process)
                reply = self.in_box(
                    box, ['dbus-send', '--session', '--dest=org.freedesktop.DBus', '--print-reply',
                          '/org/freedesktop/DBus', 'org.freedesktop.DBus.GetId'])
                self.assertEqual(reply.returncode, 0, reply.stderr)
                sockets.append(box.host_socket)
                self.assertTrue(box.host_socket.exists())
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
                directory.cleanup()
            self.assertFalse(sockets[-1].exists())
        self.assertEqual(len(sockets), 2)
        self.assertNotEqual(sockets[0], sockets[1])

    def test_apparmor_query_file_is_writable_only_with_the_query_bind(self):
        if not sandbox.APPARMOR_ACCESS.is_file():
            self.skipTest('no AppArmor query interface')
        probe = (
            "import os\n"
            f"p={str(sandbox.APPARMOR_ACCESS)!r}\n"
            "try:\n"
            "  open(p,'r+b',buffering=0).close()\n"
            "  print('open-ok')\n"
            "except OSError as e:\n"
            "  print(e.errno)\n"
        )
        bound, _bound_procs = self.start_box(bind_apparmor_query=True)
        unbound, _unbound_procs = self.start_box(bind_apparmor_query=False)
        opened = self.in_box(bound, ['python3', '-c', probe])
        blocked = self.in_box(unbound, ['python3', '-c', probe])
        self.assertEqual(opened.returncode, 0, opened.stderr)
        self.assertIn('open-ok', opened.stdout)
        self.assertEqual(blocked.returncode, 0, blocked.stderr)
        self.assertIn('30', blocked.stdout)
        removed = self.in_box(
            bound,
            ['python3', '-c',
             "open('/sys/kernel/security/apparmor/.remove','a').close()"])
        self.assertNotEqual(removed.returncode, 0)
        self.assertIn('Read-only file system', removed.stderr)

    @unittest.skipUnless(
        shutil.which('Xvfb') and shutil.which('gnome-calculator'),
        'Xvfb and gnome-calculator required',
    )
    def test_gnome_calculator_uses_the_private_bus(self):
        box, processes = self.start_box()
        read_fd, write_fd = os.pipe()
        xvfb = subprocess.Popen(
            ['Xvfb', '-displayfd', str(write_fd), '-screen', '0', '800x600x24',
             '-nolisten', 'tcp', '-extension', 'MIT-SHM'],
            pass_fds=(write_fd,), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            env=box.host_env)
        os.close(write_fd)
        processes.append(xvfb)
        import select
        if not select.select([read_fd], [], [], 10)[0]:
            self.fail('Xvfb startup timed out')
        with os.fdopen(read_fd) as pipe:
            number = pipe.readline().strip()
        self.assertTrue(number.isdigit())
        box.sandbox_env['DISPLAY'] = ':' + number
        box.sandbox_env['XDG_SESSION_TYPE'] = 'x11'
        box.sandbox_env['GTK_A11Y'] = 'none'
        box.sandbox_env['NO_AT_BRIDGE'] = '1'
        self.start_bus(box, processes)
        log_path = box.state / 'calculator.log'
        with log_path.open('wb') as log:
            calculator = subprocess.Popen(
                box.namespace + ['gnome-calculator'], env=box.sandbox_env,
                stdout=log, stderr=subprocess.STDOUT)
        processes.append(calculator)
        started = time.monotonic()
        owned = False
        while time.monotonic() - started < 8:
            text = log_path.read_text()
            self.assertNotIn('Failed to query AppArmor policy', text)
            self.assertNotIn('Unable to acquire session bus', text)
            self.assertIsNone(calculator.poll(), text)
            names = self.in_box(
                box, ['dbus-send', '--session', '--dest=org.freedesktop.DBus', '--print-reply',
                      '/org/freedesktop/DBus', 'org.freedesktop.DBus.ListNames'])
            if names.returncode == 0 and 'org.gnome.Calculator' in names.stdout:
                owned = True
                break
            time.sleep(0.2)
        text = log_path.read_text()
        self.assertNotIn('Failed to query AppArmor policy', text)
        self.assertNotIn('Unable to acquire session bus', text)
        self.assertIsNone(calculator.poll(), text)
        if owned:
            ping = self.in_box(
                box, ['dbus-send', '--session', '--dest=org.gnome.Calculator', '--print-reply',
                      '/org/gnome/Calculator', 'org.freedesktop.DBus.Peer.Ping'])
            self.assertEqual(ping.returncode, 0, ping.stderr + ping.stdout)
        else:
            self.assertGreaterEqual(time.monotonic() - started, 2.5, text)


@unittest.skipUnless(
    sys.platform.startswith('linux')
    and all(shutil.which(tool) for tool in ('Xvfb', 'openbox', 'x11vnc', 'bwrap', 'dbus-daemon')),
    'full isolated desktop fixture tools required',
)
class ServeFixtureTests(unittest.TestCase):
    def test_serve_starts_private_bus_then_expires_it(self):
        parent = Path(tempfile.mkdtemp(prefix='horizon-serve-dbus-'))
        self.addCleanup(lambda: shutil.rmtree(parent, ignore_errors=True))
        state = parent / 'state'
        serve = Path(__file__).resolve().parents[1] / 'serve.py'
        result = subprocess.run(
            [sys.executable, str(serve), '--horizon', '/bin/true', '--native-view',
             '--state', str(state)],
            text=True, capture_output=True, timeout=25)
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        manifest = json.loads(result.stdout)
        self.assertTrue(manifest['session_bus']['address'].startswith('unix:path=/run/user/'))
        self.assertEqual(
            manifest['session_bus']['apparmor']['sandbox_query_bind'],
            sandbox.APPARMOR_ACCESS.is_file(),
        )
        self.assertIsNone(manifest['viewer_url'])
        self.assertFalse((state / 'data').exists())
        self.assertFalse((state / 'target.json').exists())
        self.assertTrue((state / 'dbus.log').exists())
        self.assertTrue((state / 'xvfb.log').exists())


if __name__ == '__main__':
    unittest.main()
