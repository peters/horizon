"""Grants for one task: the person allows a repository for the agent session that asked, and
the socket, the Git proxy and the API broker give it only to that session's processes. The
pane processes are this test process and processes that it starts, read from the real /proc."""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from test_github_access_requests import ALPHA
from test_github_api_broker import BrokerTestCase
from test_github_git_proxy import ProxyTestCase
from test_horizon_worker_github import ACCESS, NOW, ServiceTestCase, agents, chain, installation, service

tasks = agents.tasks
mcp = service.mcp


def stat(pid, parent, start):
    """A /proc/PID/stat line with the fields that the module reads."""
    return '%d (a b) S %d %s %d\n' % (pid, parent, ' '.join(['0'] * 17), start)


class FakeProc:
    def __init__(self, root):
        self.root = Path(root)

    def add(self, pid, parent, start, sockets=()):
        folder = self.root / str(pid)
        (folder / 'fd').mkdir(parents=True)
        (folder / 'stat').write_text(stat(pid, parent, start))
        for number, inode in enumerate(sockets):
            os.symlink('socket:[%d]' % inode, folder / 'fd' / str(number))


def grant(repository, access, root, account=('client', 'login')):
    return {'repository': repository, 'access': access, 'root': list(root), 'window': '@1', 'session': 's',
            'account': list(account), 'granted_at': NOW}


