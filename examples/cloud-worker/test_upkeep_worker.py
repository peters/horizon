"""The Upkeep exchange writes only as the owner of the agent's home and never follows a link
the agent planted; the loop keeps invalid names out of commands and gives the prompt on stdin."""
import base64
import contextlib
import io
import json
import os
from pathlib import Path
import runpy
import stat
import sys
import tempfile
import time
import types
import unittest
from unittest import mock
import zlib

WORKER = Path(__file__).resolve().parents[2] / 'crates/horizon-core/src/upkeep/worker'
EXCHANGE = runpy.run_path(str(WORKER / 'exchange.py'))
RUN = runpy.run_path(str(WORKER / 'run.py'))
SESSION = 'upkeep-45ee423d-7fb2'


def payload(**changes):
    value = {'session': SESSION, 'revision': 'a' * 40, 'config': {'repositories': ['acme/web']},
             'instructions': 'Keep moving.', 'prompt': ' Hourly.', 'daily_prompt': ' Daily.',
             'run': '#!/bin/bash\n', 'scheduler': 'print()\n',
             'run_now': False, 'paused': None}
    value.update(changes)
    return value


class Worker:
    """A workspace in a temporary folder, with tmux replaced."""

    def __init__(self, root):
        self.root = Path(root)
        self.home = self.root / 'home'
        self.sessions = self.root / 'sessions'
        self.data = self.root / 'session-data'
        for path in (self.home, self.sessions, self.data):
            path.mkdir(mode=0o700)
        self.outside = self.root / 'outside'
        self.outside.write_text('untouched')
        self.calls = []

    def tmux(self, *args):
        self.calls.append(args)
        # No session yet, so the exchange starts one.
        code = 1 if args[0] == 'display-message' else 0
        return types.SimpleNamespace(returncode=code, stdout='', stderr='')

    def exchange(self, value=None):
        names = EXCHANGE['exchange'].__globals__
        with mock.patch.dict(names, HOME=str(self.home), SESSIONS=str(self.sessions),
                             SESSION_DATA=str(self.data), tmux=self.tmux):
            return json.loads(EXCHANGE['exchange'](value or payload()))

    @property
    def upkeep(self):
        return self.home / 'upkeep'


class ExchangeTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.worker = Worker(directory.name)

    def test_a_new_loop_is_announced_only_after_the_whole_install(self):
        self.worker.exchange()
        self.assertTrue((self.worker.upkeep / 'restart').is_file())
        (self.worker.upkeep / 'restart').unlink()
        self.worker.exchange()
        self.assertFalse((self.worker.upkeep / 'restart').exists())
        self.worker.exchange(payload(scheduler='print(2)\n'))
        self.assertTrue((self.worker.upkeep / 'restart').is_file())
        self.assertFalse((self.worker.upkeep / 'installing').exists())
        self.assertEqual((self.worker.upkeep / 'run.py').read_text(), 'print(2)\n')
        # An exchange that stopped after the new loop was written: the next one still asks for
        # the restart, although the files are already the same.
        (self.worker.upkeep / 'restart').unlink()
        with mock.patch.dict(EXCHANGE['exchange'].__globals__, signal=mock.Mock(side_effect=OSError())):
            with self.assertRaises(OSError):
                self.worker.exchange(payload(scheduler='print(3)\n'))
        self.assertTrue((self.worker.upkeep / 'installing').is_file())
        self.worker.exchange(payload(scheduler='print(3)\n'))
        self.assertTrue((self.worker.upkeep / 'restart').is_file())
        self.assertFalse((self.worker.upkeep / 'installing').exists())

    def test_the_payload_arrives_compressed_on_the_command_line(self):
        argument = base64.b64encode(zlib.compress(json.dumps(payload()).encode())).decode()
        names = EXCHANGE['main'].__globals__
        with mock.patch.dict(names, HOME=str(self.worker.home), SESSIONS=str(self.worker.sessions),
                             SESSION_DATA=str(self.worker.data), tmux=self.worker.tmux), \
                mock.patch('sys.argv', ['-', argument]), contextlib.redirect_stdout(io.StringIO()) as out:
            EXCHANGE['main']()
        self.assertTrue(json.loads(out.getvalue())['loop'])
        self.assertEqual((self.worker.upkeep / 'daily.md').read_text(), 'Keep moving. Daily.')

    def test_installs_the_loop_and_binding_and_starts_the_session(self):
        answer = self.worker.exchange(payload(run_now=True))
        upkeep = self.worker.upkeep
        self.assertEqual(json.loads((upkeep / 'config.json').read_text()), {'repositories': ['acme/web']})
        self.assertEqual(stat.S_IMODE(upkeep.stat().st_mode), 0o700)
        self.assertEqual(stat.S_IMODE((upkeep / 'run.sh').stat().st_mode), 0o700)
        self.assertEqual(stat.S_IMODE((upkeep / 'prompt.md').stat().st_mode), 0o600)
        self.assertEqual((upkeep / 'prompt.md').read_text(), 'Keep moving. Hourly.')
        self.assertEqual((upkeep / 'daily.md').read_text(), 'Keep moving. Daily.')
        self.assertTrue((upkeep / 'run-now').is_file())
        binding = self.worker.sessions / SESSION
        self.assertEqual((binding / 'agent').read_text(), 'codex\n')
        self.assertEqual((binding / 'revision').read_text(), 'a' * 40 + '\n')
        self.assertTrue((binding / 'launch-requested').is_file())
        self.assertTrue((self.worker.data / SESSION).is_dir())
        self.assertEqual(answer, {'loop': True, 'settled': False, 'runs': [], 'next_run': None,
                                  'paused': False, 'refused': [], 'companions': [], 'status': None})
        started = self.worker.calls[-1]
        self.assertEqual(started[:4], ('new-session', '-d', '-s', SESSION))
        self.assertEqual(started[-2:], (str(upkeep) + '/run.sh', SESSION))
        self.assertEqual([name for name in os.listdir(upkeep) if name.startswith('.')], [])
        # A pause drops Run now, and Run now while paused is not kept for the resume.
        self.assertTrue(self.worker.exchange(payload(paused=True))['paused'])
        self.assertFalse((upkeep / 'run-now').exists())
        self.worker.exchange(payload(run_now=True))
        self.assertFalse((upkeep / 'run-now').exists())
        # Resume clears the pause; an unchanged file is left as it is.
        before = (upkeep / 'config.json').stat().st_ino
        self.assertFalse(self.worker.exchange(payload(paused=False))['paused'])
        self.assertEqual((upkeep / 'config.json').stat().st_ino, before)

    def test_a_run_now_sent_again_with_its_token_starts_one_run(self):
        upkeep = self.worker.upkeep
        token = 'b' * 32
        self.worker.exchange(payload(run_now=token))
        self.assertEqual((upkeep / 'run-now').read_text(), token)
        take = RUN['take_run_now']
        with mock.patch.dict(take.__globals__, DIR=str(upkeep)):
            # Sent again while pending: still one signal.
            self.worker.exchange(payload(run_now=token))
            self.assertTrue(take())
            self.assertFalse((upkeep / 'run-now').exists())
            self.assertEqual((upkeep / 'run-now-token').read_text(), token)
            # Taken already: sending it again asks for nothing.
            self.worker.exchange(payload(run_now=token))
            self.assertFalse((upkeep / 'run-now').exists())
            # A loop that stopped after it recorded the token never runs it twice.
            (upkeep / 'run-now').write_text(token)
            self.assertFalse(take())
            self.assertFalse((upkeep / 'run-now').exists())
            self.worker.exchange(payload(run_now='c' * 32))
            self.assertTrue(take())
            # A value that is not a token still asks for a run each time, as before.
            self.worker.exchange(payload(run_now='../x'))
            self.assertEqual((upkeep / 'run-now').read_text(), '')
            self.assertTrue(take())
            self.assertFalse(take(), 'no signal, no run')
            # A token that cannot be recorded starts no run, and the loop goes on.
            (upkeep / 'run-now').write_text('d' * 32)
            with contextlib.redirect_stdout(io.StringIO()), \
                    mock.patch('tempfile.mkstemp', side_effect=OSError('disk full')):
                self.assertFalse(take())
            self.assertFalse((upkeep / 'run-now').exists())
            # Pipes the agent planted never block the loop.
            os.mkfifo(upkeep / 'run-now')
            self.assertFalse(take())
            (upkeep / 'run-now-token').unlink()
            os.mkfifo(upkeep / 'run-now-token')
            (upkeep / 'run-now').write_text('e' * 32)
            self.assertTrue(take())
            self.assertEqual((upkeep / 'run-now-token').read_text(), 'e' * 32)

    def test_a_planted_link_is_replaced_and_never_written_through(self):
        self.worker.upkeep.mkdir()
        for name in ('config.json', 'run.sh', 'run-now', 'paused'):
            (self.worker.upkeep / name).symlink_to(self.worker.outside)
        binding = self.worker.sessions / SESSION
        binding.mkdir()
        (binding / 'revision').symlink_to(self.worker.outside)
        (binding / 'launch-requested').symlink_to(self.worker.root / 'created-through-link')
        self.worker.exchange(payload(run_now=True, paused=True))
        self.assertEqual(self.worker.outside.read_text(), 'untouched')
        self.assertFalse((self.worker.root / 'created-through-link').exists())
        self.assertFalse(os.path.lexists(self.worker.upkeep / 'run-now'))
        for path in (self.worker.upkeep / 'config.json', self.worker.upkeep / 'run.sh',
                     self.worker.upkeep / 'paused', binding / 'revision', binding / 'launch-requested'):
            self.assertFalse(path.is_symlink(), path)
            self.assertTrue(path.is_file(), path)

    def test_a_linked_folder_is_refused(self):
        target = self.worker.root / 'elsewhere'
        target.mkdir()
        self.worker.upkeep.symlink_to(target)
        with self.assertRaises(EXCHANGE['Refused']):
            self.worker.exchange()
        self.assertEqual(os.listdir(target), [])
        self.worker.upkeep.unlink()
        (self.worker.sessions / SESSION).symlink_to(target)
        with self.assertRaises(EXCHANGE['Refused']):
            self.worker.exchange()
        self.assertEqual(os.listdir(target), [])
        (self.worker.sessions / SESSION).unlink()
        self.worker.home.rename(self.worker.root / 'real-home')
        self.worker.home.symlink_to(self.worker.root / 'real-home')
        with self.assertRaises(EXCHANGE['Refused']):
            self.worker.exchange()

    def test_another_account_never_writes_into_the_home(self):
        # Root on a worker that isolates agents: the home belongs to the agent account.
        with mock.patch('os.geteuid', return_value=os.geteuid() + 1):
            with self.assertRaises(EXCHANGE['Refused']):
                self.worker.exchange()
        self.assertFalse(self.worker.upkeep.exists())

    def test_reports_from_the_agent_are_read_without_following_or_blocking(self):
        self.worker.upkeep.mkdir(mode=0o700)
        (self.worker.upkeep / 'status.json').symlink_to(self.worker.outside)
        os.mkfifo(self.worker.upkeep / 'next-run')
        lines = ['not json', '[1]'] + [json.dumps({'started_at': n}) for n in range(300)]
        (self.worker.upkeep / 'runs.jsonl').write_text('\n'.join(lines) + '\n')
        answer = self.worker.exchange()
        self.assertIsNone(answer['status'])
        self.assertIsNone(answer['next_run'])
        self.assertEqual(len(answer['runs']), 200)
        self.assertEqual(answer['runs'][-1], {'started_at': 299})

    def test_a_run_file_of_the_agent_cannot_make_the_answer_invalid(self):
        self.worker.upkeep.mkdir(mode=0o700)
        lines = ['{"started_at": 1, "x": NaN, "outcome": "done", "merged": 2}', '[' * 100000,
                 '{"started_at": 3, "outcome": "\\ud800"}',
                 '{"started_at": 99999999999999999999}', '{"started_at": true}',
                 '{"started_at": 2, "finished_at": 1e400, "outcome": {"deep": [[[]]]}, "merged": -1}',
                 '{"started_at": 4, "kind": "daily"}', '{"started_at": 5, "kind": ["daily"]}']
        (self.worker.upkeep / 'runs.jsonl').write_text('\n'.join(lines) + '\n')
        (self.worker.upkeep / 'next-run').write_text('99999999999999999999')
        os.mkdir(self.worker.upkeep / 'status.json')
        answer = self.worker.exchange()
        self.assertEqual(answer['runs'], [{'started_at': 1, 'merged': 2, 'outcome': 'done'},
                                          {'started_at': 3, 'outcome': '?'},
                                          {'started_at': 2}, {'started_at': 4, 'kind': 'daily'},
                                          {'started_at': 5}])
        self.assertIsNone(answer['next_run'])
        self.assertIsNone(answer['status'])

    def test_a_folder_in_place_of_a_file_stops_the_exchange(self):
        for name, value in (('config.json', payload()), ('paused', payload(paused=False))):
            self.worker.upkeep.mkdir(mode=0o700, exist_ok=True)
            (self.worker.upkeep / name).mkdir()
            with self.assertRaisesRegex(EXCHANGE['Refused'], name + ' is a folder'):
                self.worker.exchange(value)
            (self.worker.upkeep / name).rmdir()

    def test_refusals_that_root_logged_reach_the_board_and_nothing_else(self):
        gate = self.worker.root / 'gate'
        gate.mkdir()
        (gate / 'refused.jsonl').write_text('\n'.join([
            json.dumps({'at': 300, 'kind': 'api', 'reason': 'later'}),
            json.dumps({'at': 200, 'kind': 'merge', 'repository': 'acme/web', 'reason': 'no merge.\x1b[2J'}),
            'not json', json.dumps({'at': -1, 'kind': 'api', 'reason': 'x'}), json.dumps(['x']),
            json.dumps({'at': 100, 'name': 'no reason'}), json.dumps({'at': 150, 'kind': [1], 'reason': 'odd'})]))
        names = EXCHANGE['exchange'].__globals__
        logs = ((str(gate), 'refused.jsonl'),)
        with mock.patch.dict(names, REFUSALS=logs, REFUSALS_OWNER=os.getuid()):
            refused = self.worker.exchange()['refused']
        self.assertEqual(refused, [
            {'at': 150, 'text': 'The worker refused a GitHub change: odd'},
            {'at': 200, 'text': 'The worker refused a merge on acme/web: no merge.[2J'},
            {'at': 300, 'text': 'The worker refused a GitHub change: later'}])
        # The last 20 lines come back also at their longest.
        longest = [json.dumps({'at': 1000 + n, 'kind': 'merge', 'repository': 'a' * 39 + '/' + 'b' * 100,
                               'reason': '\u00e9' * 300}) for n in range(25)]
        (gate / 'refused.jsonl').write_text('\n'.join(longest) + '\n')
        with mock.patch.dict(names, REFUSALS=logs, REFUSALS_OWNER=os.getuid()):
            self.assertEqual([entry['at'] for entry in self.worker.exchange()['refused']], list(range(1005, 1025)))
        # A folder of another account, a link or a pipe gives nothing.
        with mock.patch.dict(names, REFUSALS=logs, REFUSALS_OWNER=os.getuid() + 1):
            self.assertEqual(self.worker.exchange()['refused'], [])
        (gate / 'refused.jsonl').unlink()
        os.mkfifo(gate / 'refused.jsonl')
        with mock.patch.dict(names, REFUSALS=logs, REFUSALS_OWNER=os.getuid()):
            self.assertEqual(self.worker.exchange()['refused'], [])
        (gate / 'refused.jsonl').unlink()
        (gate / 'refused.jsonl').symlink_to(self.worker.outside)
        with mock.patch.dict(names, REFUSALS=logs, REFUSALS_OWNER=os.getuid()):
            self.assertEqual(self.worker.exchange()['refused'], [])

    def test_a_status_too_large_to_send_back_is_left_out(self):
        self.worker.upkeep.mkdir(mode=0o700)
        (self.worker.upkeep / 'status.json').write_bytes(b'\x01' * EXCHANGE['STATUS_LIMIT'])
        (self.worker.upkeep / 'next-run').write_text('1800000000 60\n')
        answer = self.worker.exchange()
        self.assertIsNone(answer['status'])
        self.assertEqual(answer['next_run'], 1800000000)

    def test_only_well_formed_companion_requests_reach_horizon(self):
        folder = self.worker.upkeep / 'companions'
        folder.mkdir(parents=True, mode=0o700)
        operation = '1b4e28ba-2fa1-11d2-883f-0016d3cca427'
        (folder / 'youpark.json').write_text(json.dumps({'action': 'ensure_ready', 'operation_id': operation}))
        (folder / 'web.json').write_text(json.dumps({'action': 'delete', 'operation_id': operation}))
        (folder / 'Upper.json').write_text(json.dumps({'action': 'stop', 'operation_id': operation}))
        (folder / 'api.json').write_text(json.dumps({'action': 'stop', 'operation_id': 'not-a-uuid'}))
        (folder / 'big.json').write_text(json.dumps({'action': 'stop', 'operation_id': operation, 'x': 'y' * 2000}))
        # Valid JSON in the first KiB, and more after it.
        padded = json.dumps({'action': 'stop', 'operation_id': operation}).ljust(EXCHANGE['COMPANION_REQUEST_LIMIT'])
        (folder / 'padded.json').write_text(padded + '}')
        (folder / 'fits.json').write_text(padded)
        (folder / 'linked.json').symlink_to(self.worker.outside)
        answer = self.worker.exchange()
        self.assertEqual(answer['companions'],
                         [{'alias': 'fits', 'action': 'stop', 'operation_id': operation},
                          {'alias': 'youpark', 'action': 'ensure_ready', 'operation_id': operation}])
        for n in range(6):
            (folder / f'c{n}.json').write_text(json.dumps({'action': 'stop', 'operation_id': operation}))
        self.assertEqual(len(self.worker.exchange()['companions']), EXCHANGE['COMPANION_LIMIT'])
        # Requests that Horizon answered as done go and make room for the later ones.
        done = {f'c{n}': {'operation_id': operation, 'done': True} for n in range(6)}
        done['fits'] = {'operation_id': operation, 'done': True}
        done['youpark'] = {'operation_id': operation, 'done': False}
        self.assertEqual([ask['alias'] for ask in self.worker.exchange(payload(companions=done))['companions']],
                         ['youpark'])
        self.assertFalse((folder / 'c0.json').exists())
        self.assertTrue((folder / 'youpark.json').exists())

    def test_answered_requests_never_hold_back_later_ones(self):
        folder = self.worker.upkeep / 'companions'
        folder.mkdir(parents=True, mode=0o700)
        operation = '1b4e28ba-2fa1-11d2-883f-0016d3cca427'
        aliases = [chr(ord('a') + n) for n in range(9)]
        for alias in aliases:
            (folder / f'{alias}.json').write_text(json.dumps({'action': 'stop', 'operation_id': operation}))
        # Horizon keeps at most four answers, the ones of the last requests it ran.
        answers, reached = {}, []
        for _ in range(4):
            sent = [ask['alias'] for ask in self.worker.exchange(payload(companions=answers))['companions']]
            reached += [alias for alias in sent if alias not in reached]
            answers = {alias: {'operation_id': operation, 'done': True} for alias in sent}
        self.assertEqual(reached, aliases)
        self.assertEqual(list(folder.iterdir()), [])
        # A request that the agent changed after it was read stays.
        (folder / 'a.json').write_text(json.dumps({'action': 'stop', 'operation_id': operation}))
        seen = os.stat(folder / 'a.json')
        (folder / 'a.json').write_text(json.dumps({'action': 'stop', 'operation_id': operation}) + ' ')
        handle = os.open(folder, os.O_RDONLY)
        try:
            EXCHANGE['retire'](handle, 'a.json', seen)
        finally:
            os.close(handle)
        self.assertTrue((folder / 'a.json').exists())

    def test_horizon_answers_companion_requests_in_a_status_file(self):
        self.assertEqual(self.worker.exchange()['companions'], [])
        self.assertFalse((self.worker.upkeep / 'companions-status.json').exists())
        status = {'youpark': {'phase': 'ready', 'done': True}}
        self.worker.exchange(payload(companions=status))
        written = self.worker.upkeep / 'companions-status.json'
        self.assertEqual(json.loads(written.read_text()), status)
        self.assertEqual(stat.S_IMODE(written.stat().st_mode), 0o600)
        written.unlink()
        written.symlink_to(self.worker.outside)
        self.worker.exchange(payload(companions={}))
        self.assertEqual(self.worker.outside.read_text(), 'untouched')
        self.assertEqual(json.loads(written.read_text()), {})

    def test_an_invalid_session_is_refused(self):
        for value in (payload(session='upkeep-../x'), payload(session='other'), payload(revision='HEAD')):
            with self.assertRaises(EXCHANGE['Refused']):
                self.worker.exchange(value)
        self.assertFalse(self.worker.upkeep.exists())


