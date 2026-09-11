#!/usr/bin/env python3
"""Synthetic-only, offline checks for explicit worker credential installation."""

import concurrent.futures
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('credentials', HERE / 'github-credentials.py')
C = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(C)
TOKEN = b'github_pat_SYNTHETIC_INSTALL_TEST'


class InstallTests(unittest.TestCase):
    def setUp(self):
        self.fixture = tempfile.TemporaryDirectory(prefix='horizon-token-install-')
        self.root = Path(self.fixture.name)
        self.runtime = self.root / 'runtime'
        self.runtime.mkdir(mode=0o700)
        self.token = self.runtime / 'github-token'
        self.pending = self.runtime / 'github-token.pending'
        self.runtime_patch = patch.object(C, 'RUNTIME', self.runtime)
        self.runtime_patch.start()

    def tearDown(self):
        self.runtime_patch.stop()
        self.fixture.cleanup()

    def install(self, token=TOKEN):
        return C.install_token(io.BytesIO(token))

    def wrapper(self):
        wrapper = self.root / 'credential'
        source = (HERE / 'github-credentials.py').read_text()
        wrapper.write_text(source.replace("RUNTIME = Path('/run/horizon')",
                                          'RUNTIME = Path(' + repr(str(self.runtime)) + ')'))
        return wrapper

    def test_install_is_private_and_exact_repetition_does_not_write(self):
        self.assertEqual('installed', self.install(TOKEN + b'\n'))
        self.assertEqual(TOKEN.decode(), C.read_token())
        self.assertEqual(0o600, self.token.stat().st_mode & 0o777)
        self.assertEqual(1, self.token.stat().st_nlink)
        self.assertFalse(self.pending.exists())
        before = (C.identity(self.token.stat()), C.identity(self.runtime.stat()))
        self.assertEqual('present', self.install())
        self.assertEqual(before, (C.identity(self.token.stat()), C.identity(self.runtime.stat())))

    def test_different_existing_token_is_never_rotated(self):
        self.install()
        before = self.token.read_bytes(), C.identity(self.token.stat())
        with self.assertRaises(C.CredentialError):
            self.install(b'different_SYNTHETIC_token')
        self.assertEqual(before, (self.token.read_bytes(), C.identity(self.token.stat())))
        self.assertFalse(self.pending.exists())

    def test_malformed_and_oversized_requests_do_not_create_files(self):
        for token in (b'', b'\n', TOKEN + b'\n\n', TOKEN + b'\r\n', b'x\0y', b'private secret',
                      b'\xff', b'x' * (C.MAX_TOKEN + 1)):
            with self.subTest(token_length=len(token)), self.assertRaises(C.CredentialError):
                self.install(token)
            self.assertEqual([], list(self.runtime.iterdir()))

    def test_missing_or_unsafe_runtime_is_never_created_or_repaired(self):
        self.runtime.rmdir()
        with self.assertRaises(OSError):
            self.install()
        self.assertFalse(self.runtime.exists())
        self.runtime.mkdir(mode=0o755)
        with self.assertRaises(C.CredentialError):
            self.install()
        self.assertEqual(0o755, self.runtime.stat().st_mode & 0o777)
        self.runtime.chmod(0o700)
        with patch.object(C.os, 'geteuid', return_value=os.geteuid() + 1):
            with self.assertRaises(C.CredentialError):
                self.install()
        self.assertEqual([], list(self.runtime.iterdir()))

    def test_symlink_runtime_and_final_file_are_rejected(self):
        alias = self.root / 'alias'
        alias.symlink_to(self.runtime, target_is_directory=True)
        with patch.object(C, 'RUNTIME', alias), self.assertRaises(OSError):
            self.install()
        target = self.root / 'outside'
        target.write_bytes(TOKEN)
        target.chmod(0o600)
        self.token.symlink_to(target)
        with self.assertRaises(OSError):
            self.install()
        self.assertEqual(TOKEN, target.read_bytes())
        self.assertFalse(self.pending.exists())

    def test_interrupted_claim_is_not_replayed_or_removed(self):
        self.pending.write_bytes(b'partial_SYNTHETIC')
        self.pending.chmod(0o600)
        before = self.pending.read_bytes(), C.identity(self.pending.stat())
        with self.assertRaises(FileExistsError):
            self.install()
        self.assertEqual(before, (self.pending.read_bytes(), C.identity(self.pending.stat())))
        self.assertFalse(self.token.exists())

    def test_write_failure_retains_private_claim_without_publication(self):
        with patch.object(C.os, 'write', side_effect=OSError('private sentinel')):
            with self.assertRaises(OSError):
                self.install()
        self.assertEqual(0o600, self.pending.stat().st_mode & 0o777)
        self.assertFalse(self.token.exists())
        with self.assertRaises(FileExistsError):
            self.install()

    def test_short_writes_complete_before_publication(self):
        write = os.write
        with patch.object(C.os, 'write', side_effect=lambda fd, data: write(fd, data[:3])):
            self.assertEqual('installed', self.install())
        self.assertEqual(TOKEN, self.token.read_bytes())

    def test_publication_never_overwrites_a_concurrent_final_file(self):
        link = os.link
        other = b'other_SYNTHETIC_token'
        def insert(*args, **kwargs):
            self.token.write_bytes(other)
            self.token.chmod(0o600)
            return link(*args, **kwargs)
        with patch.object(C.os, 'link', side_effect=insert), self.assertRaises(FileExistsError):
            self.install()
        self.assertEqual(other, self.token.read_bytes())
        self.assertEqual(TOKEN, self.pending.read_bytes())

    def test_failed_unlink_is_unknown_not_a_readable_credential(self):
        with patch.object(C.os, 'unlink', side_effect=OSError('synthetic unlink failure')):
            with self.assertRaises(OSError):
                self.install()
        self.assertEqual(2, self.token.stat().st_nlink)
        with self.assertRaises(C.CredentialError):
            C.read_token()
        with self.assertRaises(C.CredentialError):
            self.install()

    def test_cli_receipts_and_errors_never_contain_credentials(self):
        wrapper = self.wrapper()
        for token, expected, status in ((TOKEN, 0, 'installed'), (TOKEN, 0, 'present'),
                                        (b'different_SYNTHETIC', 1, None), (b'bad secret', 1, None)):
            result = subprocess.run(['/usr/bin/python3', '-I', str(wrapper), 'install'], input=token,
                                    capture_output=True, timeout=5, check=False)
            self.assertEqual(expected, result.returncode)
            self.assertNotIn(token, result.stdout + result.stderr)
            if status:
                self.assertEqual({'version': 1, 'status': status}, json.loads(result.stdout))
                self.assertEqual(b'', result.stderr)
            else:
                self.assertEqual(b'', result.stdout)
                self.assertEqual(b'horizon-worker: GitHub credential unavailable or request rejected\n', result.stderr)

    def test_concurrent_processes_publish_only_one_token(self):
        wrapper = self.wrapper()
        tokens = [TOKEN + str(index).encode() for index in range(4)]
        def install(token):
            return subprocess.run(['/usr/bin/python3', '-I', str(wrapper), 'install'], input=token,
                                  capture_output=True, timeout=5, check=False)
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
            results = list(pool.map(install, tokens))
        self.assertLessEqual(sum(result.returncode == 0 for result in results), 1)
        winner = self.token.read_bytes()
        self.assertIn(winner, tokens)
        self.assertEqual('present', self.install(winner))
        if self.pending.exists():
            self.assertIn(self.pending.read_bytes(), tokens)
            self.assertNotEqual(winner, self.pending.read_bytes())
            self.assertEqual(0o600, self.pending.stat().st_mode & 0o777)
            self.assertEqual(1, self.pending.stat().st_nlink)
        for token in tokens:
            self.assertTrue(all(token not in result.stdout + result.stderr for result in results))

    def test_delayed_loser_retains_private_candidate_without_replacing_winner(self):
        open_file = os.open
        other = b'other_SYNTHETIC_winner'
        raced = False
        def delayed_open(name, *args, **kwargs):
            nonlocal raced
            if name == 'github-token.pending' and not raced:
                raced = True
                self.assertEqual('installed', self.install(other))
            return open_file(name, *args, **kwargs)
        with patch.object(C.os, 'open', side_effect=delayed_open), self.assertRaises(FileExistsError):
            self.install()
        self.assertEqual(other, self.token.read_bytes())
        self.assertEqual(TOKEN, self.pending.read_bytes())
        before = C.identity(self.pending.stat())
        self.assertEqual('present', self.install(other))
        self.assertEqual(before, C.identity(self.pending.stat()))

    def test_boot_clears_runtime_claim_but_does_not_install_implicitly(self):
        entrypoint = (HERE / 'entrypoint.sh').read_text()
        self.assertIn('rm -f /run/horizon/github-token /run/horizon/github-token.pending', entrypoint)
        self.assertNotIn('horizon-github-credential install', entrypoint)


if __name__ == '__main__':
    unittest.main()
