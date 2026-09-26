"""The worker GPU lock serializes GPU work across sessions, using real flock."""
import contextlib
import os
import signal
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest

SCRIPTS = Path(__file__).parent


@unittest.skipUnless(shutil.which('flock'), 'the GPU lock needs util-linux flock')
class GpuLockTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.lock = self.root / 'workspace/locks/gpu.lock'
        self.script = self.root / 'horizon-worker-gpu-lock'
        source = (SCRIPTS / 'horizon-worker-gpu-lock').read_text()
        self.script.write_text(source.replace('/workspace', str(self.root / 'workspace')))

    def locked(self, *command, wait=None):
        options = ['--wait', str(wait)] if wait is not None else []
        return ['bash', str(self.script), *options, '--', *command]

    def hold(self, seconds):
        """Starts a holder that keeps the lock for `seconds` once it reports it has it."""
        held = self.root / 'held'
        holder = subprocess.Popen(self.locked('sh', '-c', f'touch {held}; sleep {seconds}'), start_new_session=True)
        self.addCleanup(self.stop, holder)
        deadline = time.monotonic() + 10
        while not held.exists():
            self.assertLess(time.monotonic(), deadline, 'holder never acquired the lock')
            time.sleep(0.02)
        return holder

    @staticmethod
    def stop(process):
        with contextlib.suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGKILL)
        process.wait()

    @staticmethod
    def kill(pid):
        with contextlib.suppress(ProcessLookupError):
            os.kill(pid, signal.SIGKILL)

    def test_runs_the_command_and_returns_its_status(self):
        result = subprocess.run(self.locked('sh', '-c', 'echo "$0"; exit 9', 'argument'),
                                capture_output=True, text=True, timeout=10)
        self.assertEqual((result.returncode, result.stdout), (9, 'argument\n'))
        self.assertTrue(self.lock.is_file())

    def test_a_second_command_waits_until_the_first_releases(self):
        holder = self.hold(1.5)
        started = time.monotonic()
        result = subprocess.run(self.locked('true'), timeout=20)
        self.assertEqual(result.returncode, 0)
        self.assertIsNotNone(holder.poll(), 'the second command ran while the first held the lock')
        self.assertGreater(time.monotonic() - started, 1)

    def test_a_bounded_wait_gives_up_with_75(self):
        holder = self.hold(5)
        marker = self.root / 'ran'
        for wait, least, most in [(0, 0, 1), (1, 0.9, 4)]:
            started = time.monotonic()
            result = subprocess.run(self.locked('touch', marker, wait=wait), timeout=20)
            elapsed = time.monotonic() - started
            self.assertEqual(result.returncode, 75, wait)
            self.assertTrue(least <= elapsed < most, (wait, elapsed))
            self.assertFalse(marker.exists())
        self.assertIsNone(holder.poll())

    def test_scripts_can_use_flock_on_the_same_file(self):
        self.hold(5)
        self.assertEqual(subprocess.run(['flock', '-n', str(self.lock), 'true']).returncode, 1)

    def test_a_background_process_left_by_the_command_does_not_keep_the_lock(self):
        pid_file = self.root / 'background.pid'
        subprocess.run(self.locked('sh', '-c', f'sleep 30 >/dev/null 2>&1 & echo $! > {pid_file}'), timeout=10, check=True)
        background = int(pid_file.read_text())
        self.addCleanup(self.kill, background)
        os.kill(background, 0)
        self.assertEqual(subprocess.run(self.locked('true', wait=0), timeout=10).returncode, 0)

    def test_usage_and_missing_commands(self):
        for arguments in [[], ['true'], ['--'], ['--wait'], ['--wait', '-1', '--', 'true'],
                          ['--wait', '1.5', '--', 'true'], ['--wait', 'soon', '--', 'true']]:
            result = subprocess.run(['bash', str(self.script), *arguments], capture_output=True, timeout=10)
            self.assertEqual(result.returncode, 2, arguments)
        for command in ['no-such-command-for-the-gpu-lock', 'cd']:
            result = subprocess.run(self.locked(command), capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 127, command)
            self.assertIn('command not found', result.stderr)
        plain = self.root / 'plain-file'
        plain.write_text('#!/bin/sh\n')
        for command in [plain, self.root]:
            result = subprocess.run(self.locked(command), capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 126, command)
            self.assertIn('not executable', result.stderr)
        self.assertFalse(self.lock.exists())


if __name__ == '__main__':
    unittest.main()
