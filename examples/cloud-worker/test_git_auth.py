"""Synthetic credential tests; never use account credentials here."""
import importlib.machinery
import importlib.util
import io
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
            reply = auth.credential_reply([self.value], 'protocol=https\nhost=github.com\npath=' + path + '\n')
            self.assertIn('password=synthetic-private-token\n', reply)
        for request in (
            'protocol=http\nhost=github.com\npath=example/project',
            'protocol=https\nhost=github.com.evil.invalid\npath=example/project',
            'protocol=https\nhost=github.com\npath=example/other',
            'protocol=https\nhost=github.com\npath=example/project/submodule',
            'protocol=https\nhost=github.com\n',
        ):
            self.assertEqual(auth.credential_reply([self.value], request), '')

    def test_install_atomic_private_and_clear_removes_future_access(self):
        with mock.patch.object(auth.subprocess, 'run') as commands:
            auth.install(self.value)
        self.assertEqual(auth.CREDENTIAL.stat().st_mode & 0o777, 0o600)
        self.assertEqual(auth.read_grants(), (1, [dict(self.value, target='primary')]))
        self.assertNotIn(self.value['token'], str(commands.call_args_list))
        self.assertEqual(list(auth.CREDENTIAL.parent.glob('.github-*')), [])
        with mock.patch.object(auth.sys, 'argv', ['horizon-worker-git-auth', 'clear']):
            auth.main()
        self.assertIsNone(auth.read_grants())

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
            auth.read_grants()

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
                          + "print(auth.credential_reply([value], sys.stdin.read()), end='')\n")
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



def git(*args, cwd=None, env=None):
    return subprocess.run(['git', *args], cwd=cwd, env=env, check=True, capture_output=True, text=True).stdout.strip()