class ModuleTests(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.proc = FakeProc(folder.name)
        # Two sessions: pane 10 with a shell 11 and Git 12, pane 20 with gh 21.
        for pid, parent, start in [(10, 1, 100), (11, 10, 110), (12, 11, 120), (20, 1, 200), (21, 20, 210)]:
            self.proc.add(pid, parent, start, sockets=(7,) if pid == 12 else (8,) if pid == 21 else ())

    def test_a_grant_reaches_only_the_processes_of_its_pane(self):
        grants = [grant('example/extra', 'push', (10, 100))]
        account = ['client', 'login']
        self.assertEqual(tasks.lineage(12, self.proc.root), {(12, 120), (11, 110), (10, 100)})
        self.assertEqual(tasks.access(grants, 'Example/Extra', [12], account, self.proc.root), 'push')
        self.assertIsNone(tasks.access(grants, 'example/extra', [21], account, self.proc.root))
        self.assertIsNone(tasks.access(grants, 'example/other', [12], account, self.proc.root))
        self.assertIsNone(tasks.access(grants, 'example/extra', [12], ['client', 'other'], self.proc.root))
        # A socket that processes of two sessions hold gets no task grant, also when both
        # sessions have the same grant.
        self.assertIsNone(tasks.access(grants, 'example/extra', [12, 21], account, self.proc.root))
        both = grants + [grant('example/extra', 'push', (20, 200))]
        self.assertEqual(tasks.access(both, 'example/extra', [21], account, self.proc.root), 'push')
        self.assertIsNone(tasks.access(both, 'example/extra', [12, 21], account, self.proc.root))
        self.assertEqual(tasks.access(both, 'example/extra', [11, 12], account, self.proc.root), 'push')
        self.assertIsNone(tasks.access(grants, 'example/extra', [], account, self.proc.root))

    def test_a_grant_ends_with_its_pane_and_never_passes_to_a_new_process_with_its_id(self):
        grants = [grant('example/extra', 'push', (10, 100)), grant('example/extra', 'push', (20, 999))]
        self.assertEqual(tasks.prune(grants, self.proc.root), grants[:1])
        self.assertIsNone(tasks.access(grants[1:], 'example/extra', [21], ['client', 'login'], self.proc.root))
        self.assertEqual(tasks.root(10, self.proc.root), [10, 100])
        self.assertIsNone(tasks.root(30, self.proc.root))

    def test_the_holders_of_a_socket_are_found_by_its_inode(self):
        self.assertEqual(tasks.holders(7, self.proc.root), [12])
        self.assertEqual(tasks.holders(9, self.proc.root), [])

    def test_a_request_reads_the_task_grants_once(self):
        book = mock.Mock()
        book.read.return_value = {'requests': [], 'task_grants': [grant('example/extra', 'push', (10, 100))]}
        access = agents.task_access(book, None, [12])
        with mock.patch.object(tasks, 'PROC', self.proc.root), \
                mock.patch.object(agents, 'account', return_value=['client', 'login']):
            for _ in range(3):
                access('example/extra')
        self.assertEqual(book.read.call_count, 1)

    def test_root_reads_the_holders_as_the_agent_account(self):
        calls = []

        def run(command, **options):
            calls.append((command, options))
            return subprocess.CompletedProcess(command, 0, stdout='[12, "x", 0, 1]')
        with mock.patch.object(tasks.os, 'geteuid', return_value=0):
            self.assertEqual(tasks.holders(7, self.proc.root, run), [12])
            command, options = calls[0]
            self.assertEqual(command[-2:], ['holders', '7'])
            self.assertEqual((options['user'], options['group'], options['extra_groups']), (10001, 10001, []))
            self.assertEqual(tasks.holders(7, self.proc.root, lambda *a, **k: (_ for _ in ()).throw(
                subprocess.TimeoutExpired('x', 10))), [])

    def test_the_holders_command_scans_as_its_own_account(self):
        left, right = socket.socketpair()
        self.addCleanup(left.close)
        self.addCleanup(right.close)
        inode = os.fstat(left.fileno()).st_ino
        result = subprocess.run([sys.executable, '-I', str(Path(tasks.__file__)), 'holders', str(inode)],
                                capture_output=True, text=True, check=True)
        self.assertEqual(json.loads(result.stdout), [os.getpid()])

    def test_only_whole_grants_are_read_back(self):
        good = grant('example/extra', 'read', (10, 100))
        for bad in [dict(good, access='admin'), dict(good, root=[10]), dict(good, root=[0, 1]),
                    dict(good, repository='../x'), dict(good, extra=1), dict(good, granted_at='now')]:
            self.assertFalse(tasks.valid(bad), bad)
        self.assertTrue(tasks.valid(good))


class DecisionTests(ServiceTestCase):
    """The person allows a repository for the task; this test process is the pane of the
    session that asked, and its parent is outside that session."""

    def setUp(self):
        super().setUp()
        self.install()
        self.me = ('@1', 'agent-alpha', 'claude', tasks.root(os.getpid()))
        self.outside = os.getppid()

    def ask(self, who, repository='example/extra', access='push'):
        return agents.answer({'request': 'request', 'repository': repository, 'access': access,
                              'reason': 'Push the fix'}, self.store, self.book, NOW, lambda: who)[0]

    def decide(self, identifier, decision='allow-task'):
        return agents.decide(self.store, self.book, identifier, decision, now=lambda: NOW,
                             check=mock.Mock(return_value=None))

    def token(self, pid, repository='example/extra'):
        return agents.answer({'request': 'gh-token', 'repository': repository}, self.store, self.book, NOW,
                             lambda: self.me, pid)[0]

    def test_a_task_grant_reaches_the_session_that_asked_and_no_other(self):
        identifier = self.ask(self.me)['id']
        self.assertTrue(agents.requests_report(self.book, self.store.load()[0], lambda: NOW)['requests'][0]['task'])
        self.assertEqual(self.decide(identifier), {
            'ok': True, 'id': identifier, 'decision': 'allow-task', 'repository': 'example/extra',
            'access': 'push', 'status': 'allowed', 'scope': 'task'})
        status = agents.answer({'request': 'request-status', 'id': identifier}, self.store, self.book, NOW,
                               lambda: self.me)[0]
        self.assertEqual((status['status'], status['scope']), ('allowed', 'task'))
        self.assertEqual(self.ask(self.me)['scope'], 'task')
        self.assertEqual(self.token(os.getpid())['token'], ACCESS)
        self.assertFalse(self.token(self.outside)['ok'])
        # The cloud's grants do not change, and Horizon's status does not list a task grant.
        self.assertNotIn('cloud_grants', self.store.load()[0])
        self.assertNotIn('example/extra', [item['repository'] for item in service.status(self.store)['repositories']])
        # Another session of the cloud asks on its own.
        child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])
        self.addCleanup(child.wait)
        self.addCleanup(child.kill)
        other = ('@2', 'agent-beta', 'codex', tasks.root(child.pid))
        self.assertEqual(self.ask(other)['status'], 'pending')

    def test_git_and_gh_use_the_task_grant_of_their_own_session_only(self):
        self.decide(self.ask(self.me, access='read')['id'])
        plan = service.gitproxy.plan
        self.assertEqual(plan(self.store, 'example/extra', 'read', NOW, holders=lambda: [os.getpid()]).token, ACCESS)
        with self.assertRaisesRegex(service.gitproxy.relay.Refusal, 'reading only'):
            plan(self.store, 'example/extra', 'push', NOW, holders=lambda: [os.getpid()])
        self.assertIsNone(plan(self.store, 'example/extra', 'read', NOW, holders=lambda: [self.outside]).token)
        found, _ = service.broker.candidates(self.store, NOW, static=lambda: None, pid=os.getpid())
        self.assertTrue(found[0].allowed('example/extra', 'read'))
        self.assertFalse(found[0].allowed('example/extra', 'push'))
        found, _ = service.broker.candidates(self.store, NOW, static=lambda: None, pid=self.outside)
        self.assertFalse(found[0].allowed('example/extra', 'read'))

    def test_the_proxy_looks_for_the_callers_processes_only_when_a_task_has_grants(self):
        def holders():
            raise AssertionError('no task grant, so no scan')
        plan = service.gitproxy.plan(self.store, 'example/other', 'read', NOW, holders=holders)
        self.assertIsNone(plan.token)

    def test_a_session_that_ended_is_not_allowed(self):
        ended = self.me[:3] + ([os.getpid(), self.me[3][1] + 1],)
        identifier = self.ask(ended)['id']
        self.assertFalse(agents.requests_report(self.book, self.store.load()[0], lambda: NOW)['requests'][0]['task'],
                         'the card offers only the cloud')
        self.assertEqual(self.decide(identifier)['error'], 'session_ended')
        self.assertEqual(self.book.read()['requests'][0]['status'], 'expired')
        self.assertEqual(self.book.read()['task_grants'], [])
        # A request from before task grants has no pane to allow.
        identifier = self.ask(ALPHA[:3] + (None,), 'example/other')['id']
        self.assertEqual(self.decide(identifier)['error'], 'session_ended')

    def test_a_session_or_chain_that_changes_while_github_is_asked_gets_no_grant(self):
        child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])
        self.addCleanup(child.wait)
        self.addCleanup(child.kill)
        identifier = self.ask(('@2', 'agent-beta', 'codex', tasks.root(child.pid)))['id']

        def session_ends(*args):
            child.kill()
            child.wait()
        self.assertEqual(agents.decide(self.store, self.book, identifier, 'allow-task', now=lambda: NOW,
                                       check=session_ends)['error'], 'session_ended')
        self.assertEqual(self.book.read()['task_grants'], [])
        identifier = self.ask(self.me)['id']

        def another_account(*args):
            self.install(installation(login='someone-else', chain=chain(access='ghu_other')))
        self.assertEqual(agents.decide(self.store, self.book, identifier, 'allow-task', now=lambda: NOW,
                                       check=another_account)['error'], 'chain_changed')
        self.assertEqual(self.book.read()['task_grants'], [])

    def test_grants_of_another_account_do_not_count_toward_the_bound(self):
        with self.book.edit() as data:
            data['task_grants'] = [grant('example/old', 'push', tasks.root(os.getpid()), ['other-app', 'someone'])]
        with mock.patch.object(tasks, 'MAX_TASK_GRANTS', 1):
            self.assertEqual(self.decide(self.ask(self.me)['id'])['status'], 'allowed')
        self.assertEqual([item['repository'] for item in self.book.read()['task_grants']], ['example/extra'])

    def test_every_decision_drops_ended_task_grants(self):
        with self.book.edit() as data:
            data['task_grants'] = [grant('example/gone', 'push', (os.getpid(), self.me[3][1] + 1),
                                         agents.account(self.store.load()[0]))]
        self.decide(self.ask(self.me)['id'], 'deny')
        self.assertEqual(self.book.read()['task_grants'], [])

    def test_task_grants_are_bounded_and_ended_ones_are_dropped(self):
        with self.book.edit() as data:
            data['task_grants'] = [grant('example/gone', 'push', (os.getpid(), self.me[3][1] + 1),
                                         agents.account(self.store.load()[0]))]
        with mock.patch.object(tasks, 'MAX_TASK_GRANTS', 1):
            self.assertEqual(self.decide(self.ask(self.me)['id'])['status'], 'allowed')
            self.assertEqual([item['repository'] for item in self.book.read()['task_grants']], ['example/extra'])
            self.assertEqual(self.decide(self.ask(self.me, 'example/more')['id'])['error'], 'too_many_task_grants')

    def test_the_tool_names_the_scope(self):
        reply = {'ok': True, 'status': 'allowed', 'scope': 'task', 'id': None}
        success, text = mcp.call_tool({'repository': 'example/extra', 'access': 'push', 'reason': 'PR'},
                                      lambda _: reply)
        self.assertTrue(success)
        self.assertIn('Allowed for this task', text)
        self.assertIn('for this task only', mcp.GUIDANCE)



