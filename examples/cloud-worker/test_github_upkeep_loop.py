"""What the worker does by itself for the loop of its Upkeep: the service allows the loop's own
requests for the repositories of the policy, ends the loop's grants for a repository that a new
policy drops, and starts the loop again after a container restart. This test process is the
pane of the loop's session."""
import fcntl
import itertools
import json
import os
import subprocess
import sys
from unittest import mock
import unittest

from test_horizon_worker_github import NOW, ServiceTestCase, agents, service

loop = service.upkeep_loop
upkeep = service.upkeep
tasks = agents.tasks
SESSION = 'upkeep-c1'


def policy(*repositories, session=SESSION):
    return {'version': 1, 'session': session, 'repositories': list(repositories or ['example/web']),
            'bots': ['dependabot[bot]'], 'workflows': []}


def root_once():
    """main's own check sees root; the storage checks see this account."""
    return itertools.chain([0], itertools.repeat(os.geteuid()))


class UpkeepTestCase(ServiceTestCase):
    def setUp(self):
        super().setUp()
        self.install()
        self.loop = ('@1', SESSION, 'codex', tasks.root(os.getpid()))
        self.check = mock.Mock(return_value=None)
        self.clock = [NOW]
        self.marker = self.root / 'run/github-upkeep-policy'
        patcher = mock.patch.object(upkeep, 'MARKER', self.marker)
        patcher.start()
        self.addCleanup(patcher.stop)

    def set(self, *repositories, session=SESSION):
        return loop.install(self.store, json.dumps(policy(*repositories, session=session)), self.book)

    def ask(self, who, repository='example/web', access='push'):
        return agents.answer({'request': 'request', 'repository': repository, 'access': access,
                              'reason': 'Fix the update'}, self.store, self.book, NOW, lambda: who)[0]

    def allower(self):
        def decide(*args, **options):
            return agents.decide(*args, now=lambda: NOW, check=self.check, **options)
        return loop.Allower(self.store, self.book, decide, clock=lambda: self.clock[0], log=lambda *a, **k: None)

    def status(self, identifier):
        return next(record for record in self.book.read()['requests'] if record['id'] == identifier)['status']

    def other_session(self):
        child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])
        self.addCleanup(child.wait)
        self.addCleanup(child.kill)
        return ('@2', 'codex-1', 'codex', tasks.root(child.pid))


class PolicyTests(UpkeepTestCase):
    def test_the_requests_report_names_the_policy(self):
        self.assertIsNone(json.loads(self.main_output('requests'))['upkeep'])
        with mock.patch.object(service.common, 'AGENT_ISOLATION', self.root / 'run'):
            installed = json.loads(self.main_output('upkeep', json.dumps(policy())))
        self.assertEqual(installed, {'session': SESSION, 'repositories': ['example/web'],
                                     'bots': ['dependabot[bot]'], 'workflows': []})
        self.assertEqual(json.loads(self.main_output('requests'))['upkeep'], installed)
        self.assertTrue(self.marker.exists())

    def test_a_policy_that_cannot_be_read_allows_nothing(self):
        self.set()
        os.chmod(self.store.persistent / upkeep.POLICY, 0o666)
        self.assertIsNone(loop.current(self.store))
        self.assertIsNone(loop.summary(self.store))
        identifier = self.ask(self.loop)['id']
        self.allower().step()
        self.assertEqual(self.status(identifier), 'pending')

    def main_output(self, operation, stdin=''):
        printed = []
        source = mock.Mock()
        source.buffer.read.return_value = stdin.encode()
        with mock.patch.object(service, 'Store', return_value=self.store), \
                mock.patch.object(service.agents, 'Book', return_value=self.book), \
                mock.patch.object(service.os, 'geteuid', side_effect=root_once()), \
                mock.patch.object(service.os, 'umask'), \
                mock.patch.object(service.sys, 'stdin', source), \
                mock.patch.object(service, 'print_json', side_effect=lambda value: printed.append(json.dumps(value))):
            self.assertEqual(service.main([operation]), 0)
        return printed[0]


