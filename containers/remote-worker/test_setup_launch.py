"""Launcher protocol, mocked handoff faults, and response-only interpreter probes."""

import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location('setup_launch', Path(__file__).with_name('setup-launch.py'))
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def observation(status='absent', **changes):
    return dict({'version': 1, 'status': status, 'recording': 'not_acknowledged',
                 'reason': None, 'execution': None}, **changes)


def result(value, code=0):
    return subprocess.CompletedProcess([], code, json.dumps(value).encode()+b'\n', b'')


class SetupLaunchTests(unittest.TestCase):
    def setUp(self):
        self.runner = self.enterContext(mock.patch.object(MODULE.subprocess, 'run'))
        self.spawn = self.enterContext(mock.patch.object(MODULE.subprocess, 'Popen'))
        self.runner.return_value = result(observation())

    def run_request(self, request=b'private request', output=None, diagnostics=None):
        if output is None:
            output = io.BytesIO()
        if diagnostics is None:
            diagnostics = io.StringIO()
        code = MODULE.run(io.BytesIO(request), output, diagnostics)
        return code, json.loads(output.getvalue())

    def test_observation_preserves_existing_results_without_launch(self):
        cases = [('claimed_unknown', 4), ('error', 1), ('rejected', 2)]
        for status, expected in cases:
            value = observation(status, reason='static reason' if expected in (1, 2) else None)
            self.runner.return_value = result(value, expected)
            code, response = self.run_request()
            self.assertEqual((code, response), (expected, {'version': 1, 'state': 'observed', 'observation': value}))
        for state, expected in [('published', 0), ('rejected', 2), ('unpublished', 1),
                                ('published_unsynchronized', 1), ('rename_unconfirmed', 1)]:
            value = observation('completed', recording='observed', execution={'state': state})
            self.runner.return_value = result(value, expected)
            self.assertEqual(self.run_request(), (expected, {'version': 1, 'state': 'observed', 'observation': value}))
        self.spawn.assert_not_called()
        args, kwargs = self.runner.call_args
        self.assertEqual(args, ([MODULE.HELPER, 'setup-status'],))
        self.assertEqual(kwargs['input'], b'private request')
        self.assertEqual(kwargs['env'], {'PATH': '/usr/bin:/bin', 'LC_ALL': 'C'})
        self.assertTrue(kwargs['close_fds'] and kwargs['capture_output'])
        self.assertEqual((kwargs['cwd'], kwargs['timeout']), ('/', 30))

    def test_only_actual_absence_can_submit_and_handoff_is_not_completion(self):
        with mock.patch.object(MODULE, 'handoff') as handoff:
            for complete, state, expected in [(True, 'submitted', 0), (False, 'handoff_unconfirmed', 1)]:
                handoff.return_value = complete
                self.assertEqual(self.run_request(), (expected, {'version': 1, 'state': state, 'observation': None}))
                handoff.assert_called_with(b'private request')
            handoff.side_effect = MODULE.LaunchError('static failure')
            diagnostics = io.StringIO()
            self.assertEqual(self.run_request(diagnostics=diagnostics),
                             (1, {'version': 1, 'state': 'error', 'observation': None}))
            self.assertEqual(diagnostics.getvalue(), 'static failure\n')
        self.spawn.assert_not_called()

    def test_oversized_and_failed_reads_never_observe_or_launch(self):
        self.assertEqual(self.run_request(b' '*(MODULE.REQUEST_LIMIT+1)),
                         (2, {'version': 1, 'state': 'rejected', 'observation': None}))
        stream = mock.Mock()
        stream.read.side_effect = OSError('private path')
        self.assertEqual(MODULE.execute(stream), ('rejected', None, 2))
        stream.read.assert_called_once_with(MODULE.REQUEST_LIMIT+1)
        self.runner.assert_not_called()
        self.spawn.assert_not_called()

    def test_invalid_observations_fail_closed_and_redact_output(self):
        absent = json.dumps(observation()).encode()
        invalid = [b'private output', b'\xff\n', b'{}\n', b'[]\n',
                   b'{"version":1,'+absent[1:]+b'\n', b' '*MODULE.OBSERVATION_LIMIT+b'\n',
                   absent+b'\n{}\n']
        for raw in invalid:
            with self.subTest(raw=raw[:40]):
                self.runner.return_value = subprocess.CompletedProcess([], 0, raw, b'')
                diagnostics = io.StringIO()
                self.assertEqual(self.run_request(diagnostics=diagnostics)[0], 1)
                self.assertNotIn('private', diagnostics.getvalue())
        for value, code in [(observation(version=True), 0), (observation(version=2), 0),
                            (observation(extra=True), 0), (observation(recording='acknowledged'), 0),
                            (observation(execution={}), 0), (observation(reason='invalid'), 0),
                            (observation(), 1), (observation('unknown'), 0),
                            (observation('claimed_unknown'), 0),
                            (observation('completed', recording='observed', execution={'state': 'future'}), 0),
                            (observation('completed', recording='observed', execution={'state': []}), 0),
                            (observation('completed', recording='observed', execution={'state': 'published'}), 1)]:
            self.runner.return_value = result(value, code)
            self.assertEqual(self.run_request()[0], 1)
        for code, stderr in [(3, b''), (-9, b''), (0, b'private output')]:
            self.runner.return_value = result(observation(), code)
            self.runner.return_value.stderr = stderr
            self.assertEqual(self.run_request()[0], 1)
        self.spawn.assert_not_called()

    def test_observer_timeout_and_spawn_errors_are_distinct_from_handoff(self):
        for error in (OSError('private path'), subprocess.TimeoutExpired('private argv', 30)):
            self.runner.side_effect = error
            self.assertEqual(self.run_request(), (1, {'version': 1, 'state': 'error', 'observation': None}))
        self.spawn.assert_not_called()
        self.runner.side_effect = None
        self.spawn.side_effect = OSError('private path')
        diagnostics = io.StringIO()
        self.assertEqual(self.run_request(diagnostics=diagnostics)[0], 1)
        self.assertEqual(diagnostics.getvalue(), 'independent setup could not be spawned\n')

    def handoff_mocks(self):
        process = self.spawn.return_value
        process.stdin.fileno.return_value = 29
        blocking = self.enterContext(mock.patch.object(MODULE.os, 'set_blocking'))
        write = self.enterContext(mock.patch.object(MODULE.os, 'write'))
        select = self.enterContext(mock.patch.object(MODULE.selectors, 'DefaultSelector'))
        ready = select.return_value.__enter__.return_value
        ready.select.return_value = [(29, MODULE.selectors.EVENT_WRITE)]
        return process, blocking, write, ready

    def assert_not_terminated(self, process):
        for name in ('kill', 'terminate', 'wait', 'communicate', '__enter__'):
            getattr(process, name).assert_not_called()
        process.poll.assert_called_once_with()
        process.stdin.close.assert_called_once_with()

    def test_detached_handoff_handles_partial_writes_and_would_block(self):
        process, blocking, write, ready = self.handoff_mocks()
        write.side_effect = [BlockingIOError(), 2, 3]
        self.assertTrue(MODULE.handoff(b'hello'))
        self.assertEqual([bytes(call.args[1]) for call in write.call_args_list], [b'hello', b'hello', b'llo'])
        blocking.assert_called_once_with(29, False)
        ready.register.assert_called_once_with(29, MODULE.selectors.EVENT_WRITE)
        args, kwargs = self.spawn.call_args
        self.assertEqual(args, ([MODULE.HELPER, 'setup'],))
        self.assertEqual(kwargs, {'stdin': subprocess.PIPE, 'stdout': subprocess.DEVNULL,
                                 'stderr': subprocess.DEVNULL, 'bufsize': 0, 'close_fds': True,
                                 'start_new_session': True, 'cwd': '/', 'env': MODULE.ENVIRONMENT,
                                 'umask': 0o077})
        self.assert_not_terminated(process)

    def test_partial_handoff_failure_or_deadline_never_kills_or_retries(self):
        process, _, write, ready = self.handoff_mocks()
        for fault in ('broken', 'zero', 'selector', 'deadline', 'close'):
            with self.subTest(fault=fault):
                process.reset_mock()
                self.spawn.reset_mock()
                process.stdin.close.side_effect = OSError() if fault == 'close' else None
                write.side_effect = [2, BrokenPipeError()] if fault == 'broken' else None
                write.return_value = 0 if fault == 'zero' else 5
                ready.select.return_value = [] if fault == 'selector' else [(29, 2)]
                with mock.patch.object(MODULE.time, 'monotonic', side_effect=[0, 16] if fault == 'deadline' else None,
                                       return_value=0):
                    self.assertFalse(MODULE.handoff(b'hello'))
                self.spawn.assert_called_once()
                self.assert_not_terminated(process)

    def test_output_failure_after_submission_does_not_undo_handoff(self):
        for failure in ('short', 'write', 'flush'):
            with mock.patch.object(MODULE, 'handoff', return_value=True) as handoff:
                output = mock.Mock()
                output.write.return_value = 0
                if failure == 'write':
                    output.write.side_effect = BrokenPipeError()
                if failure == 'flush':
                    output.write.side_effect = len
                    output.flush.side_effect = OSError()
                self.assertEqual(MODULE.run(io.BytesIO(b'private request'), output, io.StringIO()), 3)
                handoff.assert_called_once_with(b'private request')
        self.spawn.assert_not_called()

    def test_maximum_observation_fits_wrapped_response_bound(self):
        value = observation('completed', recording='observed', execution={'state': 'rename_unconfirmed',
            'source_metadata': '/'+('\x01'*4095), 'checkout': '/'+('\x02'*4095),
            'possible_destination': '/'+('\x03'*4095)})
        response = result(value, 1)
        self.assertGreater(len(response.stdout), 64*1024)
        self.assertLess(len(response.stdout), MODULE.OBSERVATION_LIMIT)
        self.runner.return_value = response
        output = io.BytesIO()
        self.assertEqual(self.run_request(output=output)[0], 1)
        self.assertLess(len(output.getvalue()), MODULE.RESPONSE_LIMIT)
        self.assertTrue(output.getvalue().endswith(b'\n'))

    def test_bad_arguments_do_not_read_stdin_even_when_usage_output_fails(self):
        with mock.patch.object(MODULE.sys, 'argv', ['launcher', 'unexpected']), \
                mock.patch.object(MODULE.os, 'write', side_effect=OSError()), \
                mock.patch.object(MODULE, 'run') as run:
            self.assertEqual(MODULE.main(), 2)
            run.assert_not_called()
        self.runner.assert_not_called()
        self.spawn.assert_not_called()


