"""The Upkeep exchange writes only as the owner of the agent's home and never follows a link
the agent planted; the loop keeps invalid names out of commands and gives the prompt on stdin."""
import contextlib
import io
import json
import os
from pathlib import Path
import runpy
import stat
import tempfile
import types
import unittest
from unittest import mock

WORKER = Path(__file__).resolve().parents[2] / 'crates/horizon-core/src/upkeep/worker'
EXCHANGE = runpy.run_path(str(WORKER / 'exchange.py'))
RUN = runpy.run_path(str(WORKER / 'run.py'))
SESSION = 'upkeep-45ee423d-7fb2'


def payload(**changes):
    value = {'session': SESSION, 'revision': 'a' * 40, 'config': {'repositories': ['acme/web']},
             'prompt': 'Keep moving.', 'run': '#!/bin/bash\n', 'scheduler': 'print()\n',
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

    def test_installs_the_loop_and_binding_and_starts_the_session(self):
        answer = self.worker.exchange(payload(run_now=True, paused=True))
        upkeep = self.worker.upkeep
        self.assertEqual(json.loads((upkeep / 'config.json').read_text()), {'repositories': ['acme/web']})
        self.assertEqual(stat.S_IMODE(upkeep.stat().st_mode), 0o700)
        self.assertEqual(stat.S_IMODE((upkeep / 'run.sh').stat().st_mode), 0o700)
        self.assertEqual(stat.S_IMODE((upkeep / 'prompt.md').stat().st_mode), 0o600)
        self.assertTrue((upkeep / 'run-now').is_file())
        binding = self.worker.sessions / SESSION
        self.assertEqual((binding / 'agent').read_text(), 'codex\n')
        self.assertEqual((binding / 'revision').read_text(), 'a' * 40 + '\n')
        self.assertTrue((binding / 'launch-requested').is_file())
        self.assertTrue((self.worker.data / SESSION).is_dir())
        self.assertEqual(answer, {'loop': True, 'settled': False, 'runs': [], 'next_run': None,
                                  'paused': True, 'status': None})
        started = self.worker.calls[-1]
        self.assertEqual(started[:4], ('new-session', '-d', '-s', SESSION))
        self.assertEqual(started[-2:], (str(upkeep) + '/run.sh', SESSION))
        self.assertEqual([name for name in os.listdir(upkeep) if name.startswith('.')], [])
        # Resume clears the pause; an unchanged file is left as it is.
        before = (upkeep / 'config.json').stat().st_ino
        self.assertFalse(self.worker.exchange(payload(paused=False))['paused'])
        self.assertEqual((upkeep / 'config.json').stat().st_ino, before)

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
        for path in (self.worker.upkeep / 'config.json', self.worker.upkeep / 'run.sh',
                     self.worker.upkeep / 'run-now', binding / 'revision'):
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
        lines = ['{"started_at": 1, "x": NaN, "outcome": "done"}', '[' * 100000,
                 '{"started_at": 3, "outcome": "\\ud800"}',
                 '{"started_at": 99999999999999999999}', '{"started_at": true}',
                 '{"started_at": 2, "finished_at": 1e400, "outcome": {"deep": [[[]]]}}']
        (self.worker.upkeep / 'runs.jsonl').write_text('\n'.join(lines) + '\n')
        (self.worker.upkeep / 'next-run').write_text('99999999999999999999')
        os.mkdir(self.worker.upkeep / 'status.json')
        answer = self.worker.exchange()
        self.assertEqual(answer['runs'], [{'started_at': 1, 'outcome': 'done'}, {'started_at': 3, 'outcome': '?'},
                                          {'started_at': 2}])
        self.assertIsNone(answer['next_run'])
        self.assertIsNone(answer['status'])

    def test_a_folder_in_place_of_a_file_stops_the_exchange(self):
        for name, value in (('config.json', payload()), ('paused', payload(paused=False))):
            self.worker.upkeep.mkdir(mode=0o700, exist_ok=True)
            (self.worker.upkeep / name).mkdir()
            with self.assertRaisesRegex(EXCHANGE['Refused'], name + ' is a folder'):
                self.worker.exchange(value)
            (self.worker.upkeep / name).rmdir()

    def test_a_status_too_large_to_send_back_is_left_out(self):
        self.worker.upkeep.mkdir(mode=0o700)
        (self.worker.upkeep / 'status.json').write_bytes(b'\x01' * EXCHANGE['STATUS_LIMIT'])
        (self.worker.upkeep / 'next-run').write_text('1800000000\n')
        answer = self.worker.exchange()
        self.assertIsNone(answer['status'])
        self.assertEqual(answer['next_run'], 1800000000)

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

    def test_invalid_names_never_reach_a_command(self):
        config = {'repositories': ['acme/web', '--repo=x/y', 'acme/..', 'acme/web', 'a/b/c', 7],
                  'authors': ['app/dependabot', 'renovate[bot]', '--state=closed', 'a b'],
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
            codex.write_text('#!/bin/sh\nprintf "args:%s env:%s\\n" "$*" "$UPKEEP_TEST"\ncat\n')
            codex.chmod(0o700)
            for folder in ('repos', 'logs', 'status.d'):
                (directory / folder).mkdir()
            names = RUN['Job'].__init__.__globals__
            with mock.patch.dict(names, DIR=str(directory), REFRESH=0.05), \
                    mock.patch.dict(os.environ, PATH=f'{tools}:{os.environ["PATH"]}'):
                job = RUN['Job']('acme/web')
                job.run('--help\nWork on @@REPOSITORY@@ and write @@STATUS@@.', dict(os.environ, UPKEEP_TEST='1'))
            log = Path(job.log).read_text()
            self.assertEqual(job.code, 0)
            self.assertIn('args:exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox - env:1\n', log)
            self.assertIn(f'--help\nWork on acme/web and write {directory}/status.d/acme__web.json.', log)


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
        self.pulls.write_text('[{"number": 1, "headRefOid": "a", "statusCheckRollup": []}]')
        self.agents = self.root / 'agents.log'
        scripts = {
            'gh': f'#!/bin/sh\ncat {self.pulls}\n',
            'codex': f'#!/bin/sh\ncat >> {self.agents}\necho >> {self.agents}\n',
            'horizon-worker-session-env': '#!/bin/sh\n',
        }
        for name, text in scripts.items():
            (tools / name).write_text(text)
            (tools / name).chmod(0o700)
        policy = self.root / 'agent-approvals'
        policy.write_text('auto\n')
        names = RUN['run_once'].__globals__
        for patch in (mock.patch.dict(names, DIR=str(self.dir), APPROVALS=str(policy), REFRESH=0.05),
                      mock.patch.dict(os.environ, PATH=f'{tools}:{os.environ["PATH"]}'),
                      mock.patch('os.geteuid', return_value=10001)):
            patch.start()
            self.addCleanup(patch.stop)

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
        self.pulls.write_text('[{"number": 1, "headRefOid": "b", "statusCheckRollup": []}]')
        self.assertTrue(self.run_once())
        self.assertEqual(self.agents_started(), 6)

    def test_reports_of_any_shape_merge_into_a_bounded_status(self):
        (self.dir / 'status.d' / 'acme__web.json').write_text('[]')
        (self.dir / 'status.d' / 'acme__api.json').write_text(json.dumps({
            'pull_requests': [{'number': n, 'note': 'x' * (5000 if n == 0 else 10)} for n in range(600)] + [7],
            'activity': None}))
        RUN['merge_status'](['acme/web', 'acme/api'])
        status = json.loads((self.dir / 'status.json').read_text())
        self.assertEqual(len(status['pull_requests']), RUN['PULL_REQUESTS'])
        self.assertEqual(status['pull_requests'][0]['number'], 1)
        self.assertEqual(status['activity'], [])

    def test_a_run_the_loop_did_not_finish_is_closed_as_failed(self):
        RUN['record']({'started_at': 100, 'outcome': 'running'})
        RUN['close_interrupted']()
        RUN['close_interrupted']()
        lines = [json.loads(line) for line in (self.dir / 'runs.jsonl').read_text().splitlines()]
        self.assertEqual([line['outcome'] for line in lines], ['running', 'failed'])

    def test_a_listing_that_hangs_or_is_not_a_list_reads_as_unreadable(self):
        self.pulls.write_text('{"number": 1}')
        self.assertIsNone(RUN['open_pull_requests']('acme/web', ['app/dependabot']))
        with mock.patch('subprocess.run', side_effect=RUN['subprocess'].TimeoutExpired('gh', 60)):
            self.assertIsNone(RUN['open_pull_requests']('acme/web', ['app/dependabot']))

    def test_a_log_past_its_limit_keeps_its_end(self):
        log = self.dir / 'logs' / 'big.log'
        log.write_bytes(b'a' * RUN['LOG_LIMIT'] + b'end')
        RUN['trim'](str(log))
        self.assertEqual(log.stat().st_size, RUN['LOG_LIMIT'] // 8)
        self.assertTrue(log.read_bytes().endswith(b'end'))


if __name__ == '__main__':
    unittest.main()
