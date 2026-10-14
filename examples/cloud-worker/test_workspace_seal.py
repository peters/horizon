"""Startup metadata protection with descriptor races and secret-free diagnostics."""
import importlib.machinery
import importlib.util
import io
import os
from pathlib import Path
import runpy
import stat
import subprocess
import tempfile
import unittest
from unittest.mock import patch

loader = importlib.machinery.SourceFileLoader('workspace_seal', str(Path(__file__).with_name('horizon-worker-tailnet')))
spec = importlib.util.spec_from_loader(loader.name, loader)
worker = importlib.util.module_from_spec(spec)
loader.exec_module(worker)
SECRET = 'tskey-auth-do-not-print-synthetic0123456789'


class WorkspaceSealTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.workspace = Path(self.temp.name) / 'workspace'
        self.workspace.mkdir()
        self.owner = 0
        real_path, real_fstat, real_lstat = Path, os.fstat, Path.lstat
        def path(*parts):
            value = real_path(*parts)
            return self.workspace if value == real_path('/workspace') else value
        def owned(info):
            values = list(info); values[4] = self.owner
            return os.stat_result(values)
        for mock in [patch.object(worker, 'Path', side_effect=path), patch.object(worker.os, 'geteuid', return_value=0), patch.object(worker.os, 'fstat', side_effect=lambda fd: owned(real_fstat(fd))), patch.object(Path, 'lstat', autospec=True, side_effect=lambda path, *a, **kw: owned(real_lstat(path, *a, **kw)))]:
            mock.start(); self.addCleanup(mock.stop)

    def refused(self, code):
        with self.assertRaises(worker.WorkerStorageRequired) as caught:
            worker.prepare_workspace()
        self.assertEqual(caught.exception.code, code)

    def test_empty_writable_seal_is_metadata_only_and_retry_preserves_identity(self):
        self.workspace.chmod(0o777)
        before = self.workspace.stat()
        worker.prepare_workspace()
        after = self.workspace.stat()
        self.assertEqual(stat.S_IMODE(after.st_mode), 0o755)
        self.assertEqual((before.st_uid, before.st_gid, before.st_dev, before.st_ino), (after.st_uid, after.st_gid, after.st_dev, after.st_ino))
        self.assertEqual(list(self.workspace.iterdir()), [])
        with patch.object(worker.os, 'fchmod') as chmod:
            worker.prepare_workspace()
        chmod.assert_not_called()

    def test_retained_safe_root_unchanged_and_writable_root_never_adopted(self):
        (self.workspace / '.horizon-tailnet').mkdir(mode=0o700)
        key = self.workspace / '.horizon-tailnet/selection'; key.write_text('retained')
        for mode in [0o755, 0o777]:
            self.workspace.chmod(mode)
            with self.subTest(mode=mode), patch.object(worker.os, 'fchmod') as chmod:
                if mode == 0o755: worker.prepare_workspace()
                else: self.refused('retained-writable')
            chmod.assert_not_called()
            self.assertEqual(key.read_text(), 'retained')
            self.assertEqual(stat.S_IMODE(self.workspace.stat().st_mode), mode)

    def test_other_owner_symlink_and_regular_file_refuse_without_mutation(self):
        self.owner = 10001
        with patch.object(worker.os, 'fchmod') as chmod:
            self.refused('workspace-owner')
        chmod.assert_not_called(); self.owner = 0
        target = self.workspace.parent / 'target'; target.mkdir(); target.chmod(0o777)
        self.workspace.rmdir(); self.workspace.symlink_to(target, target_is_directory=True)
        self.refused('workspace-type')
        self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o777)
        self.workspace.unlink(); self.workspace.write_text('unchanged')
        self.refused('workspace-type'); self.assertEqual(self.workspace.read_text(), 'unchanged')

    def test_ignored_and_failed_chmod_refuse(self):
        self.workspace.chmod(0o777)
        with patch.object(worker.os, 'fchmod'):
            self.refused('mode-enforcement')
        with patch.object(worker.os, 'fchmod', side_effect=PermissionError(SECRET)):
            self.refused('storage-operation')
        self.assertEqual(stat.S_IMODE(self.workspace.stat().st_mode), 0o777)

    def test_private_control_directory_with_ignored_chmod_remains_rejected(self):
        private = self.workspace / '.horizon-tailnet'
        private.mkdir(); private.chmod(0o777)
        (private / 'selection').write_text('retained')
        with patch.object(worker.os, 'chmod'):
            with self.assertRaises(worker.WorkerStorageRequired) as caught:
                worker.private_directory(private)
        self.assertEqual(caught.exception.code, 'private-directory')
        self.assertEqual(stat.S_IMODE(private.stat().st_mode), 0o777)
        self.assertEqual((private / 'selection').read_text(), 'retained')

    def test_replacement_between_open_and_initial_snapshot_never_chmods_other_inode(self):
        self.workspace.chmod(0o777)
        held = self.workspace.parent / 'held'; real_open = os.open
        def raced(*args, **kwargs):
            fd = real_open(*args, **kwargs)
            self.workspace.rename(held); self.workspace.mkdir(); self.workspace.chmod(0o777)
            return fd
        with patch.object(worker.os, 'open', side_effect=raced), patch.object(worker.os, 'fchmod') as chmod:
            self.refused('workspace-changed')
        chmod.assert_not_called(); self.assertEqual(stat.S_IMODE(held.stat().st_mode), 0o777)

    def test_new_entry_or_replacement_during_chmod_refuses(self):
        real_chmod = os.fchmod
        for replace in [False, True]:
            self.workspace.chmod(0o777)
            for entry in self.workspace.iterdir(): entry.unlink()
            def raced(fd, mode):
                real_chmod(fd, mode)
                if replace:
                    self.workspace.rename(self.workspace.parent / 'old'); self.workspace.mkdir()
                else: (self.workspace / 'raced-entry').write_text('untrusted')
            with self.subTest(replace=replace), patch.object(worker.os, 'fchmod', side_effect=raced):
                self.refused('workspace-changed' if replace else 'concurrent-writer')

    def test_entrypoint_fixed_diagnostic_contains_no_exception_or_environment_key(self):
        helper = Path(__file__).with_name('horizon-worker-tailnet'); real_path = Path
        def path(*parts):
            value = real_path(*parts)
            return self.workspace if value == real_path('/workspace') else value
        self.workspace.chmod(0o777)
        with patch('pathlib.Path', side_effect=path), patch.object(worker.sys, 'argv', ['worker', 'prepare-workspace']), patch('os.fchmod', side_effect=PermissionError(SECRET)), patch('sys.stderr', new_callable=io.StringIO) as errors, patch('sys.stdout', new_callable=io.StringIO) as output, patch.dict(os.environ, {'TAILSCALE_AUTH_KEY': SECRET}):
            with self.assertRaises(SystemExit) as stopped:
                runpy.run_path(str(helper), run_name='__main__')
        self.assertEqual(stopped.exception.code, 1)
        self.assertIn('code=storage-operation', errors.getvalue())
        self.assertNotIn(SECRET, errors.getvalue() + output.getvalue())
        self.assertEqual(output.getvalue(), '')
        error = worker.WorkerStorageRequired(SECRET)
        self.assertNotIn(SECRET, error.diagnostic() + str(error))