class LateEndTests(BrokerTestCase):
    def test_a_task_that_ends_while_its_request_is_read_sends_no_token(self):
        account = agents.account(self.store.load()[0])
        with self.book.edit() as data:
            data['task_grants'] = [grant('example/secret', 'read', tasks.root(os.getpid()), account)]
        choose = service.broker.choose

        def then_the_session_ends(*args):
            chosen = choose(*args)
            with self.book.edit() as data:
                data['task_grants'] = []
            return chosen
        with mock.patch.object(service.broker, 'choose', then_the_session_ends):
            status, _, payload = self.send('GET', '/repos/example/secret/issues')
        self.assertEqual(status, 403)
        self.assertIn(b'any more', payload)
        self.assertEqual(self.api.seen, [])

    def test_a_task_that_ends_while_a_graphql_request_is_planned_sends_no_query(self):
        account = agents.account(self.store.load()[0])
        with self.book.edit() as data:
            data['task_grants'] = [grant('example/secret', 'read', tasks.root(os.getpid()), account)]
        plan = service.broker.policy.plan
        calls = []

        def then_the_session_ends(*args):
            planned = plan(*args)
            calls.append(planned)
            if len(calls) == 1:
                with self.book.edit() as data:
                    data['task_grants'] = []
            return planned
        with mock.patch.object(service.broker.policy, 'plan', then_the_session_ends):
            reply = self.graphql('{ repository(owner: "example", name: "secret") { name } }')
        self.assertIn('example/secret has no GitHub grant', reply['errors'][0]['message'])
        self.assertEqual([item for item in self.api.seen if b'secret' in item[4]], [])


    def test_an_account_change_while_a_request_is_read_sends_no_token(self):
        account = agents.account(self.store.load()[0])
        with self.book.edit() as data:
            data['task_grants'] = [grant('example/secret', 'read', tasks.root(os.getpid()), account)]
        choose = service.broker.choose

        def then_another_account(*args):
            chosen = choose(*args)
            self.install(installation(login='someone-else', chain=chain(access='ghu_other')))
            return chosen
        with mock.patch.object(service.broker, 'choose', then_another_account):
            status, _, payload = self.send('GET', '/repos/example/secret/issues')
        self.assertEqual(status, 403)
        self.assertEqual(self.api.seen, [])