class GitGrantTests(unittest.TestCase):
    """Version 2: several repositories on one worker, each with its own grant."""

    def setUp(self):
        self.root = tempfile.TemporaryDirectory()
        self.addCleanup(self.root.cleanup)
        self.path = Path(self.root.name)
        self.env = dict(os.environ, HOME=str(self.path / 'home'), GIT_CONFIG_NOSYSTEM='1',
                        GIT_TERMINAL_PROMPT='0')
        (self.path / 'home').mkdir()
        for name, value in [('CREDENTIAL', self.path / 'credentials/github.json'),
                            ('GIT_DIR', self.path / 'repository.git'),
                            ('SIBLINGS', self.path / 'siblings'), ('HOME', str(self.path / 'home'))]:
            patcher = mock.patch.object(auth, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)
        environment = mock.patch.dict(auth.os.environ, {'HOME': str(self.path / 'home'),
                                                        'GIT_CONFIG_NOSYSTEM': '1'})
        environment.start()
        self.addCleanup(environment.stop)
        self.primary = {'repository': 'example/consumer', 'token': 'synthetic-consumer-token',
                        'author_name': 'Consumer Author', 'author_email': 'consumer@example.invalid',
                        'target': 'primary'}
        self.sibling = {'repository': 'example/library', 'token': 'synthetic-library-token',
                        'author_name': 'Library Author', 'author_email': 'library@example.invalid',
                        'target': 'sibling:library'}
        self.value = {'version': 2, 'grants': [self.primary, self.sibling]}

    def bare(self, directory):
        """A bare repository with one commit and a linked agent worktree, as the worker creates them."""
        seed = self.path / ('seed-' + directory.parent.name)
        git('init', '-q', '-b', 'base', str(seed), env=self.env)
        git('-c', 'user.name=Seed', '-c', 'user.email=seed@example.invalid', 'commit', '-q',
            '--allow-empty', '-m', 'seed', cwd=seed, env=self.env)
        directory.parent.mkdir(parents=True, exist_ok=True)
        git('clone', '-q', '--bare', str(seed), str(directory), env=self.env)
        git('config', '--unset', 'remote.origin.url', cwd=directory, env=self.env)
        worktree = self.path / 'agents' / directory.parent.name
        git('--git-dir=' + str(directory), 'worktree', 'add', '-q', '-b', 'agent', str(worktree), 'base',
            env=self.env)
        return worktree

    def test_install_configures_each_repository_and_its_worktrees(self):
        primary = self.bare(auth.GIT_DIR)
        library = self.bare(auth.SIBLINGS / 'library/repository.git')
        auth.install(self.value)
        self.assertEqual(auth.CREDENTIAL.stat().st_mode & 0o777, 0o600)
        self.assertEqual(auth.read_grants(), (2, self.value['grants']))
        for worktree, grant in [(primary, self.primary), (library, self.sibling)]:
            self.assertEqual(git('remote', 'get-url', 'origin', cwd=worktree, env=self.env),
                             'https://github.com/' + grant['repository'] + '.git')
            self.assertEqual(git('config', 'user.name', cwd=worktree, env=self.env), grant['author_name'])
            self.assertEqual(git('config', 'user.email', cwd=worktree, env=self.env), grant['author_email'])
        global_config = (self.path / 'home/.gitconfig').read_text()
        self.assertIn('useHttpPath = true', global_config)
        self.assertNotIn('Author', global_config)
        for grant in self.value['grants']:
            self.assertNotIn(grant['token'], global_config)
            for config in self.path.rglob('config'):
                self.assertNotIn(grant['token'], config.read_text())

    def test_missing_sibling_repository_is_refused_before_any_write(self):
        self.bare(auth.GIT_DIR)
        with mock.patch.object(auth.subprocess, 'run') as commands, self.assertRaises(ValueError):
            auth.install(self.value)
        commands.assert_not_called()
        self.assertFalse(auth.CREDENTIAL.exists())
        elsewhere = self.path / 'elsewhere'
        self.bare(elsewhere / 'repository.git')
        auth.SIBLINGS.mkdir()
        (auth.SIBLINGS / 'library').symlink_to(elsewhere)
        with mock.patch.object(auth.subprocess, 'run') as commands, self.assertRaises(ValueError):
            auth.install(self.value)
        commands.assert_not_called()
        self.assertFalse(auth.CREDENTIAL.exists())

    def test_target_that_is_not_a_bare_repository_is_refused_before_any_write(self):
        primary = self.bare(auth.GIT_DIR)
        (auth.SIBLINGS / 'library/repository.git').mkdir(parents=True)
        with self.assertRaises(ValueError):
            auth.install(self.value)
        self.assertFalse(auth.CREDENTIAL.exists())
        self.assertFalse((self.path / 'home/.gitconfig').exists())
        self.assertEqual(subprocess.run(['git', 'remote', 'get-url', 'origin'], cwd=primary, env=self.env,
                                        capture_output=True).returncode, 2)

    def test_version_2_removes_only_the_global_identity_version_1_wrote(self):
        self.bare(auth.GIT_DIR)
        self.bare(auth.SIBLINGS / 'library/repository.git')
        auth.install({key: self.primary[key] for key in auth.FIELDS})
        git('config', '--global', 'user.email', 'chosen@example.invalid', env=self.env)
        auth.install(self.value)
        self.assertEqual(subprocess.run(['git', 'config', '--global', 'user.name'], env=self.env,
                                        capture_output=True).returncode, 1)
        self.assertEqual(git('config', '--global', 'user.email', env=self.env), 'chosen@example.invalid')

    def test_version_1_binding_still_installs_for_the_primary(self):
        primary = self.bare(auth.GIT_DIR)
        legacy = {key: self.primary[key] for key in auth.FIELDS}
        auth.install(legacy)
        self.assertEqual(auth.read_grants(), (1, [self.primary]))
        self.assertEqual(json.loads(auth.CREDENTIAL.read_text()), legacy)
        self.assertEqual(git('config', '--global', 'user.name', env=self.env), 'Consumer Author')
        self.assertEqual(git('remote', 'get-url', 'origin', cwd=primary, env=self.env),
                         'https://github.com/example/consumer.git')

    def test_each_path_receives_only_its_own_token(self):
        grants = self.value['grants']
        for path, token, other in [('example/consumer.git', self.primary['token'], self.sibling['token']),
                                   ('Example/Library', self.sibling['token'], self.primary['token'])]:
            reply = auth.credential_reply(grants, 'protocol=https\nhost=github.com\npath=' + path + '\n')
            self.assertIn('password=' + token + '\n', reply)
            self.assertNotIn(other, reply)
        for request in ('protocol=https\nhost=github.com\npath=example/other',
                        'protocol=https\nhost=github.com\npath=example/\u212aonsumer',
                        'protocol=https\nhost=github.com\npath=example/library/extra',
                        'protocol=https\nhost=github.com\n',
                        'protocol=http\nhost=github.com\npath=example/library'):
            self.assertEqual(auth.credential_reply(grants, request), '')

    def test_git_helper_answers_from_the_installed_grants(self):
        self.bare(auth.GIT_DIR)
        self.bare(auth.SIBLINGS / 'library/repository.git')
        auth.install(self.value)
        for url, token in [('https://github.com/example/library.git', self.sibling['token']),
                           ('https://github.com/example/consumer.git', self.primary['token']),
                           ('https://github.com/example/other.git', None)]:
            request = 'protocol=https\nhost=github.com\npath=' + url.removeprefix('https://github.com/') + '\n'
            output = io.StringIO()
            with mock.patch.object(auth.sys, 'argv', ['horizon-worker-git-auth', 'get']), \
                    mock.patch.object(auth.sys, 'stdin', io.StringIO(request)), \
                    mock.patch.object(auth.sys, 'stdout', output):
                auth.main()
            for grant in self.value['grants']:
                self.assertEqual(grant['token'] in output.getvalue(), grant['token'] == token)

    def gh(self, env, argv=(), cwd=None):
        stderr = io.StringIO()
        previous = os.getcwd()
        os.chdir(cwd or self.path)
        try:
            with mock.patch.object(auth.sys, 'stderr', stderr):
                result = auth.gh_environment((2, self.value['grants']), dict(self.env, **env), argv)
        finally:
            os.chdir(previous)
        for grant in self.value['grants']:
            self.assertNotIn(grant['token'], stderr.getvalue())
        return result, stderr.getvalue()

    def test_gh_uses_the_working_directory_origin(self):
        primary = self.bare(auth.GIT_DIR)
        library = self.bare(auth.SIBLINGS / 'library/repository.git')
        auth.install(self.value)
        for worktree, grant in [(primary, self.primary), (library, self.sibling)]:
            env, message = self.gh({}, cwd=worktree)
            self.assertEqual((env['GH_TOKEN'], env['GH_REPO'], env['GH_HOST']),
                             (grant['token'], grant['repository'], 'github.com'))
            self.assertEqual(message, '')
        env, message = self.gh({}, cwd=self.path)
        self.assertNotIn('GH_TOKEN', env)
        self.assertIn('no Git grant', message)
        self.assertEqual(message.count('\n'), 1)

    def test_gh_prefers_repo_flag_then_gh_repo_and_never_crosses_hosts(self):
        primary = self.bare(auth.GIT_DIR)
        env, _ = self.gh({'GH_REPO': 'example/library'}, cwd=primary)
        self.assertEqual(env['GH_TOKEN'], self.sibling['token'])
        env, _ = self.gh({'GH_REPO': 'github.com/Example/Library'})
        self.assertEqual(env['GH_TOKEN'], self.sibling['token'])
        for argv in (['pr', 'list', '-R', 'example/consumer'], ['pr', 'list', '--repo=example/consumer'],
                     ['pr', 'list', '-Rexample/consumer'], ['pr', 'list', '--repo', 'example/consumer'],
                     ['issue', 'view', '1', '-cR', 'example/consumer'], ['pr', 'list', '-R=example/consumer'],
                     ['pr', 'list', '-R', 'Example/Consumer', '--repo', 'example/consumer']):
            env, _ = self.gh({'GH_REPO': 'example/library'}, argv)
            self.assertEqual(env['GH_TOKEN'], self.primary['token'])
        for extra, argv in [({'GH_REPO': 'example/unrelated'}, ()), ({'GH_REPO': 'other.invalid/example/library'}, ()),
                            ({'GH_REPO': 'example/library', 'GH_HOST': 'other.invalid'}, ()),
                            ({'GH_REPO': 'example/library'}, ['pr', 'list', '-R', 'example/unrelated']),
                            ({}, ['pr', 'list', '--repo']),
                            ({}, ['pr', 'list', '-R', 'example/library', '--repo', 'example/consumer']),
                            ({}, ['issue', 'create', '--title', '--', '--repo', 'example/unrelated']),
                            ({}, ['issue', 'view', '1', '-cR', 'example/unrelated']),
                            ({}, ['pr', 'view', 'https://github.com/example/unrelated/pull/1']),
                            ({}, ['pr', 'list', '-R', 'example/\u212aonsumer']),
                            ({}, ['api', '--hostname', 'other.invalid', 'repos/example/consumer']),
                            ({}, ['api', '--hostname=other.invalid', 'user']),
                            ({}, ['pr', 'view', 'https://other.invalid/example/consumer/pull/1']),
                            ({}, ['pr', 'view', 'https://github.com@other.invalid/example/consumer/pull/1'])]:
            env, message = self.gh(extra, argv, cwd=primary)
            self.assertNotIn('GH_TOKEN', env)
            self.assertIn('no Git grant', message)

    def test_gh_arguments_that_imply_a_repository_must_agree_with_the_resolved_one(self):
        primary = self.bare(auth.GIT_DIR)
        git('config', 'remote.origin.url', 'https://github.com/example/consumer.git', cwd=primary, env=self.env)
        for argv in (['pr', 'view', 'https://github.com/example/consumer/pull/1'],
                     ['repo', 'view', 'example/consumer'], ['api', 'repos/example/consumer/pulls'],
                     ['api', '/repos/{owner}/{repo}/pulls'], ['repo', 'clone', 'example/consumer', 'a/b/c']):
            env, _ = self.gh({}, argv, cwd=primary)
            self.assertEqual(env['GH_TOKEN'], self.primary['token'], argv)
        for argv in (['repo', 'view', 'example/library'], ['repo', 'clone', 'example/library'],
                     ['api', 'repos/example/library/contents/x'],
                     ['pr', 'view', 'https://github.com/example/library/pull/1'],
                     ['pr', 'create', '--body', 'https://github.com/example/library/pull/3'],
                     ['repo', 'view', 'library']):
            env, message = self.gh({}, argv, cwd=primary)
            self.assertNotIn('GH_TOKEN', env, argv)
            self.assertIn('no Git grant', message)
        env, _ = self.gh({}, ['repo', 'clone', 'example/library'], cwd=self.path)
        self.assertEqual((env['GH_TOKEN'], env['GH_REPO']), (self.sibling['token'], 'example/library'))
        env, _ = self.gh({'GH_REPO': 'example/library'}, ['pr', 'view', 'https://github.com/example/consumer/pull/1'])
        self.assertNotIn('GH_TOKEN', env)

    def test_gh_keeps_github_api_urls_and_drops_inherited_horizon_tokens(self):
        primary = self.bare(auth.GIT_DIR)
        git('config', 'remote.origin.url', 'https://github.com/example/consumer.git', cwd=primary, env=self.env)
        env, _ = self.gh({}, ['api', 'https://api.github.com/user'], cwd=primary)
        self.assertEqual(env['GH_TOKEN'], self.primary['token'])
        env, _ = self.gh({}, ['api', '--hostname', 'github.com', 'user'], cwd=primary)
        self.assertEqual(env['GH_TOKEN'], self.primary['token'])
        inherited = {'GH_TOKEN': self.primary['token'], 'GITHUB_TOKEN': self.sibling['token'],
                     'GH_REPO': 'example/unrelated'}
        env, message = self.gh(inherited, cwd=primary)
        self.assertNotIn('GH_TOKEN', env)
        self.assertNotIn('GITHUB_TOKEN', env)
        self.assertIn('no Git grant', message)
        env, _ = self.gh({'GH_TOKEN': 'caller-own-token', 'GH_REPO': 'example/unrelated'}, cwd=primary)
        self.assertEqual(env['GH_TOKEN'], 'caller-own-token')

    def test_version_1_after_version_2_drops_the_repository_identity_it_wrote(self):
        primary = self.bare(auth.GIT_DIR)
        library = self.bare(auth.SIBLINGS / 'library/repository.git')
        auth.install(self.value)
        git('config', 'user.email', 'chosen@example.invalid', cwd=library, env=self.env)
        legacy = dict({key: self.primary[key] for key in auth.FIELDS}, author_name='Current Author')
        auth.install(legacy)
        self.assertEqual(git('config', 'user.name', cwd=primary, env=self.env), 'Current Author')
        self.assertEqual(git('config', 'user.name', cwd=library, env=self.env), 'Current Author')
        self.assertEqual(git('config', 'user.email', cwd=library, env=self.env), 'chosen@example.invalid')

    def test_limits_unknown_keys_and_duplicates_are_rejected(self):
        many = [dict(self.primary, repository='example/r' + str(index), target='sibling:s' + str(index))
                for index in range(17)]
        for value in (
            {'version': 2, 'grants': many},
            {'version': 2, 'grants': []},
            {'version': 3, 'grants': [self.primary]},
            {'version': True, 'grants': [self.primary]},
            {'version': '2', 'grants': [self.primary]},
            {'version': 2, 'grants': [self.primary], 'extra': 1},
            {'version': 2, 'grants': [dict(self.primary, helper='x')]},
            {'version': 2, 'grants': [{key: self.primary[key] for key in auth.FIELDS}]},
            {'version': 2, 'grants': [self.primary, dict(self.sibling, repository='Example/Consumer')]},
            {'version': 2, 'grants': [self.primary, dict(self.sibling, target='primary')]},
            dict(self.primary),
        ):
            with self.assertRaises(ValueError):
                auth.grants(value)
        for target in ('sibling:', 'sibling:Library', 'sibling:../x', 'sibling:a/b', 'secondary', 5):
            with self.assertRaises(ValueError):
                auth.grants({'version': 2, 'grants': [dict(self.sibling, target=target)]})
        with self.assertRaises(ValueError):
            auth.parse('{"version": 2, "version": 2, "grants": []}')
        self.assertEqual(auth.grants({'version': 2, 'grants': many[:16]})[0], 2)

    def test_largest_accepted_payload_fits_the_private_file(self):
        identity = '\U0001F600' * 50
        grants = [{'repository': 'o' * 100 + '/' + 'r' * 98 + '%02d' % index, 'token': 't' * 2048,
                   'author_name': identity, 'author_email': identity,
                   'target': 'sibling:' + 's' * 62 + '%02d' % index} for index in range(16)]
        grants[0]['target'] = 'primary'
        value = {'version': 2, 'grants': grants}
        auth.grants(value)
        auth.write_private(value)
        self.assertLessEqual(auth.CREDENTIAL.stat().st_size, auth.MAX_BYTES)
        self.assertEqual(auth.read_grants(), (2, grants))

    def test_symlinked_or_shared_credential_files_are_refused(self):
        auth.write_private(self.value)
        link = self.path / 'link.json'
        link.symlink_to(auth.CREDENTIAL)
        with mock.patch.object(auth, 'CREDENTIAL', link), self.assertRaises(OSError):
            auth.read_grants()
        auth.CREDENTIAL.chmod(0o640)
        with self.assertRaises(ValueError):
            auth.read_grants()


if __name__ == '__main__':
    unittest.main()