class AllowTests(UpkeepTestCase):
    def setUp(self):
        super().setUp()
        self.set('Example/Web')

    def test_the_loops_request_for_its_repository_is_allowed_for_its_session_only(self):
        identifier = self.ask(self.loop, 'example/web', 'push')['id']
        self.allower().step()
        self.assertEqual(self.status(identifier), 'allowed')
        grant = self.book.read()['task_grants'][0]
        self.assertEqual((grant['session'], grant['repository'], grant['access']), (SESSION, 'example/web', 'push'))
        self.assertEqual(self.ask(self.loop)['scope'], 'task')
        # Nothing reaches the cloud's grants.
        self.assertNotIn('cloud_grants', self.store.load()[0])

    def test_other_requests_wait_for_the_person(self):
        other = self.ask(self.other_session(), 'example/web')['id']
        outside = self.ask(self.loop, 'example/rules', 'read')['id']
        self.allower().step()
        self.assertEqual((self.status(other), self.status(outside)), ('pending', 'pending'))
        self.check.assert_not_called()
        self.assertEqual(self.book.read()['task_grants'], [])

    def test_a_request_of_a_session_that_ended_waits_for_the_person(self):
        identifier = self.ask(self.loop)['id']
        # Above the largest process id that Linux gives.
        with self.book.edit() as data:
            data['requests'][0]['root'] = [2 ** 22 + 1, 1]
        self.allower().step()
        self.assertEqual(self.status(identifier), 'pending')
        self.check.assert_not_called()

    def test_without_a_policy_nothing_is_allowed(self):
        loop.remove(self.store)
        identifier = self.ask(self.loop)['id']
        self.allower().step()
        self.assertEqual(self.status(identifier), 'pending')

    def test_a_refused_check_is_tried_again_after_a_while(self):
        identifier = self.ask(self.loop)['id']
        # A second window of the loop asks for the same repository: GitHub is asked once.
        self.ask(('@3',) + self.loop[1:])
        self.check.return_value = 'unreachable'
        allower = self.allower()
        allower.step()
        allower.step()
        self.assertEqual(self.check.call_count, 1)
        self.assertEqual(self.status(identifier), 'pending')
        self.check.return_value = None
        self.clock[0] += loop.RETRY_SECONDS
        allower.step()
        self.assertEqual(self.status(identifier), 'allowed')

    def test_a_repository_removed_while_the_check_runs_is_not_allowed(self):
        identifier = self.ask(self.loop)['id']
        allower = self.allower()
        real_load = upkeep.load
        calls = []

        # The list changes after the request was read and before the decision.
        def load(store, common, marker):
            calls.append(True)
            return real_load(store, common, marker) if len(calls) == 1 else upkeep.parse(policy('example/api'))
        with mock.patch.object(upkeep, 'load', side_effect=load):
            allower.step()
        self.assertEqual(self.status(identifier), 'pending')
        self.check.assert_not_called()

    def test_a_new_list_ends_the_loops_grants_outside_it_and_other_sessions_keep_theirs(self):
        self.ask(self.loop, 'example/web')
        self.allower().step()
        with self.book.edit() as data:
            first = data['task_grants'][0]
            data['task_grants'] += [
                dict(first, repository='example/api'), dict(first, repository='example/rules', access='read'),
                dict(first, session='codex-1'), dict(first, session='upkeep-old')]
        self.set('example/api')
        left = [(grant['session'], grant['repository']) for grant in self.book.read()['task_grants']]
        # The person allows a grant outside the list again; a session the list never named keeps its own.
        self.assertEqual(left, [(SESSION, 'example/api'), ('codex-1', 'example/web'), ('upkeep-old', 'example/web')])
        # The grants of a session that a new list replaces end too.
        self.set('example/api', session='upkeep-c2')
        left = [(grant['session'], grant['repository']) for grant in self.book.read()['task_grants']]
        self.assertEqual(left, [('codex-1', 'example/web'), ('upkeep-old', 'example/web')])

    def test_a_first_list_ends_grants_that_horizon_gave_before_it(self):
        loop.remove(self.store)
        with self.book.edit() as data:
            data['task_grants'] = [{'repository': 'example/gone', 'access': 'push', 'root': list(self.loop[3]),
                                    'window': '@1', 'session': SESSION, 'account': ['client', 'login'],
                                    'granted_at': NOW}]
        self.set('example/web')
        self.assertEqual(self.book.read()['task_grants'], [])

    def test_a_grant_that_the_person_gave_outside_the_list_is_kept_until_a_new_list(self):
        self.ask(self.loop, 'example/web')
        self.allower().step()
        with self.book.edit() as data:
            data['task_grants'].append(dict(data['task_grants'][0], repository='example/rules'))
        self.allower().step()
        self.assertEqual(len(self.book.read()['task_grants']), 2)

    def test_a_policy_is_stored_only_after_its_grants_ended(self):
        self.ask(self.loop, 'example/web')
        self.allower().step()
        with mock.patch.object(service.common, 'write_private', side_effect=OSError), \
                self.assertRaises(OSError):
            self.set('example/api')
        # The book could not be written, so the old list and its grant stay, and the next install tries again.
        self.assertEqual(loop.summary(self.store)['repositories'], ['Example/Web'])
        self.assertEqual(len(self.book.read()['task_grants']), 1)
        # The book was written but the policy was not: the grants ended, the old list stays, and
        # the install fails, so Horizon, which still sees the old list, sets it again.
        with mock.patch.object(upkeep, 'install', side_effect=ValueError), self.assertRaises(ValueError):
            self.set('example/api')
        self.assertEqual(loop.summary(self.store)['repositories'], ['Example/Web'])
        self.assertEqual(self.book.read()['task_grants'], [])

    def test_disconnecting_github_removes_the_policy_under_the_books_lock(self):
        held = []
        real_remove = loop.remove

        def remove(store):
            # No install can run beside it: the book's lock is taken.
            fd = os.open(self.store.runtime / 'access-requests.lock', os.O_RDWR)
            try:
                with self.assertRaises(BlockingIOError):
                    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                held.append(True)
            finally:
                os.close(fd)
            real_remove(store)
        with mock.patch.object(loop, 'remove', side_effect=remove):
            service.clear(self.store, retire=lambda: None)
        self.assertEqual(held, [True])

    def test_disconnecting_github_removes_the_policy(self):
        self.assertIsNotNone(loop.current(self.store))
        service.clear(self.store, retire=lambda: None)
        for directory in (self.store.persistent, self.store.runtime):
            self.assertFalse((directory / upkeep.POLICY).exists())
        # The marker names no policy: the strictest one holds until root sets one, and the
        # worker allows nothing by itself.
        self.assertEqual(self.marker.read_text().strip(), loop.REMOVED)
        self.assertIsNone(upkeep.load(self.store, service.common, self.marker).session)
        self.assertIsNone(loop.current(self.store))
        self.assertIsNone(loop.summary(self.store))

    def test_disconnecting_github_leaves_a_cloud_without_a_policy_without_one(self):
        loop.remove(self.store)
        self.marker.unlink()
        service.clear(self.store, retire=lambda: None)
        self.assertFalse(self.marker.exists())
        self.assertIsNone(upkeep.load(self.store, service.common, self.marker))

    def test_a_failed_policy_removal_keeps_the_chain(self):
        self.ask(self.loop)
        with mock.patch.object(loop, 'remove', side_effect=OSError), self.assertRaises(OSError):
            service.clear(self.store, retire=lambda: None)
        # Nothing of the sign-in went: no policy waits for a later sign-in without its chain.
        self.assertEqual(self.store.load()[0]['state'], 'ok')
        self.assertEqual(len(self.book.read()['requests']), 1)


    def test_the_thread_survives_a_failed_check(self):
        allower = self.allower()
        sleeps = []

        def sleep(seconds):
            sleeps.append(seconds)
            if len(sleeps) == 2:
                raise StopIteration
        with mock.patch.object(allower, 'step', side_effect=OSError), self.assertRaises(StopIteration):
            allower.forever(sleep)
        self.assertEqual(sleeps, [loop.POLL_SECONDS] * 2)