class LoopTests(unittest.TestCase):
    def test_agents_run_without_approvals_only_where_the_worker_allows_it(self):
        with tempfile.TemporaryDirectory() as root:
            policy = Path(root) / 'agent-approvals'
            names = RUN['approvals'].__globals__
            with mock.patch.dict(names, APPROVALS=str(policy)):
                self.assertEqual(RUN['approvals'](), 'ask')
                policy.write_text('auto\n')
                self.assertEqual(RUN['approvals'](), 'auto')
                with mock.patch('os.geteuid', return_value=0):
                    self.assertEqual(RUN['approvals'](), 'ask')
                for text in ('ask\n', 'auto', 'auto\nauto\n'):
                    policy.write_text(text)
                    self.assertEqual(RUN['approvals'](), 'ask')
                with contextlib.redirect_stdout(io.StringIO()) as output:
                    self.assertFalse(RUN['run_once']({'repositories': ['acme/web']}, True))
                self.assertIn('Upkeep starts no agent', output.getvalue())

    def test_the_session_environment_of_the_image_reaches_the_agent(self):
        with tempfile.TemporaryDirectory() as root:
            tool = Path(root) / 'horizon-worker-session-env'
            tool.write_text('#!/bin/sh\nprintf "DOTNET_ROOT=/opt/dotnet\\0PATH=/opt/dotnet:/usr/bin\\0"\n')
            tool.chmod(0o700)
            with mock.patch.dict(os.environ, PATH=f'{root}:{os.environ["PATH"]}'):
                environment = RUN['session_environment']()
            self.assertEqual(environment['DOTNET_ROOT'], '/opt/dotnet')
            self.assertEqual(environment['PATH'], '/opt/dotnet:/usr/bin')
            tool.write_text('#!/bin/sh\necho "refused: HOME is set by the worker" >&2\nexit 1\n')
            with mock.patch.dict(os.environ, PATH=f'{root}:{os.environ["PATH"]}'), \
                    contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertIsNone(RUN['session_environment']())
            self.assertIn('refused: HOME is set by the worker', output.getvalue())

    def test_checks_that_swap_results_are_a_change_and_names_never_collide(self):
        before = [{'number': 1, 'statusCheckRollup': [{'name': 'a', 'conclusion': 'SUCCESS'},
                                                       {'name': 'b', 'conclusion': 'FAILURE'}]}]
        after = [{'number': 1, 'statusCheckRollup': [{'name': 'a', 'conclusion': 'FAILURE'},
                                                      {'name': 'b', 'conclusion': 'SUCCESS'}]}]
        self.assertNotEqual(RUN['fingerprint'](before, ''), RUN['fingerprint'](after, ''))
        # The person's Approve is a change, also where the repository requires no review.
        approved = [dict(before[0], reviewDecision='', latestReviews=[
            {'author': {'login': 'person'}, 'state': 'APPROVED', 'submittedAt': '2026-10-15T10:00:00Z'}])]
        self.assertNotEqual(RUN['fingerprint'](before, ''), RUN['fingerprint'](approved, ''))
        self.assertEqual(RUN['slug']('acme/b__c'), 'acme__b__c')
        self.assertFalse(RUN['REPOSITORY'].fullmatch('acme__b/c'))

    def test_invalid_names_never_reach_a_command(self):
        config = {'repositories': ['acme/web', '--repo=x/y', 'acme/..', 'acme/web', 'a/b/c', 7],
                  'authors': ['app/dependabot', 'renovate[bot]', '--state=closed', 'a b', 'app/dependabot[bot]'],
                  'order': ['acme/web', '../x']}
        with contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(RUN['names'](config, 'repositories', RUN['REPOSITORY']), ['acme/web'])
            self.assertEqual(RUN['names'](config, 'authors', RUN['AUTHOR']), ['app/dependabot', 'renovate[bot]'])
            self.assertEqual(RUN['names'](config, 'order', RUN['REPOSITORY']), ['acme/web'])
        self.assertIn("Left out an invalid name in the authors of config.json: '--state=closed'", output.getvalue())
        self.assertEqual(RUN['names']({}, 'authors', RUN['AUTHOR'], ['app/dependabot']), ['app/dependabot'])
        self.assertEqual(RUN['names']({'authors': 'app/x'}, 'authors', RUN['AUTHOR']), [])

    def test_the_prompt_reaches_the_agent_on_stdin(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            tools = directory / 'tools'
            tools.mkdir()
            codex = tools / 'codex'
            codex.write_text('#!/bin/sh\n[ "$1" = --help ] && { echo "  --no-daemon"; exit 0; }\n'
                             'printf "args:%s env:%s\\n" "$*" "$UPKEEP_TEST"\ncat\n')
            codex.chmod(0o700)
            (tools / 'horizon-worker-session-env').write_text('#!/bin/sh\nprintf "UPKEEP_TEST=1\\0"\n')
            (tools / 'horizon-worker-check').write_text('#!/bin/sh\n[ "$*" = "--agent codex" ]\n')
            for tool in ('horizon-worker-session-env', 'horizon-worker-check'):
                (tools / tool).chmod(0o700)
            for folder in ('repos', 'logs', 'status.d'):
                (directory / folder).mkdir()
            policy = directory / 'agent-approvals'
            policy.write_text('auto\n')
            guard = directory / 'github-upkeep-policy'
            guard.write_text('1\n')
            names = RUN['Job'].__init__.__globals__
            with mock.patch.dict(names, DIR=str(directory), REFRESH=0.05, APPROVALS=str(policy), GUARD=str(guard),
                                 GUARD_OWNER=os.getuid()), \
                    mock.patch('os.geteuid', return_value=10001), \
                    mock.patch.dict(os.environ, PATH=f'{tools}:{os.environ["PATH"]}'):
                job = RUN['Job']('acme/web')
                job.run('--help\nWork on @@REPOSITORY@@ and write @@STATUS@@.')
            log = Path(job.log).read_text()
            self.assertEqual(job.code, 0)
            self.assertIn('args:--no-daemon exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox - '
                          'env:1\n', log)
            self.assertIn(f'--help\nWork on acme/web and write {directory}/status.d/acme__web.json.', log)
            # A policy that changed to ask during the run starts no further agent, and an old
            # capacity error in the log does not make it wait and try again.
            with open(job.log, 'a') as log:
                log.write('The model is at capacity.\n')
            policy.write_text('ask\n')
            with mock.patch.dict(names, DIR=str(directory), REFRESH=0.05, APPROVALS=str(policy)), \
                    mock.patch('os.geteuid', return_value=10001):
                job = RUN['Job']('acme/web')
                job.run('Work on @@REPOSITORY@@.')
            self.assertIsNone(job.code)
            self.assertFalse(job.launched)
            self.assertLess(job.finished - job.started, 5)
            self.assertIn('this agent did not start', Path(job.log).read_text())


def listing(head):
    """What gh lists: one open pull request with this head."""
    return json.dumps([{'number': 1, 'headRefOid': head, 'statusCheckRollup': [], 'reviewDecision': '',
                        'mergeable': 'MERGEABLE'}])


# The fake gh's answer to a search of a look: the listing on stdin, in each repository that
# the search names, as GitHub returns it. Any other command gets the listing as it is.
SHAPE = """
import json, sys
text = sys.stdin.read()
query = [arg[2:] for arg in sys.argv if arg.startswith('q=')]
try:
    pulls = json.loads(text)
except ValueError:
    pulls = None
if sys.argv[1:3] != ['api', 'graphql'] or not query or not isinstance(pulls, list):
    sys.stdout.write(text)
    sys.exit()
def check(value):
    if 'context' in value:
        return {'__typename': 'StatusContext', 'context': value['context'], 'state': value.get('state')}
    return {'__typename': 'CheckRun', 'name': value.get('name'), 'status': value.get('status'),
            'conclusion': value.get('conclusion') or None,
            'checkSuite': {'workflowRun': {'workflow': {'name': value['workflowName']}}} if value.get('workflowName') else None}
if 'repo:acme/secret' in query[0].split():
    print('gh: Horizon: acme/secret has no GitHub grant on this worker. (HTTP 403)', file=sys.stderr)
    sys.exit(1)
nodes = [{'repository': {'nameWithOwner': word[5:].upper()}, 'state': pr.get('state', 'OPEN'),
          'isCrossRepository': pr.get('isCrossRepository'),
          'number': pr.get('number'), 'headRefOid': pr.get('headRefOid'),
          'reviewDecision': pr.get('reviewDecision') or None, 'mergeable': pr.get('mergeable'),
          'commits': {'nodes': [{'commit': {'statusCheckRollup': {'contexts': {'nodes': [
              check(value) for value in pr.get('statusCheckRollup') or []]}}}}]}}
         for word in query[0].split() if word.startswith('repo:') for pr in pulls]
print(json.dumps({'data': {'search': {'pageInfo': {'hasNextPage': False, 'endCursor': None}, 'nodes': nodes}}}))
"""


class SchedulerTests(unittest.TestCase):
    """run_once with fake gh, codex and session environment tools on PATH."""

    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.dir = self.root / 'upkeep'
        tools = self.root / 'tools'
        for folder in (self.dir / 'repos', self.dir / 'logs', self.dir / 'status.d', tools):
            folder.mkdir(parents=True)
        (self.dir / 'prompt.md').write_text('Work on @@REPOSITORY@@.')
        self.pulls = self.root / 'pulls.json'
        self.pulls.write_text(listing('a'))
        self.agents = self.root / 'agents.log'
        scripts = {
            'gh': self.cat(self.pulls),
            'shape': f'#!{sys.executable}\n{SHAPE}',
            'codex': f'#!/bin/sh\ncat >> {self.agents}\necho >> {self.agents}\n',
            'horizon-worker-session-env': '#!/bin/sh\n',
            'horizon-worker-check': f'#!/bin/sh\n[ ! -e {self.root}/codex-disabled ]\n',
        }
        for name, text in scripts.items():
            (tools / name).write_text(text)
            (tools / name).chmod(0o700)
        policy = self.root / 'agent-approvals'
        policy.write_text('auto\n')
        self.guard = self.root / 'github-upkeep-policy'
        self.guard.write_text('1\n')
        names = RUN['run_once'].__globals__
        for patch in (mock.patch.dict(names, DIR=str(self.dir), APPROVALS=str(policy), REFRESH=0.05,
                                      GUARD=str(self.guard), GUARD_OWNER=os.getuid()),
                      mock.patch.dict(os.environ, PATH=f'{tools}:{os.environ["PATH"]}'),
                      mock.patch('os.geteuid', return_value=10001)):
            patch.start()
            self.addCleanup(patch.stop)

    def cat(self, path):
        """A fake gh that answers with the listing in `path`, for a look too."""
        return f'#!/bin/sh\ncat {path} | {self.root}/tools/shape "$@"\n'

    def run_once(self, forced=False):
        with contextlib.redirect_stdout(io.StringIO()):
            return RUN['run_once']({'repositories': ['acme/web', 'acme/api']}, forced)

    def agents_started(self):
        return self.agents.read_text().count('Work on') if self.agents.exists() else 0

    def test_unchanged_repositories_are_skipped_unless_run_now_forces_them(self):
        self.assertTrue(self.run_once())
        self.assertEqual(self.agents_started(), 2)
        self.assertTrue(self.run_once())
        self.assertEqual(self.agents_started(), 2)
        self.assertTrue(self.run_once(forced=True))
        self.assertEqual(self.agents_started(), 4)
        self.pulls.write_text(listing('b'))
        self.assertTrue(self.run_once())
        self.assertEqual(self.agents_started(), 6)
        # Closed, then opened again unchanged: the agents look at it again.
        pulls = self.pulls.read_text()
        self.pulls.write_text('[]')
        self.assertTrue(self.run_once())
        self.pulls.write_text(pulls)
        self.assertTrue(self.run_once())
        self.assertEqual(self.agents_started(), 8)

    def test_no_agent_starts_where_root_did_not_limit_the_github_access(self):
        self.guard.unlink()
        with contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertFalse(RUN['run_once']({'repositories': ['acme/web']}, True))
        self.assertIn('does not limit the GitHub access', output.getvalue())
        self.assertEqual(self.agents_started(), 0)
        self.guard.symlink_to(self.root / 'agent-approvals')
        self.assertFalse(RUN['guarded']())
        self.guard.unlink()
        self.guard.write_text('1\n')
        with mock.patch.dict(RUN['run_once'].__globals__, GUARD_OWNER=os.getuid() + 1):
            self.assertFalse(RUN['guarded']())
        self.assertTrue(RUN['guarded']())

    def test_a_run_forgets_the_companion_requests_of_earlier_runs(self):
        (self.dir / 'companions').mkdir()
        (self.dir / 'companions' / 'youpark.json').write_text('{}')
        (self.root / 'kept').write_text('kept')
        (self.dir / 'companions' / 'link.json').symlink_to(self.root / 'kept')
        (self.dir / 'companions' / 'deeper').mkdir()
        (self.dir / 'companions' / 'deeper' / 'old.json').write_text('{}')
        # The agent account owns the folders and can take away their permissions.
        (self.dir / 'companions' / 'deeper').chmod(0o000)
        (self.dir / 'companions').chmod(0o000)
        self.assertTrue(RUN['forget_companion_requests']())
        self.assertFalse((self.dir / 'companions').exists())
        self.assertEqual(list(self.dir.glob('.companions-removed-*')), [], 'nothing is left behind')
        self.assertEqual((self.root / 'kept').read_text(), 'kept')
        # A link in place of the folder goes, and what it points to stays.
        (self.root / 'elsewhere').mkdir()
        (self.root / 'elsewhere' / 'youpark.json').write_text('{}')
        (self.dir / 'companions').symlink_to(self.root / 'elsewhere')
        self.assertTrue(RUN['forget_companion_requests']())
        self.assertFalse(os.path.lexists(self.dir / 'companions'))
        self.assertTrue((self.root / 'elsewhere' / 'youpark.json').exists())
        self.assertTrue(RUN['forget_companion_requests'](), 'nothing to remove')

    def test_a_pass_that_cannot_forget_earlier_requests_does_not_start(self):
        (self.dir / 'daily.md').write_text('Daily.')
        with mock.patch.dict(RUN['main'].__globals__, forget_companion_requests=lambda: False):
            self.assertFalse(self.run_daily())
        self.assertFalse((self.dir / 'runs.jsonl').exists())
        self.assertEqual(self.agents_started(), 0)
        self.assertTrue(RUN['daily_due'](False), 'the pass stays due')

    def test_a_profile_without_codex_starts_no_agent(self):
        (self.root / 'codex-disabled').touch()
        self.assertFalse(self.run_once())
        self.assertEqual(self.agents_started(), 0)
        self.assertIn('does not enable Codex', (self.dir / 'logs' / 'acme__web.log').read_text())

    def test_reports_of_any_shape_merge_into_a_bounded_status(self):
        (self.dir / 'status.d' / 'acme__web.json').write_text('[]')
        (self.dir / 'status.d' / 'acme__api.json').write_text(json.dumps({
            'pull_requests': [{'number': n, 'note': 'x' * (5000 if n == 0 else 2000)} for n in range(600)] + [7],
            'activity': [{'at': 'b', 'text': '\ud800'}, {'at': 'a', 'text': 'kept'},
                         {'at': 'c', 'x': json.loads('[' * 40 + ']' * 40)}]}))
        RUN['merge_status'](['acme/web', 'acme/api'])
        text = (self.dir / 'status.json').read_bytes()
        self.assertLess(len(text), EXCHANGE['STATUS_LIMIT'])
        status = json.loads(text)
        self.assertEqual(len(status['pull_requests']), RUN['PULL_REQUESTS'])
        self.assertEqual(status['pull_requests'][0]['number'], 1)
        self.assertEqual(status['activity'], [{'at': 'a', 'text': 'kept'}])
        # Open pull requests of every repository come before merged ones, in turn.
        (self.dir / 'status.d' / 'acme__web.json').write_text(json.dumps({'pull_requests': [
            {'number': n, 'state': 'merged' if n < 3 else 'blocked'} for n in range(5)]}))
        RUN['merge_status'](['acme/web', 'acme/api'])
        numbers = [pr['number'] for pr in json.loads((self.dir / 'status.json').read_text())['pull_requests']]
        self.assertEqual(numbers[:4], [3, 1, 4, 2])
        self.assertEqual(len(numbers), RUN['PULL_REQUESTS'])
        # A report Horizon could not read at all is left out.
        (self.dir / 'status.d' / 'acme__web.json').write_text('[]')
        (self.dir / 'status.d' / 'acme__api.json').write_text('{"pull_requests": [{"number": NaN}]}')
        RUN['merge_status'](['acme/web', 'acme/api'])
        self.assertEqual(json.loads((self.dir / 'status.json').read_text())['pull_requests'], [])

    def test_a_repository_without_open_pull_requests_keeps_only_its_merges(self):
        report = self.dir / 'status.d' / 'acme__web.json'
        report.write_text(json.dumps({'pull_requests': [{'number': 1, 'state': 'ready'},
                                                        {'number': 2, 'state': 'merged'}]}))
        self.pulls.write_text('[]')
        self.assertTrue(self.run_once())
        self.assertEqual(self.agents_started(), 0)
        self.assertEqual(json.loads(report.read_text())['pull_requests'], [{'number': 2, 'state': 'merged'}])

    def test_the_panel_never_gets_a_control_sequence_from_a_log(self):
        job = RUN['Job']('acme/web')
        Path(job.log).write_text('first\n\x1b]52;c;ZXZpbA==\x07Fixed the build\n')
        self.assertEqual(job.last_line(), ']52;c;ZXZpbA==Fixed the build')

    def test_merges_are_the_pull_requests_merged_since_before_the_run(self):
        status = self.dir / 'status.json'
        status.write_text(json.dumps({'pull_requests': [{'repository': 'acme/web', 'number': [1], 'state': 'merged'}]}))
        before = RUN['merged']()
        status.write_text(json.dumps({'pull_requests': [
            {'repository': 'acme/web', 'number': [1], 'state': 'merged'},
            {'repository': 'acme/web', 'number': 2, 'state': 'merged'},
            {'repository': 'acme/web', 'number': 3, 'state': 'ready'}]}))
        self.assertEqual(len(RUN['merged']() - before), 1)

    def test_the_last_run_is_read_from_the_run_records(self):
        self.assertEqual(RUN['last_run'](), (0, 0))
        RUN['record']({'started_at': 3600, 'outcome': 'running'})
        RUN['record']({'started_at': 3600, 'finished_at': 3700, 'outcome': 'failed'})
        self.assertEqual(RUN['last_run'](), (3600, 3700))
        # A slot that passed during a run did not start.
        RUN['record']({'started_at': 4500, 'outcome': 'skipped'})
        self.assertEqual(RUN['last_run'](), (3600, 3700))

    def test_a_new_loop_keeps_the_planned_run_that_a_run_held_back(self):
        hour = 3600
        # Hourly: a run of a change from 09:59 to 11:05 held back the run of 10:00 and skipped
        # 11:00. A loop that starts at 11:05 runs the one of 10:00 now, not at 12:00.
        (self.dir / 'next-run').write_text(f'{10 * hour} 60\n')
        RUN['record']({'started_at': 10 * hour - 60, 'kind': 'change', 'outcome': 'running'})
        RUN['record']({'started_at': 10 * hour - 60, 'finished_at': 11 * hour + 300, 'kind': 'change',
                       'outcome': 'done'})
        RUN['record']({'started_at': 11 * hour, 'outcome': 'skipped'})
        self.assertEqual(RUN['held_back'](12 * hour, 60), 10 * hour)
        # The run of 10:00 ran: nothing is held back.
        RUN['record']({'started_at': 10 * hour + 5, 'finished_at': 10 * hour + 900, 'outcome': 'done'})
        self.assertEqual(RUN['held_back'](12 * hour, 60), 12 * hour)
        # A loop that was stopped over several planned runs does not catch up on old ones.
        RUN['record']({'started_at': 5 * hour, 'finished_at': 5 * hour + 900, 'outcome': 'done'})
        self.assertEqual(RUN['held_back'](12 * hour, 60), 12 * hour)
        # One planned run after the last: the replaced loop's due run still runs.
        self.assertEqual(RUN['held_back'](10 * hour + 60 * 60, 60), 10 * hour)
        # A schedule that changed meanwhile starts from the new one.
        self.assertEqual(RUN['held_back'](10 * hour + 30 * 60, 30), 10 * hour + 30 * 60)

    def test_a_run_the_loop_did_not_finish_is_closed_as_failed(self):
        RUN['record']({'started_at': 100, 'outcome': 'running'})
        RUN['close_interrupted']()
        RUN['close_interrupted']()
        lines = [json.loads(line) for line in (self.dir / 'runs.jsonl').read_text().splitlines()]
        self.assertEqual([line['outcome'] for line in lines], ['running', 'failed'])
        RUN['record']({'started_at': 200, 'kind': 'change', 'outcome': 'running'})
        RUN['close_interrupted']()
        last = json.loads((self.dir / 'runs.jsonl').read_text().splitlines()[-1])
        self.assertEqual((last['kind'], last['outcome']), ('change', 'failed'))

    def poll(self, poller):
        with contextlib.redirect_stdout(io.StringIO()):
            return RUN['poll_once'](poller, time.time() + 3600)

    def test_a_change_between_runs_starts_its_agents_at_once(self):
        (self.dir / 'config.json').write_text(json.dumps({'repositories': ['acme/web', 'acme/api']}))
        self.assertTrue(self.run_once())
        poller = RUN['Poller'](json.loads((self.dir / 'fingerprints.json').read_text()))
        with mock.patch.dict(RUN['run_once'].__globals__, POLL=0):
            # Nothing moved since the agents started: no run, no agent.
            self.assertFalse(self.poll(poller))
            self.assertEqual(self.agents_started(), 2)
            # A new head, a finished check or a review is a change: the agents start at once.
            self.pulls.write_text(listing('b'))
            self.assertTrue(self.poll(poller))
            self.assertEqual(self.agents_started(), 4)
            self.assertFalse(self.poll(poller))
            self.assertEqual(self.agents_started(), 4)
        lines = [json.loads(line) for line in (self.dir / 'runs.jsonl').read_text().splitlines()]
        self.assertEqual([(line['kind'], line['outcome']) for line in lines], [('change', 'running'), ('change', 'done')])
        # No listing within a minute of the last one.
        self.assertFalse(self.poll(poller))
        self.pulls.write_text(listing('c'))
        self.assertFalse(self.poll(poller))
        self.assertEqual(self.agents_started(), 4)

    def test_a_pause_during_a_search_starts_nothing_and_keeps_the_change(self):
        (self.dir / 'config.json').write_text(json.dumps({'repositories': ['acme/web', 'acme/api']}))
        self.assertTrue(self.run_once())
        poller = RUN['Poller'](json.loads((self.dir / 'fingerprints.json').read_text()))
        before = dict(poller.started)
        # The person pauses while the look waits for GitHub.
        (self.root / 'b.json').write_text(listing('b'))
        gh = self.root / 'tools' / 'gh'
        gh.write_text(f'#!/bin/sh\ntouch {self.dir}/paused\ncat {self.root}/b.json | {self.root}/tools/shape "$@"\n')
        with mock.patch.dict(RUN['run_once'].__globals__, POLL=0):
            self.assertFalse(self.poll(poller))
            self.assertEqual(poller.started, before)
            self.assertFalse((self.dir / 'runs.jsonl').exists())
            # The same in a run: the run ends with its own agents only.
            (self.dir / 'paused').unlink()
            self.pulls.write_text(listing('b'))
            gh.write_text(f'#!/bin/sh\ncase "$*" in *graphql*) touch {self.dir}/paused ;; esac\n'
                          f'cat {self.pulls} | {self.root}/tools/shape "$@"\n')
            (self.dir / 'fingerprints.json').write_text('{}')
            with contextlib.redirect_stdout(io.StringIO()):
                RUN['run_once']({'repositories': ['acme/web', 'acme/api']}, False, poller, None, time.time() + 60)
        self.assertEqual(self.agents_started(), 4)
        # After Resume the next look finds the change again.
        (self.dir / 'paused').unlink()
        self.pulls.write_text(listing('c'))
        gh.write_text(self.cat(self.pulls))
        poller.after = 0
        self.assertTrue(self.poll(poller))
        self.assertEqual(self.agents_started(), 6)

    def test_a_pause_holds_back_the_waiting_agents_of_a_change(self):
        # One agent at a time; the person pauses while the first agent works.
        (self.dir / 'config.json').write_text(json.dumps({'repositories': ['acme/web', 'acme/api'],
                                                          'order': ['acme/web', 'acme/api']}))
        (self.root / 'tools' / 'codex').write_text(
            f'#!/bin/sh\ncat >> {self.agents}\necho >> {self.agents}\ntouch {self.dir}/paused\n')
        poller = RUN['Poller']({})
        with mock.patch.dict(RUN['run_once'].__globals__, POLL=0):
            self.assertTrue(self.poll(poller))
            self.assertEqual(self.agents.read_text().split(), ['Work', 'on', 'acme/web.'])
            # After Resume the first look starts the agent that waited, although nothing moved.
            (self.dir / 'paused').unlink()
            self.assertTrue(self.poll(poller))
        self.assertEqual(self.agents.read_text().split()[-1], 'acme/api.')
        self.assertEqual(self.agents_started(), 2)

    def test_a_pause_keeps_a_held_back_run_now_for_an_unchanged_repository(self):
        (self.dir / 'config.json').write_text(json.dumps({'repositories': ['acme/web', 'acme/api'],
                                                          'order': ['acme/web', 'acme/api']}))
        self.assertTrue(self.run_once())
        self.assertEqual(self.agents_started(), 2)
        # Run now that a rate limit held back, for two repositories that did not change.
        poller = RUN['Poller'](json.loads((self.dir / 'fingerprints.json').read_text()))
        poller.forced.update({'acme/web', 'acme/api'})
        (self.root / 'tools' / 'codex').write_text(
            f'#!/bin/sh\ncat >> {self.agents}\necho >> {self.agents}\ntouch {self.dir}/paused\n')
        with mock.patch.dict(RUN['run_once'].__globals__, POLL=0):
            self.assertTrue(self.poll(poller))
            self.assertEqual(self.agents_started(), 3)
            (self.dir / 'paused').unlink()
            # Also when a new loop replaced this one meanwhile.
            poller = RUN['restored']()
            self.assertEqual(poller.forced, {'acme/api'})
            self.assertTrue(self.poll(poller))
            self.assertEqual(self.agents.read_text().split()[-1], 'acme/api.')
            (self.dir / 'paused').unlink()
            self.assertFalse(self.poll(poller))
        self.assertEqual(self.agents_started(), 4)

    def test_a_new_loop_does_not_retry_a_change_whose_agent_failed(self):
        (self.dir / 'config.json').write_text(json.dumps({'repositories': ['acme/web']}))
        self.assertTrue(self.run_once())
        poller = RUN['Poller'](json.loads((self.dir / 'fingerprints.json').read_text()))
        self.pulls.write_text(listing('b'))
        (self.root / 'tools' / 'codex').write_text(f'#!/bin/sh\ncat >> {self.agents}\necho >> {self.agents}\nexit 1\n')
        with mock.patch.dict(RUN['run_once'].__globals__, POLL=0):
            self.assertTrue(self.poll(poller))
            self.assertEqual(self.agents_started(), 3)
            # A new loop starts from what the agents started with, not from the last clean run.
            restarted = RUN['restored']()
            self.assertFalse(self.poll(restarted))
            # A run that starts no agent is kept too.
            self.pulls.write_text(listing('c'))
            with mock.patch.dict(RUN['run_once'].__globals__, session_environment=lambda: None):
                self.assertTrue(self.poll(restarted))
            self.assertFalse(self.poll(RUN['restored']()))
        self.assertEqual(self.agents_started(), 3)

    def test_a_look_leaves_more_pull_requests_than_a_listing_takes_to_the_planned_run(self):
        many = [{'number': number, 'headRefOid': 'a', 'statusCheckRollup': [], 'reviewDecision': '',
                 'mergeable': 'MERGEABLE'} for number in range(RUN['LISTED'] + 1)]
        self.pulls.write_text(json.dumps(many))
        self.assertEqual(RUN['searched'](['acme/web'], ['app/dependabot'], set()), {'acme/web': None})
        poller = RUN['Poller']({'acme/web': 'earlier'})
        self.assertEqual(poller.changed(['acme/web'], ['app/dependabot'], 't'), {})
        self.assertEqual(poller.started, {'acme/web': 'earlier'})
        self.assertEqual(poller.apart, set())

    def test_a_look_drops_closed_pull_requests_also_without_a_fingerprint(self):
        (self.dir / 'config.json').write_text(json.dumps({'repositories': ['acme/web']}))
        report = self.dir / 'status.d' / 'acme__web.json'
        report.write_text(json.dumps({'pull_requests': [{'repository': 'acme/web', 'number': 1, 'state': 'failed'},
                                                        {'repository': 'acme/web', 'number': 2, 'state': 'merged'}]}))
        RUN['merge_status'](['acme/web'])
        self.pulls.write_text('[]')
        # The first agent failed, so no fingerprint is remembered.
        with mock.patch.dict(RUN['run_once'].__globals__, POLL=0):
            self.assertFalse(self.poll(RUN['Poller']({})))
        status = json.loads((self.dir / 'status.json').read_text())
        self.assertEqual([pr['number'] for pr in status['pull_requests']], [2])

    def test_a_change_that_starts_no_agent_and_a_pull_request_that_opens_again(self):
        (self.dir / 'config.json').write_text(json.dumps({'repositories': ['acme/web', 'acme/api']}))
        self.assertTrue(self.run_once())
        poller = RUN['Poller'](json.loads((self.dir / 'fingerprints.json').read_text()))
        with mock.patch.dict(RUN['run_once'].__globals__, POLL=0):
            # A worker that refuses the session environment starts no agent; the same change
            # does not make a run again every minute.
            self.pulls.write_text(listing('b'))
            with mock.patch.dict(RUN['run_once'].__globals__, session_environment=lambda: None):
                self.assertTrue(self.poll(poller))
                self.assertFalse(self.poll(poller))
            self.assertEqual(self.agents_started(), 2)
            self.pulls.write_text(listing('c'))
            self.assertTrue(self.poll(poller))
            self.assertEqual(self.agents_started(), 4)
            # Run now that a rate limit held back starts one run, also when that run starts no agent.
            poller.forced.add('acme/web')
            with mock.patch.dict(RUN['run_once'].__globals__, session_environment=lambda: None):
                self.assertTrue(self.poll(poller))
                self.assertFalse(self.poll(poller))
            # A look that ends after the planned run came due leaves the change to that run.
            self.pulls.write_text(listing('d'))
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertFalse(RUN['poll_once'](poller, time.time() - 1))
            self.assertTrue(self.poll(poller))
            self.assertEqual(self.agents_started(), 6)
            self.pulls.write_text(listing('c'))
            self.assertTrue(self.poll(poller))
            self.assertEqual(self.agents_started(), 8)
            # Closed, then opened again unchanged between the runs: the agents look at it again.
            self.pulls.write_text('[]')
            self.assertFalse(self.poll(poller))
            self.assertEqual(json.loads((self.dir / 'fingerprints.json').read_text()), {})
            self.pulls.write_text(listing('c'))
            self.assertTrue(self.poll(poller))
            self.assertEqual(self.agents_started(), 10)

    def test_a_repository_whose_agent_runs_waits_until_it_ends(self):
        self.assertEqual(set(RUN['Poller']({}).changed(['acme/web', 'acme/api'], ['app/dependabot'], 't',
                                                       busy={'acme/web'})), {'acme/api'})
        # In a run: the listing of the run sees head a; every later listing sees head b.
        (self.root / 'b.json').write_text(listing('b'))
        count = self.root / 'gh-count'
        (self.root / 'tools' / 'gh').write_text(
            f'#!/bin/sh\nn=$(($(cat {count} 2>/dev/null || echo 0) + 1)); echo $n > {count}\n'
            f'if [ $n -le {2 * (1 + len(RUN["MARKED"]))} ]; then cat {self.pulls}; else cat {self.root}/b.json; fi | {self.root}/tools/shape "$@"\n')
        events = self.root / 'events'
        (self.root / 'tools' / 'codex').write_text(
            f'#!/bin/sh\nrepository=$(cat | sed "s/Work on //")\necho "start $repository" >> {events}\n'
            f'sleep 0.3\necho "end $repository" >> {events}\n')
        with mock.patch.dict(RUN['run_once'].__globals__, POLL=0), contextlib.redirect_stdout(io.StringIO()):
            self.assertTrue(RUN['run_once']({'repositories': ['acme/web', 'acme/api']}, False,
                                            RUN['Poller']({}), None, time.time() + 60))
        lines = events.read_text().splitlines()
        for repository in ('acme/web.', 'acme/api.'):
            # Head a, then head b once its first agent ended: never two agents at once.
            self.assertEqual([line.split()[0] for line in lines if line.endswith(repository)],
                             ['start', 'end', 'start', 'end'])

    def test_a_rate_limit_makes_the_loop_wait_longer(self):
        calls = self.root / 'gh-calls'
        gh = self.root / 'tools' / 'gh'
        limit = f'#!/bin/sh\necho x >> {calls}\necho "GraphQL: API rate limit exceeded for user ID 1." >&2\nexit 1\n'
        gh.write_text(limit)
        poller = RUN['Poller']({})
        repositories, authors = ['acme/web', 'acme/api'], ['app/dependabot']
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(poller.changed(repositories, authors, 't'), {})
            self.assertEqual(len(calls.read_text().splitlines()), 1, 'the other repositories wait too')
            self.assertGreater(poller.after - time.time(), RUN['BACKOFF'] - 10)
            self.assertEqual(poller.changed(repositories, authors, 't'), {})
            self.assertEqual(len(calls.read_text().splitlines()), 1, 'no listing before the wait ends')
            poller.after = 0
            gh.write_text(limit.replace('GraphQL: API rate limit exceeded for user ID 1.',
                                        'You have exceeded a secondary rate limit.'))
            poller.changed(repositories, authors, 't')
            self.assertEqual(poller.backoff, 2 * RUN['BACKOFF'])
            # A look that reads nothing, for example on a GitHub error, keeps the longer wait.
            gh.write_text('#!/bin/sh\necho "HTTP 502" >&2\nexit 1\n')
            poller.after = 0
            self.assertEqual(poller.changed(repositories, authors, 't'), {})
            self.assertEqual(poller.backoff, 2 * RUN['BACKOFF'])
            self.assertGreater(poller.after - time.time(), 2 * RUN['BACKOFF'] - 10)
            # So does a run that reads nothing.
            with contextlib.redirect_stdout(io.StringIO()):
                RUN['run_once']({'repositories': repositories}, False, poller)
            self.assertEqual(poller.backoff, 2 * RUN['BACKOFF'])
            # Its agents, which ask for access, are not what this test counts.
            self.agents.unlink()
            gh.write_text(limit)
            poller.after = 0
            poller.changed(repositories, authors, 't')
            self.assertEqual(poller.backoff, 4 * RUN['BACKOFF'])
            # An answer without a limit ends the longer wait.
            gh.write_text(self.cat(self.pulls))
            poller.after = 0
            self.assertEqual(set(poller.changed(repositories, authors, 't')), set(repositories))
            self.assertEqual(poller.backoff, 0)
        # A run that meets the limit lists no more and starts no agent there: the agent would
        # meet it too.
        gh.write_text(limit)
        calls.write_text('')
        self.assertTrue(self.run_once())
        self.assertEqual(self.agents_started(), 0)
        self.assertEqual(len(calls.read_text().splitlines()), 1)
        # Run now held back by a limit: the next look starts the agents, also with no change.
        (self.dir / 'config.json').write_text(json.dumps({'repositories': ['acme/web', 'acme/api']}))
        gh.write_text(self.cat(self.pulls))
        self.assertTrue(self.run_once())
        self.assertEqual(self.agents_started(), 2)
        poller = RUN['Poller'](json.loads((self.dir / 'fingerprints.json').read_text()))
        gh.write_text(limit)
        with contextlib.redirect_stdout(io.StringIO()):
            RUN['run_once']({'repositories': ['acme/web', 'acme/api']}, True, poller)
        self.assertEqual(self.agents_started(), 2)
        gh.write_text(self.cat(self.pulls))
        poller.after = 0
        self.assertTrue(self.poll(poller))
        self.assertEqual(self.agents_started(), 4)
        poller.after = 0
        self.assertFalse(self.poll(poller))
        # A run of a change starts its agent from the look that found it, with no second listing.
        (self.dir / 'config.json').write_text(json.dumps({'repositories': ['acme/web']}))
        (self.dir / 'fingerprints.json').write_text('{}')
        count = self.root / 'gh-count'
        gh.write_text(f'#!/bin/sh\necho x >> {count}\ncat {self.pulls} | {self.root}/tools/shape "$@"\n')
        self.assertTrue(self.poll(RUN['Poller']({})))
        self.assertEqual(self.agents_started(), 5)
        self.assertEqual(len(count.read_text().splitlines()), 1 + len(RUN['MARKED']), 'one look, no listing')

    def test_a_look_searches_twenty_repositories_at_once_as_a_run_lists_them(self):
        pulls = [{'number': 2, 'headRefOid': 'a', 'reviewDecision': '', 'mergeable': 'MERGEABLE',
                  'statusCheckRollup': [
                      {'name': 'test', 'workflowName': 'CI', 'status': 'IN_PROGRESS', 'conclusion': ''},
                      {'name': 'lint', 'workflowName': '', 'status': 'COMPLETED', 'conclusion': 'SUCCESS'},
                      {'context': 'ci/legacy', 'state': 'PENDING'}]}]
        self.pulls.write_text(json.dumps(pulls))
        calls = self.root / 'gh-calls'
        (self.root / 'tools' / 'gh').write_text(
            f'#!/bin/sh\necho x >> {calls}\ncat {self.pulls} | {self.root}/tools/shape "$@"\n')
        repositories = [f'acme/r{number}' for number in range(25)]
        found = RUN['searched'](repositories, ['app/dependabot', 'renovate[bot]'], set())
        self.assertEqual(len(calls.read_text().splitlines()), 2 * (2 + len(RUN['MARKED'])),
                         'two searches for each bot and each marking')
        self.assertEqual(set(found), set(repositories))
        # The worker's GitHub access takes a query of at most 1024 characters.
        long = [f'acme/{"x" * 90}{number}' for number in range(20)]
        split = list(RUN['groups'](long, 'app/dependabot'))
        self.assertEqual(sum(split, []), long)
        self.assertGreater(len(split), 1)
        self.assertTrue(all(len(RUN['query'](group, 'app/dependabot')) <= 1024 for group in split))
        listed = RUN['open_pull_requests']('acme/r0', ['app/dependabot'])
        self.assertEqual(RUN['fingerprint'](found['acme/r24'], 't'), RUN['fingerprint'](pulls, 't'), 'once each')
        self.assertEqual(RUN['fingerprint'](found['acme/r0'][:1], 't'), RUN['fingerprint'](listed, 't'))
        # A search that never ends, or does not answer, lists nothing.
        endless = types.SimpleNamespace(returncode=0, stderr='', stdout=json.dumps(
            {'data': {'search': {'pageInfo': {'hasNextPage': True, 'endCursor': 'x'}, 'nodes': []}}}))
        with mock.patch('subprocess.run', return_value=endless) as run:
            self.assertEqual(RUN['searched'](['acme/web'], ['app/dependabot'], set()), {'acme/web': None})
        self.assertEqual(run.call_count, RUN['PAGES'], 'the other queries leave it alone too')
        self.pulls.write_text('{"number": 1}')
        self.assertEqual(RUN['searched'](['acme/web'], ['app/dependabot'], set()), {'acme/web': None})
        # A pull request that merged a moment ago can still be found; it is not open.
        self.pulls.write_text(json.dumps([dict(pulls[0], state='MERGED')]))
        self.assertEqual(RUN['searched'](['acme/web'], ['app/dependabot'], set()), {'acme/web': []})

    def test_a_repository_without_a_grant_does_not_hide_the_others(self):
        poller = RUN['Poller']({})
        repositories = ['acme/web', 'acme/secret', 'acme/api']
        with mock.patch.dict(RUN['run_once'].__globals__, POLL=0):
            # The worker refuses the search that names it: the next look searches each alone.
            self.assertEqual(poller.changed(repositories, ['app/dependabot'], 't'), {})
            self.assertEqual(poller.apart, set(repositories))
            moved = poller.changed(repositories, ['app/dependabot'], 't')
            self.assertEqual(set(moved), {'acme/web', 'acme/api'})
            self.assertEqual(poller.apart, {'acme/secret'})
            poller.started.update(moved)
            # The others go back into one search; the refused one never starts or forgets.
            calls = self.root / 'gh-calls'
            (self.root / 'tools' / 'gh').write_text(
                f'#!/bin/sh\necho x >> {calls}\ncat {self.pulls} | {self.root}/tools/shape "$@"\n')
            self.assertEqual(poller.changed(repositories, ['app/dependabot'], 't'), {})
            # One search for each query of the others, one refused search for it.
            self.assertEqual(len(calls.read_text().splitlines()), 1 + len(RUN['MARKED']) + 1)
            self.assertNotIn('acme/secret', poller.started)
            # A second bot does not search a repository again whose search was refused.
            calls.write_text('')
            refused = RUN['Poller']({})
            refused.changed(repositories, ['app/dependabot', 'renovate[bot]'], 't')
            self.assertEqual(len(calls.read_text().splitlines()), 1)
            # A search that only failed, for example on a timeout, stays one search.
            (self.root / 'tools' / 'gh').write_text('#!/bin/sh\necho "HTTP 502" >&2\nexit 1\n')
            poller = RUN['Poller']({})
            self.assertEqual(poller.changed(['acme/web', 'acme/api'], ['app/dependabot'], 't'), {})
            self.assertEqual(poller.apart, set())

    def test_the_listing_adds_upkeeps_own_pull_requests_once(self):
        calls = self.root / 'gh-calls'
        gh = self.root / 'tools' / 'gh'
        listing = (f'#!/bin/sh\necho "$@" >> {calls}\n'
                   'case "$*" in *head:upkeep/*) echo \'[{"number": 7, "headRefOid": "u", "isCrossRepository": false},'
                   ' {"number": 9, "headRefOid": "f", "isCrossRepository": true}, {"number": 10}]\' ;;\n'
                   f'*) cat {self.pulls} ;; esac\n')
        gh.write_text(listing)
        found = RUN['open_pull_requests']('acme/web', ['app/dependabot'])
        self.assertEqual(sorted(pr['number'] for pr in found), [1, 7], 'never a fork or an unknown origin')
        self.assertIn('--search label:upkeep', calls.read_text())
        # A look finds the same ones in its searches.
        marked = self.root / 'marked.json'
        marked.write_text('[{"number": 7, "headRefOid": "u", "isCrossRepository": false},'
                          ' {"number": 9, "headRefOid": "f", "isCrossRepository": true}, {"number": 1}]')
        shape = f'{self.root}/tools/shape "$@"'
        gh.write_text(f'#!/bin/sh\ncase "$*" in *head:upkeep/*) cat {marked} | {shape} ;; *) cat {self.pulls} | {shape} ;; esac\n')
        found = RUN['searched'](['acme/web'], ['app/dependabot'], set())['acme/web']
        self.assertEqual(sorted(pr['number'] for pr in found), [1, 7])
        gh.write_text(listing)
        # A marked pull request that moves is a change for the skip rule.
        self.assertTrue(self.run_once())
        started = self.agents_started()
        gh.write_text(gh.read_text().replace('"u"', '"v"'))
        self.assertTrue(self.run_once())
        self.assertEqual(self.agents_started(), started + 2, "both repositories list it")

    def test_a_listing_that_hangs_or_is_not_a_list_reads_as_unreadable(self):
        self.pulls.write_text('{"number": 1}')
        self.assertIsNone(RUN['open_pull_requests']('acme/web', ['app/dependabot']))
        with mock.patch('subprocess.run', side_effect=RUN['subprocess'].TimeoutExpired('gh', 60)):
            self.assertIsNone(RUN['open_pull_requests']('acme/web', ['app/dependabot']))
        # The agent of an unreadable repository starts, and the next look compares with the
        # fingerprint of the agent before it, not with nothing.
        poller = RUN['Poller']({'acme/web': 'earlier'})
        with contextlib.redirect_stdout(io.StringIO()):
            RUN['run_once']({'repositories': ['acme/web']}, False, poller)
        self.assertEqual(self.agents_started(), 1)
        self.assertEqual(poller.started, {'acme/web': 'earlier'})

    def run_daily(self):
        # As the loop does: both read before any agent of the run starts.
        template = RUN['read_prompt']('daily.md')
        with contextlib.redirect_stdout(io.StringIO()):
            return RUN['run_daily']({'repositories': ['acme/web', 'acme/api', '-x']}, template)

    def test_the_daily_pass_runs_once_a_day_as_a_run_of_its_own(self):
        (self.dir / 'daily.md').write_text('Daily for @@REPOSITORIES@@, report to @@STATUS@@.')
        (self.dir / 'status.d' / 'daily.json').write_text('{"activity": [{"at": "a", "text": "yesterday"}]}')
        self.assertTrue(RUN['daily_due'](False))
        self.assertFalse(RUN['daily_due'](True), 'Run now alone never starts it')
        # A request that the run before left behind starts nothing in the pass.
        (self.dir / 'companions').mkdir()
        (self.dir / 'companions' / 'youpark.json').write_text('{}')
        self.assertTrue(self.run_daily())
        self.assertFalse((self.dir / 'companions').exists())
        self.assertIn(f'Daily for acme/web, acme/api, report to {self.dir}/status.d/daily.json',
                      self.agents.read_text())
        self.assertEqual((self.dir / 'daily-day').read_bytes(), RUN['today']())
        self.assertFalse(RUN['daily_due'](False), 'once a day, also after a restart of the loop')
        lines = [json.loads(line) for line in (self.dir / 'runs.jsonl').read_text().splitlines()]
        self.assertEqual([(line['kind'], line['outcome']) for line in lines], [('daily', 'running'), ('daily', 'done')])
        self.assertEqual(lines[0]['started_at'], lines[1]['started_at'])
        self.assertTrue((self.dir / 'logs' / 'daily.log').is_file())
        self.assertNotIn('yesterday', (self.dir / 'status.json').read_text(), 'only this pass reports')
        # A test makes the pass due again with an older day; Run now alone still does not.
        (self.dir / 'daily-day').write_text('2000-01-01')
        self.assertFalse(RUN['daily_due'](True))
        self.assertTrue(RUN['daily_due'](False))

    def test_the_daily_pass_does_not_start_without_its_instructions_or_approvals(self):
        self.assertFalse(self.run_daily())
        (self.dir / 'daily.md').write_text('Daily.')
        (self.root / 'codex-disabled').touch()
        self.assertFalse(self.run_daily())
        (self.root / 'codex-disabled').unlink()
        (self.dir / 'paused').touch()
        self.assertFalse(self.run_daily())
        (self.dir / 'paused').unlink()
        RUN['run_daily'].__globals__['APPROVALS'] = str(self.root / 'missing')
        self.assertFalse(self.run_daily())
        self.assertFalse((self.dir / 'runs.jsonl').exists())
        self.assertTrue(RUN['daily_due'](False), 'a pass that did not start is still due')

    def test_the_pass_runs_from_what_it_read_before_the_agents(self):
        (self.dir / 'daily.md').write_text('Trusted for @@REPOSITORIES@@.')
        template = RUN['read_prompt']('daily.md')
        # A repository agent rewrites the file before the pass starts.
        (self.dir / 'daily.md').write_text('Planted.')
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertTrue(RUN['run_daily']({'repositories': ['acme/web']}, template))
        self.assertIn('Trusted for acme/web.', self.agents.read_text())
        self.assertNotIn('Planted', self.agents.read_text())

    def test_instructions_that_are_not_a_regular_file_are_not_read(self):
        os.mkfifo(self.dir / 'daily.md')
        self.assertIsNone(RUN['read_prompt']('daily.md'))
        (self.dir / 'daily.md').unlink()
        (self.dir / 'daily.md').symlink_to(self.dir / 'prompt.md')
        self.assertIsNone(RUN['read_prompt']('daily.md'))
        (self.dir / 'daily.md').unlink()
        (self.dir / 'daily.md').write_text('x' * (RUN['PROMPT_LIMIT'] + 1))
        self.assertIsNone(RUN['read_prompt']('daily.md'))
        self.assertEqual(RUN['read_prompt']('prompt.md'), 'Work on @@REPOSITORY@@.')

    def test_a_pass_whose_agent_never_launched_stays_due(self):
        (self.dir / 'daily.md').write_text('Daily.')
        (self.dir / 'daily-day').write_text('2000-01-01')
        template = RUN['read_prompt']('daily.md')
        with mock.patch.object(RUN['Job'], 'attempt', lambda job, template: None), \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertTrue(RUN['run_daily']({'repositories': ['acme/web']}, template))
        self.assertEqual((self.dir / 'daily-day').read_text(), '2000-01-01')
        self.assertTrue(RUN['daily_due'](False))
        last = json.loads((self.dir / 'runs.jsonl').read_text().splitlines()[-1])
        self.assertEqual(last['outcome'], 'failed')
        # With no earlier day, the record goes again.
        (self.dir / 'daily-day').unlink()
        with mock.patch.object(RUN['Job'], 'attempt', lambda job, template: None), \
                contextlib.redirect_stdout(io.StringIO()):
            RUN['run_daily']({'repositories': ['acme/web']}, template)
        self.assertFalse((self.dir / 'daily-day').exists())
        # A run record that cannot be written leaves the pass due too.
        (self.dir / 'daily-day').write_text('2000-01-01')
        with mock.patch.dict(RUN['run_daily'].__globals__, record=mock.Mock(side_effect=OSError('full'))), \
                contextlib.redirect_stdout(io.StringIO()), self.assertRaises(OSError):
            RUN['run_daily']({'repositories': ['acme/web']}, template)
        self.assertEqual((self.dir / 'daily-day').read_text(), '2000-01-01')

    def test_a_report_that_is_a_pipe_a_link_or_too_large_never_blocks_the_merge(self):
        os.mkfifo(self.dir / 'status.d' / 'daily.json')
        outside = self.root / 'outside.json'
        outside.write_text('{"activity": [{"at": "a", "text": "outside"}]}')
        (self.dir / 'status.d' / 'acme__web.json').symlink_to(outside)
        (self.dir / 'status.d' / 'acme__api.json').write_text(
            '{"activity": [{"at": "a", "text": "' + 'x' * RUN['REPORT_LIMIT'] + '"}]}')
        RUN['merge_status'](['acme/web', 'acme/api'])
        self.assertEqual(json.loads((self.dir / 'status.json').read_text()), {'pull_requests': [], 'activity': []})

    def test_a_daily_pass_the_loop_did_not_finish_stays_daily(self):
        RUN['record']({'started_at': 100, 'kind': 'daily', 'outcome': 'running'})
        RUN['close_interrupted']()
        last = json.loads((self.dir / 'runs.jsonl').read_text().splitlines()[-1])
        self.assertEqual((last['kind'], last['outcome']), ('daily', 'failed'))

    def test_only_the_daily_report_makes_daily_lines_and_it_keeps_its_own_share(self):
        (self.dir / 'status.d' / 'acme__web.json').write_text(json.dumps({'activity': [
            {'at': f'2026-10-14T01:{n:02}:00Z', 'text': 'repository', 'daily': True} for n in range(90)]}))
        (self.dir / 'status.d' / 'daily.json').write_text(json.dumps({
            'activity': [{'at': f'2026-10-14T00:{n:02}:00Z', 'text': 'daily'} for n in range(30)],
            'pull_requests': [{'number': 1}]}))
        RUN['merge_status'](['acme/web'])
        status = json.loads((self.dir / 'status.json').read_text())
        daily = [entry for entry in status['activity'] if entry.get('daily')]
        self.assertEqual(len(daily), RUN['DAILY_ACTIVITY'])
        self.assertEqual({entry['text'] for entry in daily}, {'daily'})
        self.assertEqual(daily[-1]['at'], '2026-10-14T00:29:00Z')
        self.assertEqual(len(status['activity']), RUN['ACTIVITY'] + RUN['DAILY_ACTIVITY'])
        self.assertEqual(status['pull_requests'], [])

    def test_a_log_past_its_limit_keeps_its_end(self):
        log = self.dir / 'logs' / 'big.log'
        log.write_bytes(b'a' * RUN['LOG_LIMIT'] + b'end')
        RUN['trim'](str(log))
        self.assertEqual(log.stat().st_size, RUN['LOG_LIMIT'] // 8)
        self.assertTrue(log.read_bytes().endswith(b'end'))


if __name__ == '__main__':
    unittest.main()
