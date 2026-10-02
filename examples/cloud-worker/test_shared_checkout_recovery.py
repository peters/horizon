"""A shared checkout preparation outlives its SSH client, and a refused attach binds nothing."""
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time
import unittest

import test_session_relaunch as fixtures

# Generous upper bounds: a loaded machine can be slow, and a bound only costs time when a test fails.
BOUND = 120

# Records each call, then holds the preparation open until the test releases it.
BLOCKING_SOURCE = '''import os, sys, time
from pathlib import Path
with open(os.environ['SOURCE_LOG'], 'a') as log:
    log.write(' '.join(sys.argv[1:]) + '\\n')
Path(os.environ['SOURCE_PID']).write_text(str(os.getpid()))
release = Path(os.environ['SOURCE_RELEASE'])
deadline = time.monotonic() + {bound}
while not release.exists() and time.monotonic() < deadline:
    time.sleep(0.05)
sys.exit(0 if release.exists() else 9)
'''.replace('{bound}', str(BOUND))


def wait_for(condition, timeout=BOUND):
    deadline = time.monotonic() + timeout
    while not condition():
        if time.monotonic() > deadline:
            raise AssertionError('Timed out waiting for the preparation')
        time.sleep(0.05)


@unittest.skipUnless(shutil.which('flock') and shutil.which('git') and shutil.which('setsid'),
                     'worker scripts need util-linux flock and setsid, and Git')
class SharedCheckoutRecoveryTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.SessionRelaunchTests(methodName='runTest')
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        f = self.fixture
        self.checkout = f.workspace / 'checkout'
        self.state = f.workspace / 'shared-checkout-state'
        self.release = f.root / 'release'
        self.source_pid = f.root / 'source.pid'
        f.env.update(SOURCE_RELEASE=str(self.release), SOURCE_PID=str(self.source_pid))
        source = f.tools / 'horizon-worker-source'
        source.write_text('#!' + sys.executable + '\n' + BLOCKING_SOURCE)
        # Never leave a held preparation behind a failed assertion.
        self.addCleanup(self.release.touch)

    def command(self, identity, revision=None):
        return ['bash', str(self.fixture.script), '--shared', identity, 'shell', revision or self.fixture.revision]

    def start(self, identity, revision=None):
        return subprocess.run(self.command(identity, revision), env=self.fixture.env,
                              capture_output=True, text=True, timeout=BOUND)

    def begin(self, identity):
        # Its own process group stands in for the SSH session that a disconnect hangs up.
        return subprocess.Popen(self.command(identity), env=self.fixture.env, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, text=True, start_new_session=True)

    def begin_waiting(self, identity):
        progress = self.fixture.root / f'{identity}.err'
        with progress.open('w') as error:
            waiting = subprocess.Popen(self.command(identity), env=self.fixture.env,
                                       stdout=subprocess.DEVNULL, stderr=error)
        wait_for(lambda: 'Waiting for another panel' in progress.read_text())
        return waiting, progress

    def source_calls(self):
        log = self.fixture.root / 'source.log'
        return log.read_text().splitlines() if log.exists() else []

    def assert_no_session_state(self, identity):
        sessions = self.fixture.workspace / 'sessions'
        self.assertFalse((sessions / identity).exists())
        self.assertEqual([p.name for p in sessions.iterdir() if p.name.startswith('.preparing-')], [])

    def test_disconnect_mid_preparation_lets_it_finish_for_the_next_attach(self):
        f = self.fixture
        client = self.begin('one')
        wait_for(self.source_pid.exists)
        os.killpg(client.pid, signal.SIGHUP)
        client.communicate(timeout=BOUND)
        self.assertNotEqual(client.returncode, 0)
        self.assertFalse((self.state / 'ready').exists())
        waiting, progress = self.begin_waiting('two')
        self.release.touch()
        waiting.wait(timeout=BOUND)
        self.assertEqual(waiting.returncode, 0, progress.read_text())
        self.assertTrue((self.state / 'ready').exists())
        self.assertFalse((self.state / 'failed').exists())
        reattached = self.start('one')
        self.assertEqual(reattached.returncode, 0, reattached.stderr)
        self.assertEqual(len(self.source_calls()), 1)
        self.assertEqual([entry['cwd'] for entry in f.launches()], [str(self.checkout)] * 2)
        self.assertEqual(f.git('-C', self.checkout, 'rev-parse', 'HEAD'), f.revision)

    def test_concurrent_attach_waits_for_the_preparation_and_both_launch_once(self):
        f = self.fixture
        first = self.begin('one')
        wait_for(self.source_pid.exists)
        waiting, progress = self.begin_waiting('two')
        self.assertIsNone(first.poll())
        self.assertEqual(f.launches(), [])
        self.release.touch()
        _, error = first.communicate(timeout=BOUND)
        self.assertEqual(first.returncode, 0, error)
        waiting.wait(timeout=BOUND)
        self.assertEqual(waiting.returncode, 0, progress.read_text())
        self.assertEqual(len(self.source_calls()), 1)
        self.assertEqual(sorted(entry['session'] for entry in f.launches()), ['one', 'two'])

    def test_killed_preparation_resumes_on_the_next_attach(self):
        f = self.fixture
        client = self.begin('one')
        wait_for(self.source_pid.exists)
        # A container restart kills the preparation without recording a failure.
        os.killpg(os.getpgid(int(self.source_pid.read_text())), signal.SIGKILL)
        _, error = client.communicate(timeout=BOUND)
        self.assertEqual(client.returncode, 3, error)
        self.assertIn('Attach again to resume', error)
        self.assert_no_session_state('one')
        self.assertFalse((self.state / 'failed').exists())
        self.release.touch()
        resumed = self.start('one')
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertIn('Resuming', resumed.stderr)
        self.assertEqual(len(self.source_calls()), 2)
        self.assertEqual([entry['cwd'] for entry in f.launches()], [str(self.checkout)])
        self.assertEqual((self.checkout / 'file.txt').read_text(), 'committed\n')

    def test_failed_preparation_resumes_after_the_retry_command(self):
        f = self.fixture
        (f.tools / 'horizon-worker-source').write_text('#!/bin/sh\necho source unavailable >&2\nexit 9\n')
        failed = self.start('one')
        self.assertEqual(failed.returncode, 3)
        self.assertIn('source unavailable', failed.stderr)
        self.assertIn('source unavailable', (self.state / 'prepare.log').read_text())
        self.assert_no_session_state('one')
        f.install('horizon-worker-source')
        (f.workspace / 'source').mkdir()
        (f.workspace / 'source/manifest.json').write_text('{"modules":[],"assets":[]}')
        fenced = self.start('one')
        self.assertEqual(fenced.returncode, 3)
        self.assertIn("run 'horizon-worker-session --retry-shared-checkout'", fenced.stderr)
        self.assert_no_session_state('one')
        retry = subprocess.run(['bash', str(f.script), '--retry-shared-checkout'], env=f.env,
                               capture_output=True, text=True, timeout=BOUND)
        self.assertEqual(retry.returncode, 0, retry.stderr)
        self.assertIn('Cleared', retry.stdout)
        self.assertFalse((self.state / 'failed').exists())
        resumed = self.start('one')
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertEqual([entry['cwd'] for entry in f.launches()], [str(self.checkout)])

    def sentinel_df(self):
        """A df that records being run: the storage message must never call it."""
        record = self.fixture.root / 'df-was-run'
        stub = self.fixture.tools / 'df'
        stub.write_text(f"#!/bin/sh\necho ran >> '{record}'\necho Filesystem\n")
        stub.chmod(0o700)
        return record

    def refuse_flush(self, pattern, allow=0):
        """A `sync` that fails with the real mount's message for paths matching `pattern`, until
        lifted, after `allow` calls for them have succeeded."""
        refusing = self.fixture.root / 'sync-fails'
        refusing.touch()
        calls = self.fixture.root / 'sync-calls'
        stub = self.fixture.tools / 'sync'
        stub.write_text(f"#!/bin/sh\nif [ -e '{refusing}' ]; then\n  case \"$2\" in {pattern})\n"
                        f"    echo x >> '{calls}'\n    if [ \"$(wc -l < '{calls}')\" -gt {allow} ]; then\n"
                        "      echo \"sync: error syncing '$2': Disk quota exceeded\" >&2; exit 1\n    fi;;\n  esac\nfi\n"
                        f"exec {shutil.which('sync')} \"$@\"\n")
        stub.chmod(0o700)
        return refusing

    def prepare_only(self):
        return subprocess.run(['bash', str(self.fixture.script), '--shared', '--prepare-only', 'prepare', 'shell',
                               self.fixture.revision], env=self.fixture.env, capture_output=True, text=True,
                              timeout=BOUND)

    def test_prepare_only_prepares_the_checkout_once_and_binds_no_session(self):
        f = self.fixture
        self.release.touch()
        prepared = self.prepare_only()
        self.assertEqual(prepared.returncode, 0, prepared.stderr)
        self.assertTrue((self.state / 'ready').exists())
        self.assertEqual(f.git('-C', self.checkout, 'rev-parse', 'HEAD'), f.revision)
        self.assertEqual(f.launches(), [], 'no process starts')
        self.assert_no_session_state('prepare')
        self.assertFalse((f.workspace / 'sessions' / 'prepare').exists())
        again = self.prepare_only()
        self.assertEqual(again.returncode, 0, again.stderr)
        first_panel = self.start('one')
        self.assertEqual(first_panel.returncode, 0, first_panel.stderr)
        self.assertEqual(len(self.source_calls()), 1, 'the checkout was prepared once, not by the panel')
        self.assertEqual([entry['cwd'] for entry in f.launches()], [str(self.checkout)])

    def test_prepare_only_drops_what_an_interrupted_run_left_behind(self):
        f = self.fixture
        self.release.touch()
        stale = f.workspace / 'sessions' / 'prepare'
        stale.mkdir(parents=True)
        (stale / 'preparing').touch()
        (stale / 'attach-owner').write_text('.preparing-gone\n')
        prepared = self.prepare_only()
        self.assertEqual(prepared.returncode, 0, prepared.stderr)
        self.assertFalse(stale.exists(), 'a retry leaves no stale preparation state')
        self.assertEqual(f.launches(), [])

    def test_prepare_only_refuses_an_id_a_session_already_uses_and_never_attaches(self):
        f = self.fixture
        self.release.touch()
        session = f.workspace / 'sessions' / 'prepare'
        session.mkdir(parents=True)
        (session / 'launch-requested').touch()
        refused = self.prepare_only()
        self.assertEqual(refused.returncode, 5, refused.stderr)
        self.assertIn('reserved for preparing the checkout', refused.stderr)
        self.assertTrue(session.exists(), 'a session that is in use is left untouched')
        self.assertEqual(f.launches(), [])

    def test_prepare_only_reports_a_failed_preparation_like_an_attach(self):
        f = self.fixture
        (f.tools / 'horizon-worker-source').write_text('#!/bin/sh\necho source unavailable >&2\nexit 9\n')
        failed = self.prepare_only()
        self.assertEqual(failed.returncode, 3)
        self.assertIn('source unavailable', failed.stderr)
        self.assert_no_session_state('prepare')
        self.assertEqual(f.launches(), [])

    def test_prepare_only_proves_the_volume_takes_a_write_before_any_checkout_work(self):
        f = self.fixture
        self.release.touch()
        # Neither an existing file of the old fixed name nor a link there may be touched.
        victim = f.root / 'victim'
        victim.write_text('keep me')
        (f.workspace / '.horizon-write-probe').symlink_to(victim)
        refusing = self.refuse_flush('*/.horizon-write-probe.*')
        refused = self.prepare_only()
        self.assertEqual(refused.returncode, 74, refused.stderr)
        self.assertIn('Workspace storage did not accept a write', refused.stderr)
        self.assertEqual(victim.read_text(), 'keep me')
        self.assertEqual([p.name for p in f.workspace.glob('.horizon-write-probe.*')], [], 'no probe file is left')
        self.assertFalse(self.state.exists(), 'nothing of the checkout was started')
        self.assertEqual(self.source_calls(), [])
        self.assertEqual(f.launches(), [])
        refusing.unlink()
        prepared = self.prepare_only()
        self.assertEqual(prepared.returncode, 0, prepared.stderr)
        self.assertEqual([p.name for p in f.workspace.glob('.horizon-write-probe.*')], [])
        self.assertTrue((self.state / 'ready').exists())

    def test_an_interrupted_probe_leaves_no_file_behind(self):
        f = self.fixture
        self.release.touch()
        # sync never returns, as when a flush hangs on a stalled volume; the run is then terminated.
        stub = f.tools / 'sync'
        stub.write_text('#!/bin/sh\nsleep 30\n')
        stub.chmod(0o700)
        running = subprocess.Popen(['bash', str(f.script), '--shared', '--prepare-only', 'prepare', 'shell', f.revision],
                                   env=f.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        wait_for(lambda: list(f.workspace.glob('.horizon-write-probe.*')))
        running.terminate()
        running.communicate(timeout=BOUND)
        self.assertEqual([p.name for p in f.workspace.glob('.horizon-write-probe.*')], [], 'the exit trap removed it')

    def test_a_refused_flush_says_why_and_the_next_attach_resumes(self):
        f = self.fixture
        df_run = self.sentinel_df()
        refusing = self.refuse_flush('*')
        refused = self.start('one')
        self.assertEqual(refused.returncode, 74, refused.stderr)
        self.assertFalse(df_run.exists(), 'df describes shared storage on a network volume, so it is not run')
        self.assertIn('Workspace storage did not accept a write', refused.stderr)
        self.assertIn('Disk quota exceeded', refused.stderr)
        self.assertIn('Try again', refused.stderr)
        self.assertRegex(refused.stderr, r'Used on \S*workspace: \S+')
        self.assertEqual(f.launches(), [])
        self.assert_no_session_state('one')
        self.assertFalse((self.state / 'failed').exists(), 'a storage refusal is not a failed preparation')
        refusing.unlink()
        self.release.touch()
        resumed = self.start('one')
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertIn('Resuming', resumed.stderr)
        self.assertEqual([entry['cwd'] for entry in f.launches()], [str(self.checkout)])

    def test_an_undurable_ready_marker_is_withdrawn_before_any_session_can_enter(self):
        f = self.fixture
        self.release.touch()
        refusing = self.refuse_flush('*/ready')
        refused = self.start('one')
        self.assertEqual(refused.returncode, 74, refused.stderr)
        self.assertIn('Workspace storage did not accept a write', refused.stderr)
        self.assertFalse((self.state / 'ready').exists(), 'a marker that may vanish must not admit a session')
        self.assertFalse((self.state / 'failed').exists())
        self.assertEqual(f.launches(), [])
        refusing.unlink()
        resumed = self.start('one')
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertIn('Resuming', resumed.stderr)
        self.assertEqual([entry['cwd'] for entry in f.launches()], [str(self.checkout)])

    def test_a_refusal_in_the_detached_preparation_ends_the_attach_with_the_storage_status(self):
        f = self.fixture
        self.release.touch()
        refusing = self.refuse_flush('*/checkout')
        refused = self.start('one')
        self.assertEqual(refused.returncode, 74, refused.stderr)
        self.assertIn('Workspace storage did not accept a write', refused.stderr)
        self.assertFalse((self.state / 'ready').exists())
        self.assertFalse((self.state / 'failed').exists(), 'attaching again must resume, not need a retry command')
        self.assertEqual(f.launches(), [])
        refusing.unlink()
        resumed = self.start('one')
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertIn('Resuming', resumed.stderr)
        self.assertEqual([entry['cwd'] for entry in f.launches()], [str(self.checkout)])

    def test_a_refusal_while_recording_a_failed_preparation_still_says_it_was_storage(self):
        f = self.fixture
        (f.tools / 'horizon-worker-source').write_text('#!/bin/sh\necho source unavailable >&2\nexit 9\n')
        # The foreground flush after creating the state directory passes; recording the failure is refused.
        refusing = self.refuse_flush('*/shared-checkout-state', allow=1)
        refused = self.start('one')
        self.assertEqual(refused.returncode, 74, refused.stderr)
        self.assertIn('Workspace storage did not accept a write', refused.stderr)
        self.assertFalse((self.state / 'failed').exists(), 'an unconfirmed failure record is withdrawn')
        self.assertEqual(f.launches(), [])
        refusing.unlink()

    def test_a_refused_launch_fence_is_withdrawn_so_the_next_attach_is_not_an_uncertain_launch(self):
        f = self.fixture
        self.release.touch()
        refusing = self.refuse_flush('*/launch-requested')
        refused = self.start('one')
        self.assertEqual(refused.returncode, 74, refused.stderr)
        self.assertIn('launch-requested', refused.stderr)
        self.assertEqual(f.launches(), [])
        session = f.workspace / 'sessions' / 'one'
        self.assertFalse((session / 'launch-requested').exists())
        refusing.unlink()
        retried = self.start('one')
        self.assertEqual(retried.returncode, 0, retried.stderr)
        self.assertNotIn('launch is uncertain', retried.stderr)
        self.assertEqual([entry['cwd'] for entry in f.launches()], [str(self.checkout)])

    def test_a_fatal_git_status_128_is_a_recorded_failure(self):
        f = self.fixture
        (f.tools / 'horizon-worker-source').write_text('#!/bin/sh\necho fatal: bad object >&2\nexit 128\n')
        failed = self.start('one')
        self.assertEqual(failed.returncode, 3)
        self.assertIn('preparation failed', failed.stderr)
        self.assertTrue((self.state / 'failed').exists())
        fenced = self.start('one')
        self.assertEqual(fenced.returncode, 3)
        self.assertIn('--retry-shared-checkout', fenced.stderr)
        self.assertEqual(f.launches(), [])

    def test_a_hangup_right_after_publishing_the_binding_rolls_it_back(self):
        f = self.fixture
        self.release.touch()
        # The rename completes, then the SSH client hangs up before the next command.
        mv = f.tools / 'mv'
        mv.write_text(f'#!/bin/bash\n{shutil.which("mv")} "$@" || exit\n'
                      'case "$1" in */.preparing-*) kill -HUP "$PPID";; esac\n')
        mv.chmod(0o700)
        interrupted = self.start('one')
        self.assertEqual(interrupted.returncode, -signal.SIGHUP, interrupted.stderr)
        self.assert_no_session_state('one')
        mv.unlink()
        retried = self.start('one')
        self.assertEqual(retried.returncode, 0, retried.stderr)
        self.assertEqual(len(f.launches()), 1)

    def test_a_failure_recorded_after_ready_still_fences_attaches(self):
        f = self.fixture
        self.release.touch()
        self.assertEqual(self.start('one').returncode, 0)
        # The preparation wrote `ready`, then failed its final sync.
        (self.state / 'failed').touch()
        refused = self.start('two')
        self.assertEqual(refused.returncode, 3)
        self.assertIn('preparation failed', refused.stderr)
        self.assert_no_session_state('two')
        self.assertEqual(len(f.launches()), 1)

    def test_refused_attach_leaves_no_binding_for_its_session_id(self):
        f = self.fixture
        self.release.touch()
        self.assertEqual(self.start('one').returncode, 0)
        refused = self.start('retry', revision='b' * 40)
        self.assertEqual(refused.returncode, 3)
        self.assertIn('Shared checkout binding mismatch', refused.stderr)
        self.assert_no_session_state('retry')
        retried = self.start('retry')
        self.assertEqual(retried.returncode, 0, retried.stderr)
        self.assertEqual(len(f.launches()), 2)
        self.assertEqual(len(self.source_calls()), 1)


    def test_refused_attach_never_removes_an_existing_session(self):
        f = self.fixture
        self.release.touch()
        self.assertEqual(self.start('one').returncode, 0)
        session = f.workspace / 'sessions/one'
        before = sorted(p.name for p in session.iterdir())
        (self.checkout / 'file.txt').write_text('agent edit\n')
        refused = self.start('one', revision='b' * 40)
        self.assertEqual(refused.returncode, 3)
        self.assertIn('Session binding mismatch', refused.stderr)
        self.assertEqual(sorted(p.name for p in session.iterdir()), before)
        self.assertTrue((f.workspace / 'session-data/one').is_dir())
        self.assertEqual((self.checkout / 'file.txt').read_text(), 'agent edit\n')
        reattached = self.start('one')
        self.assertEqual(reattached.returncode, 0, reattached.stderr)
        self.assertEqual(len(f.launches()), 1)


if __name__ == '__main__':
    unittest.main()