class GitTests(ProxyTestCase):
    def test_a_socket_that_passes_to_another_session_before_sending_gets_no_token(self):
        env = self.routed()
        account = agents.account(self.store.load()[0])
        with self.book.edit() as data:
            data['task_grants'] = [grant('example/secret', 'read', tasks.root(os.getpid()), account)]
        scan = tasks.holders
        calls = []

        def holders(inode, *args):
            calls.append(inode)
            # The first scan finds this session; the scan before sending finds another one.
            return scan(inode, *args) if len(calls) == 1 else [os.getppid()]
        with mock.patch.object(tasks, 'holders', holders):
            result, _ = self.clone('example/secret', env)
        self.assertIn('any more', result.stderr)
        self.assertEqual([item for item in self.github.seen if item[2]], [], 'no token reached GitHub')

    def test_a_task_that_ends_while_git_is_answered_sends_no_token(self):
        env = self.routed()
        account = agents.account(self.store.load()[0])
        with self.book.edit() as data:
            data['task_grants'] = [grant('example/secret', 'read', tasks.root(os.getpid()), account)]
        plan = service.gitproxy.plan

        def then_the_session_ends(*args, **options):
            planned = plan(*args, **options)
            with self.book.edit() as data:
                data['task_grants'] = []
            return planned
        with mock.patch.object(service.gitproxy, 'plan', then_the_session_ends):
            result, _ = self.clone('example/secret', env)
        self.assertIn('any more', result.stderr)
        self.assertEqual([item for item in self.github.seen if item[2]], [], 'no token reached GitHub')

    def test_git_reaches_a_repository_allowed_for_its_task_and_other_sessions_do_not(self):
        env = self.routed()
        result, _ = self.clone('example/secret', env)
        self.assertIn('remote: Horizon: example/secret has no GitHub grant on this worker', result.stderr)
        account = agents.account(self.store.load()[0])
        # This test process is the pane of the session; Git runs below it.
        with self.book.edit() as data:
            data['task_grants'] = [grant('example/secret', 'push', tasks.root(os.getpid()), account)]
        result, secret = self.clone('example/secret', env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.push(secret, env).returncode, 0)
        # The same grant for another session's pane: Git here is not below it.
        child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])
        self.addCleanup(child.wait)
        self.addCleanup(child.kill)
        with self.book.edit() as data:
            data['task_grants'] = [grant('example/secret', 'push', tasks.root(child.pid), account)]
        result = self.push(secret, env)
        self.assertIn('remote: Horizon: example/secret has no GitHub grant on this worker', result.stderr)



class BrokerTests(BrokerTestCase):
    def test_gh_reaches_a_repository_allowed_for_its_task_and_other_sessions_do_not(self):
        self.assertEqual(self.send('GET', '/repos/example/secret/issues')[0], 403)
        account = agents.account(self.store.load()[0])
        with self.book.edit() as data:
            data['task_grants'] = [grant('example/secret', 'read', tasks.root(os.getpid()), account)]
        self.assertEqual(self.send('GET', '/repos/example/secret/issues')[0], 200)
        self.assertEqual(self.api.seen[-1][3], 'token ' + ACCESS)
        status, _, payload = self.send('POST', '/repos/example/secret/issues', body={'title': 't'})
        self.assertEqual(status, 403)
        self.assertIn(b'reading only', payload)
        child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])
        self.addCleanup(child.wait)
        self.addCleanup(child.kill)
        with self.book.edit() as data:
            data['task_grants'] = [grant('example/secret', 'read', tasks.root(child.pid), account)]
        self.assertEqual(self.send('GET', '/repos/example/secret/issues')[0], 403)


if __name__ == '__main__':
    unittest.main()
