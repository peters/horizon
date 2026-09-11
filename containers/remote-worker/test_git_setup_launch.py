"""Ordinary Git submission shares the established detached handoff, never overlay intake."""

import io
import json
import subprocess
import unittest
from unittest import mock

from test_setup_launch import MODULE, result

HANDOFF = MODULE.handoff


def observation(state='absent', reason=None):
    return {'version': 1, 'state': state, 'reason': reason,
            'checkout': '/workspace/horizon/repository' if state == 'complete' else None}


class GitSetupLaunchTests(unittest.TestCase):
    def setUp(self):
        self.runner = self.enterContext(mock.patch.object(MODULE.subprocess, 'run'))
        self.handoff = self.enterContext(mock.patch.object(MODULE, 'handoff', return_value=True))
        self.runner.return_value = result(observation())

    def request(self, value=b'synthetic request'):
        output, diagnostics = io.BytesIO(), io.StringIO()
        code = MODULE.run(io.BytesIO(value), output, diagnostics, True)
        return code, json.loads(output.getvalue()), diagnostics.getvalue()

    def test_absence_submits_only_fixed_git_operation(self):
        self.assertEqual(self.request(),
                         (0, {'version': 1, 'state': 'submitted', 'observation': None}, ''))
        self.handoff.assert_called_once_with(b'synthetic request', True)
        args, kwargs = self.runner.call_args
        self.assertEqual(args, ([MODULE.HELPER, 'git-status'],))
        self.assertEqual(kwargs['input'], b'synthetic request')
        self.assertEqual(kwargs['env'], MODULE.ENVIRONMENT)

    def test_existing_claim_success_and_failure_never_replay(self):
        for state, reason, code in [('claimed_unknown', None, 1), ('complete', None, 0),
                                   ('claimed_unknown', 'git', 1), ('error', 'invalid', 2),
                                   ('error', 'unsupported', 2), ('error', 'unsafe_root', 1),
                                   ('complete', 'unsafe_root', 1)]:
            with self.subTest(state=state, reason=reason):
                value = observation(state, reason)
                self.runner.return_value = result(value, code)
                self.assertEqual(self.request(),
                                 (code, {'version': 1, 'state': 'observed', 'observation': value}, ''))
        self.handoff.assert_not_called()

    def test_protocol_mismatch_never_submits_or_exposes_raw_output(self):
        invalid = [(dict(observation(), version=True), 0),
                   (dict(observation(), extra='private text'), 0),
                   (dict(observation(), state=[]), 0),
                   (dict(observation(), reason=[]), 0),
                   (observation('absent', 'git'), 1),
                   (observation('error'), 1),
                   (observation('claimed_unknown'), 0),
                   (dict(observation('complete'), checkout='/private/path'), 0),
                   (observation('error', 'private text'), 1),
                   (observation('complete'), 1)]
        for value, code in invalid:
            self.runner.return_value = result(value, code)
            actual, response, diagnostics = self.request()
            self.assertEqual((actual, response), (1, {'version': 1, 'state': 'error', 'observation': None}))
            self.assertNotIn('private', diagnostics)
        for raw in [b'private text\n', b' ' * 1024 + b'\n',
                    b'{"version":1,"version":1,"state":"absent","reason":null,"checkout":null}\n',
                    json.dumps(observation()).encode() + b'\n{}\n']:
            self.runner.return_value = subprocess.CompletedProcess([], 0, raw, b'')
            self.assertEqual(self.request()[0], 1)
        self.handoff.assert_not_called()

    def test_git_request_limit_is_narrower_than_overlay(self):
        self.assertEqual(self.request(b' ' * (16 * 1024 + 1))[0], 2)
        self.runner.assert_not_called()
        self.handoff.assert_not_called()

    def test_handoff_loss_stays_unknown_without_second_attempt(self):
        self.handoff.return_value = False
        self.assertEqual(self.request(),
                         (1, {'version': 1, 'state': 'handoff_unconfirmed', 'observation': None}, ''))
        self.handoff.assert_called_once_with(b'synthetic request', True)

    def test_git_handoff_detaches_fixed_child_without_waiting_or_killing(self):
        with mock.patch.object(MODULE.subprocess, 'Popen') as spawn, \
                mock.patch.object(MODULE.os, 'set_blocking'), \
                mock.patch.object(MODULE.os, 'write', return_value=5), \
                mock.patch.object(MODULE.selectors, 'DefaultSelector') as select:
            process = spawn.return_value
            process.stdin.fileno.return_value = 29
            select.return_value.__enter__.return_value.select.return_value = [(29, 2)]
            self.assertTrue(HANDOFF(b'hello', True))
            args, kwargs = spawn.call_args
            self.assertEqual(args, ([MODULE.HELPER, 'git-prepare'],))
            self.assertTrue(kwargs['start_new_session'] and kwargs['close_fds'])
            self.assertEqual(kwargs['umask'], 0o077)
            self.assertEqual((kwargs['stdout'], kwargs['stderr']),
                             (subprocess.DEVNULL, subprocess.DEVNULL))
            for method in ('wait', 'kill', 'terminate', 'communicate'):
                getattr(process, method).assert_not_called()
            process.poll.assert_called_once_with()
            process.stdin.close.assert_called_once_with()

    def test_main_rejects_other_switch_shapes(self):
        for arguments in [['--git', 'extra'], ['--git=true'], ['--git', '--git']]:
            with mock.patch.object(MODULE.sys, 'argv', ['launcher', *arguments]), \
                    mock.patch.object(MODULE.os, 'write'), mock.patch.object(MODULE, 'run') as run:
                self.assertEqual(MODULE.main(), 2)
                run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
