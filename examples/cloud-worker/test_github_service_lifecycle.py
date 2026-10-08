"""Lifecycle of the worker GitHub service: a chain that only memory holds, clear across
processes, the host's status of the serving process and a start that fails."""
import contextlib
import fcntl
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
        service.clear(host, retire=lambda: None)
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


class VolumeCopyTests(ServiceTestCase):
    def fail_volume_writes(self):
        real = common.write_private

        def write(directory, name, data):
            if directory == self.store.persistent:
                raise OSError('volume refused the write')
            return real(directory, name, data)
        return mock.patch.object(common, 'write_private', side_effect=write)

    def test_an_older_volume_chain_is_removed_before_tmpfs_takes_a_new_one(self):
        self.install()
        old = self.store.persistent / service.STATE
        with self.fail_volume_writes():
            report = self.install(installation(chain=chain(access='ghu_synthetic-new')))
        self.assertFalse(report['persistent'])
        self.assertFalse(old.exists())
        self.assertEqual(self.stored()['chain']['access_token'], 'ghu_synthetic-new')
        restarted = service.Store(self.store.persistent, self.root / 'run/after-restart')
        self.assertIsNone(restarted.load()[0], 'a restart must not bring the older chain back')

    def test_a_removal_that_does_not_reach_the_disk_fails_the_fallback(self):
        self.install()
        old = self.store.persistent / service.STATE
        real = common.sync_descriptor

        def fsync(fd, directory):
            if directory == self.store.persistent:
                raise OSError('input/output error')
            return real(fd, directory)
        with self.fail_volume_writes(), mock.patch.object(common, 'sync_descriptor', side_effect=fsync), \
                self.assertRaisesRegex(ValueError, 'older GitHub token chain'):
            self.install(installation(chain=chain(access='ghu_synthetic-new')))
        self.assertFalse((self.store.runtime / service.STATE).exists())

    def test_clear_keeps_the_chain_while_the_static_binding_stays(self):
        self.install()

        def stuck():
            raise OSError('busy')
        with self.assertRaises(OSError):
            service.clear(self.store, retire=stuck)
        self.assertEqual(self.stored()['chain'], chain(), 'agents never keep a static token after clear')
        removed = []
        service.clear(self.store, retire=lambda: removed.append(True))
        self.assertEqual(removed, [True])
        self.assertIsNone(self.stored())

    def test_serials_count_up_without_the_clock_and_survive_a_clear(self):
        self.install()
        first = self.stored()['serial']
        self.install(installation(chain=chain(access='ghu_synthetic-second')))
        self.assertEqual(self.stored()['serial'], first + 1)
        service.clear(self.store, retire=lambda: None)
        self.install(installation(chain=chain(access='ghu_synthetic-third')))
        self.assertEqual(self.stored()['chain']['access_token'], 'ghu_synthetic-third',
                         'a chain installed after a clear counts')
        self.assertGreater(self.stored()['serial'], first + 1)

    def test_a_pending_chain_never_overwrites_a_newer_install(self):
        self.install(installation(chain=chain(access='ghu_synthetic-newer')))
        # A rotation this process could not store, older than the install another process made.
        self.store.pending = dict(self.stored(), serial=self.stored()['serial'] - 1,
                                  chain=chain(access='ghu_synthetic-stale'))
        with mock.patch.object(common, 'write_private', side_effect=AssertionError('must not write')):
            self.assertTrue(self.store.retry_pending())
        self.assertIsNone(self.store.pending)
        self.assertEqual(self.stored()['chain']['access_token'], 'ghu_synthetic-newer')

    def test_a_clear_that_cannot_remove_the_volume_copy_changes_nothing(self):
        self.install()
        real = common.sync_descriptor

        def fsync(fd, directory):
            if directory == self.store.persistent:
                raise OSError('input/output error')
            return real(fd, directory)
        with mock.patch.object(common, 'sync_descriptor', side_effect=fsync), self.assertRaises(OSError):
            service.clear(self.store, retire=lambda: None)
        self.assertFalse((self.store.runtime / common.CLEARED).exists(), 'no clear mark without a durable removal')

    def test_a_write_whose_directory_sync_fails_counts_as_stored(self):
        self.install()
        real = common.fsync_directory

        def fsync(directory):
            if directory == self.store.persistent:
                raise OSError('input/output error')
            return real(directory)
        with mock.patch.object(common, 'fsync_directory', side_effect=fsync):
            self.install(installation(chain=chain(access='ghu_synthetic-new')))
        self.assertEqual(self.stored()['chain']['access_token'], 'ghu_synthetic-new')
        self.assertTrue((self.store.runtime / service.STATE).exists(), 'tmpfs keeps a second copy')
        self.assertFalse(service.status(self.store)['persistent'], 'an unsynced volume copy is not reported durable')

    def unconfirmed_volume(self):
        """A volume whose directory sync fails while tmpfs refuses the second copy."""
        real_sync, real_write = common.fsync_directory, common.write_private

        def fsync(directory):
            if directory == self.store.persistent:
                raise OSError('input/output error')
            return real_sync(directory)

        def write(directory, name, data):
            if directory == self.store.runtime and name == service.STATE:
                raise OSError('no space left on device')
            return real_write(directory, name, data)
        return fsync, write

    def test_a_rotation_no_storage_confirmed_stays_pending_and_unserved(self):
        self.install()
        _, post = self.fake_github((200, rotated(2)))
        due = chain()['access_expires_at'] - 60
        fsync, write = self.unconfirmed_volume()
        with mock.patch.object(common, 'fsync_directory', side_effect=fsync), \
                mock.patch.object(common, 'write_private', side_effect=write), \
                self.assertRaisesRegex(ValueError, 'confirmed'):
            service.refresh_once(self.store, lambda: due, post)
        rotated_chain = self.store.pending['chain']
        self.assertEqual(self.stored()['chain'], rotated_chain, 'the rotated chain is in place on the volume')
        self.assertIsNone(self.store.load(serving=True)[0], 'an unconfirmed chain is never served')
        self.assertTrue(service.status(self.store)['pending_write'])
        self.assertTrue(self.store.retry_pending())
        self.assertIsNone(self.store.pending, 'its own unconfirmed copy does not supersede it')
        self.assertEqual(self.store.load(serving=True)[0]['chain'], rotated_chain)

    def test_an_unconfirmed_rotation_is_pending_before_readers_look_again(self):
        self.install()
        _, post = self.fake_github((200, rotated(2)))
        due = chain()['access_expires_at'] - 60
        fsync, write = self.unconfirmed_volume()
        held = []
        keep = self.store.keep_pending

        def record(state):
            fd = os.open(self.store.runtime / 'commit.lock', os.O_RDONLY)
            try:
                with self.assertRaises(BlockingIOError):
                    fcntl.flock(fd, fcntl.LOCK_SH | fcntl.LOCK_NB)
                held.append(True)
            finally:
                os.close(fd)
            keep(state)
        with mock.patch.object(common, 'fsync_directory', side_effect=fsync), \
                mock.patch.object(common, 'write_private', side_effect=write), \
                mock.patch.object(self.store, 'keep_pending', side_effect=record), \
                self.assertRaisesRegex(ValueError, 'confirmed'):
            service.refresh_once(self.store, lambda: due, post)
        self.assertEqual(held, [True], 'readers wait until the chain is recorded as pending')

    def test_an_install_no_storage_confirmed_keeps_the_previous_chain(self):
        self.install()
        fsync, write = self.unconfirmed_volume()
        with mock.patch.object(common, 'fsync_directory', side_effect=fsync), \
                mock.patch.object(common, 'write_private', side_effect=write), \
                self.assertRaisesRegex(ValueError, 'confirmed'):
            self.install(installation(chain=chain(access='ghu_synthetic-new')))
        self.assertEqual(self.stored()['chain'], chain(), 'the previous chain is stored again')
        self.assertIsNone(self.store.pending)

    def test_a_serving_read_waits_for_a_write_under_way(self):
        self.install()
        read = []
        with self.store.commit():
            reader = threading.Thread(target=lambda: read.append(self.store.load(serving=True)[0]))
            reader.start()
            reader.join(0.3)
            self.assertTrue(reader.is_alive(), 'the reader waits for the commit')
        reader.join(5)
        self.assertEqual(read[0]['chain'], chain())

    def test_removal_never_follows_a_swapped_directory_link(self):
        victim = self.root / 'elsewhere'
        victim.mkdir()
        (victim / service.STATE).write_text('{}')
        self.store.persistent.parent.mkdir(parents=True, exist_ok=True)
        if self.store.persistent.exists():
            self.store.persistent.rmdir()
        self.store.persistent.symlink_to(victim)
        common.remove_durably(self.store.persistent, service.STATE)
        self.assertTrue((victim / service.STATE).exists(), 'root never removes a file behind a link')

    def test_a_memory_only_chain_is_never_served(self):
        self.install()
        self.store.pending = dict(self.stored(), serial=self.stored()['serial'] + 1,
                                  chain=chain(access='ghu_synthetic-unwritten'))
        self.assertEqual(self.store.load()[0]['chain']['access_token'], 'ghu_synthetic-unwritten')
        self.assertEqual(self.store.load(serving=True)[0]['chain'], chain())

    def test_a_revocation_that_only_memory_holds_still_ends_serving(self):
        self.install()
        self.store.pending = dict(self.stored(), serial=self.stored()['serial'] + 1, state='revoked',
                                  last_error='bad_refresh_token')
        self.assertEqual(self.store.load(serving=True)[0]['state'], 'revoked')

    def test_a_full_tmpfs_leaves_the_older_volume_chain_usable(self):
        self.install()
        old = self.store.persistent / service.STATE
        real = common.write_private

        def write(directory, name, data):
            if directory in (self.store.persistent, self.store.runtime):
                raise OSError('no space left on device')
            return real(directory, name, data)
        with mock.patch.object(common, 'write_private', side_effect=write), \
                self.assertRaisesRegex(ValueError, 'No private GitHub storage'):
            self.install(installation(chain=chain(access='ghu_synthetic-new')))
        self.assertTrue(old.exists(), 'the older chain is not removed before a new one is stored')
        self.assertEqual(self.stored()['chain'], chain())

    def test_tmpfs_refuses_a_chain_while_an_older_volume_chain_stays(self):
        self.install()
        real_unlink = common.os.unlink

        def unlink(path, *args, dir_fd=None, **kwargs):
            if dir_fd is not None and path == service.STATE:
                raise PermissionError('volume refused the removal')
            return real_unlink(path, *args, dir_fd=dir_fd, **kwargs)
        with self.fail_volume_writes(), mock.patch.object(common.os, 'unlink', unlink), \
                self.assertRaisesRegex(ValueError, 'older GitHub token chain'):
            self.install(installation(chain=chain(access='ghu_synthetic-new')))
        self.assertFalse((self.store.runtime / service.STATE).exists())
        self.assertEqual(self.stored()['chain'], chain(), 'the install failed, so the stored chain is unchanged')