class StartTests(UpkeepTestCase):
    def setUp(self):
        super().setUp()
        self.loop_file = self.root / 'workspace/home/upkeep/run.sh'
        self.loop_file.parent.mkdir(parents=True)
        self.loop_file.write_text('#!/bin/bash\n')
        self.isolation = self.root / 'run/agent-isolation'
        self.isolation.touch()
        self.ready = self.root / 'run/browser-host-instance'
        self.ready.touch()
        self.run = mock.Mock()
        self.sleeps = []

    def start(self):
        return loop.start_loop(self.store, self.book, run=self.run, loop=self.loop_file,
                               isolation=self.isolation, ready=self.ready, sleep=self.sleeps.append, wait=3)

    def test_the_loop_starts_in_its_session_as_the_agent_account(self):
        self.set()
        self.assertTrue(self.start())
        command = self.run.call_args.args[0]
        self.assertEqual(command[:4], [loop.TAILNET, 'agent', 'tmux', '-L'])
        self.assertEqual(command[-2:], [str(self.loop_file), SESSION])
        self.assertIn('-s', command)
        self.assertEqual(command[command.index('-s') + 1], SESSION)
        self.assertEqual(self.run.call_args.kwargs['env'], {'PATH': loop.PATH})

    def test_a_restarted_container_gets_the_marker_again(self):
        # The marker is on tmpfs: a container restart removes it, the volume's policy stays.
        self.set()
        digest = self.marker.read_text().strip()
        self.marker.unlink()
        self.assertTrue(self.start())
        self.assertEqual(self.marker.read_text().strip(), digest)
        self.assertEqual(loop.current(self.store).digest, digest)

    def test_nothing_starts_without_a_policy_isolation_or_a_regular_loop(self):
        self.assertFalse(self.start())
        self.set()
        self.isolation.unlink()
        self.assertFalse(self.start())
        self.isolation.touch()
        self.loop_file.unlink()
        self.assertFalse(self.start())
        self.loop_file.symlink_to('/bin/true')
        self.assertFalse(self.start())
        self.run.assert_not_called()

    def test_the_loop_waits_until_sessions_may_start(self):
        self.set()
        self.ready.unlink()
        self.assertFalse(self.start(), 'no browser host instance yet: no session starts')
        self.assertEqual(self.sleeps, [1, 1, 1])
        self.run.assert_not_called()

    def test_a_policy_removed_while_the_loop_waits_starts_nothing(self):
        self.set()
        self.ready.unlink()

        def sleep(seconds):
            loop.remove(self.store)
            self.ready.touch()
        self.assertFalse(loop.start_loop(self.store, self.book, run=self.run, loop=self.loop_file,
                                         isolation=self.isolation, ready=self.ready, sleep=sleep, wait=3))
        self.run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
