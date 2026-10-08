"""Lifecycle of the worker GitHub service: a chain that only memory holds, clear across
processes, the host's status of the serving process and a start that fails."""
import contextlib
import io
import json
import os
import threading
import time
from unittest import mock

from test_horizon_worker_github import NOW, ServiceTestCase, chain, common, installation, rotated, service


class MemoryOnlyChainTests(ServiceTestCase):
    def rotate_without_storage(self):
        """GitHub rotates the chain while no storage accepts a write."""
        self.install()
        _, post = self.fake_github((200, rotated(2)))
        due = chain()['access_expires_at'] - 60
        with mock.patch.object(common, 'write_private', side_effect=OSError), self.assertRaises(ValueError):
            service.refresh_once(self.store, lambda: due, post)
        return due

    def test_the_next_check_writes_a_chain_that_only_memory_holds(self):
        due = self.rotate_without_storage()
        report = service.status(self.store)
        self.assertEqual((report['persistent'], report['pending_write']), (False, True))
        logged = []
        log = lambda text, **kwargs: logged.append(text)  # noqa: E731
        no_refresh = mock.Mock(side_effect=AssertionError('the rotated chain is fresh'))
        with mock.patch.object(common, 'write_private', side_effect=OSError):
            service.refresh_step(self.store, 0, lambda: due, no_refresh, log=log)
        self.assertIn('GitHub token chain still not stored: ValueError', logged)
        self.assertTrue(service.status(self.store)['pending_write'])
        service.refresh_step(self.store, 0, lambda: due, no_refresh, log=log)
        self.assertIsNone(self.store.pending)
        restarted = service.Store(self.store.persistent, self.root / 'run/after-restart')
        self.assertEqual(restarted.load()[0]['chain']['refresh_token'], 'ghr_synthetic-refresh-2')
        report = service.status(self.store)
        self.assertEqual((report['persistent'], report['pending_write']), (True, False))

    def test_clear_by_another_process_ends_a_chain_that_only_memory_holds(self):
        self.rotate_without_storage()
        host = service.Store(self.store.persistent, self.store.runtime)
        service.clear(host)
        self.assertIsNone(self.store.load()[0])
        reply, _ = service.answer({'request': 'gh-token', 'repository': 'example/project'}, self.store.load()[0], NOW)
        self.assertEqual(reply['state'], 'absent')
        self.assertTrue(self.store.retry_pending())
        self.assertIsNone(self.store.pending)
        self.assertFalse((self.store.persistent / service.STATE).exists())
        # A later install counts again.
        self.install()
        self.assertEqual(self.store.load()[0]['chain'], chain())


class HostStatusTests(ServiceTestCase):
    def setUp(self):
        super().setUp()
        self.install()
        self.path = self.root / 'run/worker/github.sock'
        server = service.listen(self.path)
        self.addCleanup(server.close)
        threading.Thread(target=service.accept_forever, args=(server, self.store), daemon=True).start()
        patcher = mock.patch.object(service, 'ALLOWED_UIDS', (os.getuid(),))
        patcher.start()
        self.addCleanup(patcher.stop)

    def test_the_host_reads_the_status_of_the_serving_process(self):
        self.store.pending = dict(self.stored(), serial=time.time_ns() + 10 ** 12, chain=chain(access='ghu_memory'))
        host = service.Store(self.store.persistent, self.store.runtime)
        with mock.patch.object(service, 'ROOT_UID', os.getuid()):
            report = service.host_status(host, self.path)
        self.assertEqual((report['serving'], report['pending_write'], report['persistent']), (True, True, False))
        self.assertNotIn('ghu_', json.dumps(report))
        # Only root reads it; otherwise, and without a service, the stored copies answer.
        for path in (self.path, self.root / 'missing.sock'):
            report = service.host_status(host, path)
            self.assertEqual((report['serving'], report['pending_write'], report['persistent']), (False, False, True))


class StartTests(ServiceTestCase):
    def test_a_start_that_fails_is_retried_then_ends_the_service(self):
        path = self.root / 'run/worker/github.sock'
        waits = []
        with mock.patch.object(common, 'AGENT_ISOLATION', self.root / 'missing'), \
                contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertIsNone(service.start(self.store, path, sleep=waits.append))
            with mock.patch.object(service, 'START_WAIT_SECONDS', 0):
                self.assertEqual(service.serve(self.store, path), 1)
        self.assertEqual(waits, [2, 4, 8, 16])
        self.assertIn('GitHub access cannot start: ValueError', output.getvalue())
        self.assertFalse(path.exists())
        isolation = self.root / 'agent-isolation'
        isolation.touch()
        with mock.patch.object(common, 'AGENT_ISOLATION', isolation):
            server = service.start(self.store, path, sleep=waits.append)
        self.addCleanup(server.close)
        self.assertTrue(path.exists())

    def test_install_needs_agent_isolation(self):
        source = io.TextIOWrapper(io.BytesIO(json.dumps(installation()).encode()))
        with mock.patch.object(service.os, 'geteuid', return_value=0), \
                mock.patch.object(common, 'AGENT_ISOLATION', self.root / 'missing'), \
                mock.patch.object(service, 'Store', return_value=self.store), \
                mock.patch.object(service.sys, 'stdin', source), self.assertRaisesRegex(ValueError, 'isolation'):
            service.main(['install'])
        self.assertIsNone(self.stored())
