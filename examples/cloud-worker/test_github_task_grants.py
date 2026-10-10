"""Grants for one task: the person allows a repository for the agent session that asked, and
the socket, the Git proxy and the API broker give it only to that session's processes. The
pane processes are this test process and processes that it starts, read from the real /proc."""
import os
from pathlib import Path
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



class GitTests(ProxyTestCase):
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