class MainProcessTests(unittest.TestCase):
    PROGRAM = '''
import importlib.util
import sys
spec = importlib.util.spec_from_file_location('launcher', sys.argv[1])
launcher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(launcher)
mode = sys.argv[2]
sys.argv = ['launcher', *sys.argv[3:]]
def unexpected(*args, **kwargs):
    raise AssertionError('response-only probe must not observe or launch a worker')
launcher.observe = launcher.handoff = unexpected
if mode == 'submitted':
    launcher.execute = lambda stream, git=False: ('submitted', None, 0)
elif mode == 'error':
    def failure(stream, git=False):
        raise launcher.LaunchError('static failure')
    launcher.execute = failure
sys.exit(launcher.main())
'''

    def probe(self, redirect, mode='unchanged', *args):
        return subprocess.run(['/bin/sh', '-c', redirect+'; exec "$@"', 'launcher-probe',
            sys.executable, '-I', '-B', '-c', self.PROGRAM, str(SPEC.origin), mode, *args],
            input=b'', capture_output=True, timeout=10, check=False)

    def test_closed_stdin_rejects_without_observing_or_launching(self):
        result = self.probe('exec 0<&-')
        self.assertEqual((result.returncode, result.stderr), (2, b''))
        self.assertEqual(json.loads(result.stdout), {'version': 1, 'state': 'rejected', 'observation': None})

    def test_closed_stdout_reports_output_loss_without_observing_or_launching(self):
        result = self.probe('exec 1>&-')
        self.assertEqual((result.returncode, result.stdout, result.stderr), (3, b'', b''))

    def test_closed_stderr_does_not_break_usage_or_static_error_handling(self):
        usage = self.probe('exec 2>&-', 'unchanged', 'unexpected')
        self.assertEqual((usage.returncode, usage.stdout, usage.stderr), (2, b'', b''))
        error = self.probe('exec 2>&-', 'error')
        self.assertEqual((error.returncode, error.stderr), (1, b''))
        self.assertEqual(json.loads(error.stdout), {'version': 1, 'state': 'error', 'observation': None})

    def test_failed_stdout_does_not_retry_buffered_output_at_shutdown(self):
        result = self.probe('exec 1>/dev/full', 'submitted')
        self.assertEqual((result.returncode, result.stdout, result.stderr), (3, b'', b''))
        diagnostics = self.probe('exec 2>/dev/full', 'error')
        self.assertEqual((diagnostics.returncode, diagnostics.stderr), (1, b''))
        self.assertEqual(json.loads(diagnostics.stdout)['state'], 'error')

    def test_git_entrypoint_preserves_closed_stream_and_lost_output_behavior(self):
        closed = self.probe('exec 0<&-', 'unchanged', '--git')
        self.assertEqual((closed.returncode, closed.stderr), (2, b''))
        self.assertEqual(json.loads(closed.stdout)['state'], 'rejected')
        lost = self.probe('exec 1>/dev/full', 'submitted', '--git')
        self.assertEqual((lost.returncode, lost.stdout, lost.stderr), (3, b'', b''))


if __name__ == '__main__':
    unittest.main()
