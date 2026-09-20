"""Synthetic credential tests; never use account credentials here."""
import importlib.machinery
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import types
import unittest
from unittest import mock

SCRIPT = Path(__file__).with_name('horizon-worker-git-auth')
loader = importlib.machinery.SourceFileLoader('worker_git_auth', str(SCRIPT))
spec = importlib.util.spec_from_loader(loader.name, loader)
auth = importlib.util.module_from_spec(spec)
loader.exec_module(auth)


class GitAuthenticationTests(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.TemporaryDirectory()
        self.addCleanup(self.root.cleanup)
        self.path = Path(self.root.name)
        self.value = {'repository': 'example/project', 'token': 'synthetic-private-token',
                      'author_name': 'Test User', 'author_email': 'test@example.invalid'}
        self.credential = mock.patch.object(auth, 'CREDENTIAL', self.path / 'credentials/github.json')
        self.credential.start()
        self.addCleanup(self.credential.stop)

    def test_git_receives_token_only_for_exact_https_repository(self):
        for path in ('example/project', 'example/project.git', 'Example/Project.git'):
            reply = auth.credential_reply(self.value, 'protocol=https\nhost=github.com\npath=' + path + '\n')
            self.assertIn('password=synthetic-private-token\n', reply)
        for request in (
            'protocol=http\nhost=github.com\npath=example/project',
            'protocol=https\nhost=github.com.evil.invalid\npath=example/project',
            'protocol=https\nhost=github.com\npath=example/other',
            'protocol=https\nhost=github.com\npath=example/project/submodule',
            'protocol=https\nhost=github.com\n',
        ):
            self.assertEqual(auth.credential_reply(self.value, request), '')

    def test_install_atomic_private_and_clear_removes_future_access(self):
        with mock.patch.object(auth.subprocess, 'run') as commands:
            auth.install(self.value)
        self.assertEqual(auth.CREDENTIAL.stat().st_mode & 0o777, 0o600)
        self.assertEqual(auth.read_binding(), self.value)
        self.assertNotIn(self.value['token'], str(commands.call_args_list))
        self.assertEqual(list(auth.CREDENTIAL.parent.glob('.github-*')), [])
        with mock.patch.object(auth.sys, 'argv', ['horizon-worker-git-auth', 'clear']):
            auth.main()
        self.assertIsNone(auth.read_binding())

    def test_gh_injects_authentication_only_into_child_environment(self):
        with mock.patch.object(auth.subprocess, 'run'):
            auth.install(self.value)
        with mock.patch.dict(auth.os.environ, {'GH_REPO': 'unrelated/project', 'GH_HOST': 'unrelated.invalid'}), \
                mock.patch.object(auth.sys, 'argv', ['gh', 'pr', 'list']), \
                mock.patch.object(auth.os, 'execve') as execute:
            auth.main()
        executable, argv, env = execute.call_args.args
        self.assertEqual(executable, '/usr/bin/gh')
        self.assertNotIn(self.value['token'], str(argv))
        self.assertEqual(env['GH_TOKEN'], self.value['token'])
        self.assertEqual(env['GH_REPO'], 'example/project')
        self.assertEqual(env['GH_HOST'], 'github.com')

    def test_malformed_and_non_private_bindings_are_refused_without_secret_diagnostics(self):
        for key, value in [('repository', '../repo'), ('author_name', 'name\ninjection'),
                           ('token', 'token\nsecret')]:
            candidate = dict(self.value, **{key: value})
            with self.assertRaises(ValueError) as result:
                auth.validate(candidate)
            self.assertNotIn(value, str(result.exception))
        auth.CREDENTIAL.parent.mkdir()
        auth.CREDENTIAL.write_text(json.dumps(self.value))
        auth.CREDENTIAL.chmod(0o644)
        with self.assertRaises(ValueError):
            auth.read_binding()

    def test_non_posix_storage_fails_before_any_token_is_written(self):
        with mock.patch.object(auth.subprocess, 'run'), \
                mock.patch.object(auth.os, 'fstat', return_value=types.SimpleNamespace(st_mode=0o100666)):
            with self.assertRaises(ValueError):
                auth.install(self.value)
        self.assertFalse(auth.CREDENTIAL.exists())
        self.assertEqual(list(auth.CREDENTIAL.parent.iterdir()), [])

    def test_real_git_helper_respects_use_http_path(self):
        # A synthetic helper exercises Git's context passing, not just our parser.
        helper = self.path / 'helper'
        helper.write_text('#!/usr/bin/python3\nimport sys\n'
                          + 'sys.path.insert(0, ' + repr(str(Path(__file__).parent)) + ')\n'
                          + 'from test_git_auth import auth\n'
                          + 'value=' + repr(self.value) + '\n'
                          + "print(auth.credential_reply(value, sys.stdin.read()), end='')\n")
        helper.chmod(0o700)
        env = dict(os.environ, HOME=str(self.path), GIT_CONFIG_NOSYSTEM='1', GIT_TERMINAL_PROMPT='0')
        args = ['git', '-c', 'credential.helper=', '-c', 'credential.helper=' + str(helper),
                '-c', 'credential.useHttpPath=true', 'credential', 'fill']
        matched = subprocess.run(args, input='url=https://github.com/example/project.git\n\n',
                                 text=True, capture_output=True, env=env)
        self.assertEqual(matched.returncode, 0)
        self.assertIn('password=synthetic-private-token', matched.stdout)
        refused = subprocess.run(args, input='url=https://github.com/example/other.git\n\n',
                                 text=True, capture_output=True, env=env)
        self.assertNotEqual(refused.returncode, 0)
        self.assertNotIn(self.value['token'], refused.stdout + refused.stderr)


if __name__ == '__main__':
    unittest.main()