class StartupOrderTests(unittest.TestCase):
    def test_failed_storage_prerequisite_stops_both_boot_paths_before_allocation_or_ssh(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            log = root / 'called'
            tailnet = root / 'horizon-worker-tailnet'
            tailnet.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$CALL_LOG"\nexit 23\n')
            tailnet.chmod(0o755)
            unexpected = root / 'horizon-cloud-worker'
            unexpected.write_text('#!/bin/sh\nprintf "allocation-called\\n" >> "$CALL_LOG"\nexit 99\n')
            unexpected.chmod(0o755)
            for startup in [False, True]:
                environment = {'PATH': str(root) + ':/usr/bin:/bin', 'PUBLIC_KEY': 'synthetic-public-key', 'CALL_LOG': str(log)}
                if startup: environment['HORIZON_WORKER_STARTUP'] = 'synthetic-startup'
                result = subprocess.run(['/bin/bash', str(Path(__file__).with_name('horizon-worker-start'))], env=environment, capture_output=True, timeout=10)
                self.assertEqual(result.returncode, 23)
                self.assertEqual(log.read_text(), 'prepare-workspace\n')
                log.unlink()

    def test_seal_precedes_both_branches_workspace_content_and_ssh(self):
        source = Path(__file__).with_name('horizon-worker-start').read_text()
        seal = source.index('horizon-worker-tailnet prepare-workspace')
        for next_step in ['if [[ ${HORIZON_WORKER_STARTUP+x} ]]', 'horizon-cloud-worker prepare-allocation-ssh', 'mkdir -p /workspace', 'horizon-worker-tailnet isolate', 'exec /usr/sbin/sshd', 'exec horizon-worker-supervise']:
            self.assertLess(seal, source.index(next_step), next_step)
        self.assertLess(source.index('${PUBLIC_KEY:?'), seal)
        self.assertIn('set -euo pipefail', source)
        self.assertIn('no repository/application content is initialized', source)


if __name__ == '__main__':
    unittest.main()
