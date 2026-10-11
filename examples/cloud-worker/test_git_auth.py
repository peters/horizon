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
        # No chain service answers, so the helper uses its private file.
        service = mock.patch.object(auth, 'SERVICE_SOCKET', self.path / 'no-service.sock')
        service.start()
        self.addCleanup(service.stop)

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

    def test_under_agent_isolation_root_keeps_the_token_and_agents_keep_only_identities(self):
        root_file = self.path / 'root/static-binding.json'
        identity = auth.identity_path()
        steps = []

        def agent(operation, value):
            steps.append((operation, value))
            # The earlier image's token file is out of the agents' reach while the agent works.
            self.assertFalse(auth.CREDENTIAL.exists())
            with mock.patch.object(auth, 'root_holds_token', return_value=False), \
                    mock.patch.object(auth.subprocess, 'run'), \
                    mock.patch.object(auth, 'AGENT_ISOLATION', self.path / 'isolated'):
                (self.path / 'isolated').touch()
                auth.install_identities(value['binding'], value['previous'])
        # A token file that an earlier image left in the agents' directory.
        auth.write_private(self.value, auth.CREDENTIAL)
        with mock.patch.object(auth, 'ROOT_CREDENTIAL', root_file), \
                mock.patch.object(auth, 'root_holds_token', return_value=True), \
                mock.patch.object(auth, 'AGENT_UID', os.getuid()):
            auth.install_as_root(self.value, agent)
        self.assertFalse(auth.CREDENTIAL.exists())
        self.assertEqual(steps[0][1]['previous'], {key: self.value[key] for key in auth.IDENTITY_FIELDS},
                         'root hands the agent the earlier identities without the token')
        with mock.patch.object(auth, 'ROOT_CREDENTIAL', root_file), \
                mock.patch.object(auth, 'root_holds_token', return_value=True):
            self.assertEqual(auth.read_grants()[1][0]['token'], self.value['token'], 'root reads the token')
        self.assertEqual([operation for operation, _ in steps], ['install-identity'])
        self.assertNotIn(self.value['token'], json.dumps(steps))
        self.assertEqual(root_file.stat().st_mode & 0o777, 0o600)
        self.assertNotIn(self.value['token'], identity.read_text())
        # The agent's view: no token, but the identities that the next install forgets.
        self.assertIsNone(auth.read_grants())
        self.assertEqual(auth.previous_grants()[1][0]['author_name'], 'Test User')
        with mock.patch.object(auth, 'ROOT_CREDENTIAL', root_file), \
                mock.patch.object(auth, 'root_holds_token', return_value=True):
            auth.clear()
        self.assertFalse(root_file.exists())
        self.assertFalse(identity.exists())
        # Also when the isolation marker is gone, clear removes a root-only token.
        auth.write_private(self.value, root_file)
        with mock.patch.object(auth, 'ROOT_CREDENTIAL', root_file), \
                mock.patch.object(auth, 'root_holds_token', return_value=False):
            auth.clear()
        self.assertFalse(root_file.exists())
        # An agent's directory in place of a file does not keep the root token from going.
        auth.write_private(self.value, root_file)
        identity.mkdir()
        with mock.patch.object(auth, 'ROOT_CREDENTIAL', root_file), \
                mock.patch.object(auth, 'root_holds_token', return_value=True):
            auth.clear()
        self.assertFalse(root_file.exists())

    def test_an_earlier_token_file_is_emptied_with_its_hard_links_before_the_agent_works(self):
        auth.write_private(self.value, auth.CREDENTIAL)
        linked = auth.CREDENTIAL.with_name('copy.json')
        os.link(auth.CREDENTIAL, linked)
        root_file = self.path / 'root/static-binding.json'

        def agent(operation, value):
            self.assertEqual(linked.stat().st_size, 0, 'a hard link holds no token while the agent works')
            self.assertFalse(auth.CREDENTIAL.exists())
        with mock.patch.object(auth, 'ROOT_CREDENTIAL', root_file), \
                mock.patch.object(auth, 'root_holds_token', return_value=True), \
                mock.patch.object(auth, 'AGENT_UID', os.getuid()):
            auth.install_as_root(self.value, agent)
        self.assertTrue(root_file.exists())

    def test_a_link_in_place_of_the_earlier_file_is_removed_before_the_agent_works(self):
        auth.CREDENTIAL.parent.mkdir(parents=True, exist_ok=True)
        os.symlink(self.path / 'elsewhere.json', auth.CREDENTIAL)
        seen = []
        with mock.patch.object(auth, 'ROOT_CREDENTIAL', self.path / 'root/static-binding.json'), \
                mock.patch.object(auth, 'root_holds_token', return_value=True):
            auth.install_as_root(self.value, lambda operation, value: seen.append(auth.CREDENTIAL.is_symlink()))
        self.assertEqual(seen, [False])
        self.assertFalse((self.path / 'elsewhere.json').exists(), 'root never writes through the link')

    def test_the_gh_fallback_under_isolation_reads_the_routed_configuration(self):
        isolated = self.path / 'isolated'
        isolated.touch()
        with mock.patch.object(auth, 'AGENT_ISOLATION', isolated), \
                mock.patch.dict(auth.os.environ, {'GH_CONFIG_DIR': '/elsewhere'}, clear=True), \
                mock.patch.object(auth.sys, 'argv', ['gh', 'pr', 'list']), \
                mock.patch.object(auth.os, 'execve') as execute:
            auth.main()
        self.assertEqual(execute.call_args.args[2]['GH_CONFIG_DIR'], '/workspace/home/.config/gh')

    def test_a_failed_root_write_puts_the_identities_of_the_binding_in_place_back(self):
        root_file = self.path / 'root/static-binding.json'
        older = dict(self.value, author_name='Older Author')
        auth.write_private(older, root_file)
        steps = []
        write = auth.write_private

        def failing(value, path):
            if path == root_file:
                raise ValueError('no storage')
            return write(value, path)
        with mock.patch.object(auth, 'ROOT_CREDENTIAL', root_file), \
                mock.patch.object(auth, 'root_holds_token', return_value=True), \
                mock.patch.object(auth, 'write_private', failing), \
                self.assertRaises(ValueError):
            auth.install_as_root(dict(self.value, author_name='Newer Author'),
                                 lambda operation, value: steps.append(value['binding']['author_name']))
        self.assertEqual(steps, ['Newer Author', 'Older Author'], 'the agent configures Git for the older binding again')
        self.assertEqual(json.loads(root_file.read_text())['author_name'], 'Older Author')

    def test_an_agent_step_that_fails_part_way_is_rolled_back_to_the_binding_in_place(self):
        root_file = self.path / 'root/static-binding.json'
        auth.write_private(dict(self.value, author_name='Older Author'), root_file)
        steps = []

        def agent(operation, value):
            steps.append(value['binding']['author_name'])
            if len(steps) == 1:
                raise subprocess.CalledProcessError(1, 'install-identity')
        with mock.patch.object(auth, 'ROOT_CREDENTIAL', root_file), \
                mock.patch.object(auth, 'root_holds_token', return_value=True), \
                self.assertRaises(subprocess.CalledProcessError):
            auth.install_as_root(dict(self.value, author_name='Newer Author'), agent)
        self.assertEqual(steps, ['Newer Author', 'Older Author'])

    def test_an_earlier_token_file_that_stays_stops_the_install_before_the_agent_step(self):
        auth.write_private(self.value, auth.CREDENTIAL)
        steps = []
        unlink = Path.unlink

        def refused(path, *args, **options):
            if path == auth.CREDENTIAL:
                raise PermissionError(13, 'Permission denied')
            return unlink(path, *args, **options)
        with mock.patch.object(auth, 'ROOT_CREDENTIAL', self.path / 'root/static-binding.json'), \
                mock.patch.object(auth, 'root_holds_token', return_value=True), \
                mock.patch.object(Path, 'unlink', refused), self.assertRaises(PermissionError):
            auth.install_as_root(self.value, lambda operation, value: steps.append(operation))
        self.assertEqual(steps, [])

    def test_clear_empties_an_earlier_token_file_with_its_hard_links(self):
        auth.write_private(self.value, auth.CREDENTIAL)
        linked = auth.CREDENTIAL.with_name('copy.json')
        os.link(auth.CREDENTIAL, linked)
        with mock.patch.object(auth, 'ROOT_CREDENTIAL', self.path / 'root/static-binding.json'), \
                mock.patch.object(auth, 'root_holds_token', return_value=True), \
                mock.patch.object(auth, 'AGENT_UID', os.getuid()):
            auth.clear()
        self.assertFalse(auth.CREDENTIAL.exists())
        self.assertEqual(linked.stat().st_size, 0)

    def test_a_broken_root_file_gives_way_to_the_earlier_binding_when_an_install_fails(self):
        auth.write_private(self.value, auth.CREDENTIAL)
        root_file = self.path / 'root/static-binding.json'
        root_file.parent.mkdir(mode=0o700)
        root_file.write_text('not json')

        def agent(operation, value):
            raise subprocess.CalledProcessError(1, operation)
        with mock.patch.object(auth, 'ROOT_CREDENTIAL', root_file), \
                mock.patch.object(auth, 'root_holds_token', return_value=True), \
                mock.patch.object(auth, 'AGENT_UID', os.getuid()), \
                self.assertRaises(subprocess.CalledProcessError):
            auth.install_as_root(dict(self.value, token='synthetic-new-token'), agent)
        self.assertEqual(json.loads(root_file.read_text())['token'], self.value['token'])

    def test_a_failed_install_keeps_the_earlier_binding_where_only_root_reads_it(self):
        auth.write_private(self.value, auth.CREDENTIAL)
        root_file = self.path / 'root/static-binding.json'

        def agent(operation, value):
            raise subprocess.CalledProcessError(1, 'install-identity')
        with mock.patch.object(auth, 'ROOT_CREDENTIAL', root_file), \
                mock.patch.object(auth, 'root_holds_token', return_value=True), \
                mock.patch.object(auth, 'AGENT_UID', os.getuid()), \
                self.assertRaises(subprocess.CalledProcessError):
            auth.install_as_root(dict(self.value, token='synthetic-new-token'), agent)
        self.assertFalse(auth.CREDENTIAL.exists())
        self.assertEqual(json.loads(root_file.read_text())['token'], self.value['token'])
        # A root-only binding in place is never replaced by the earlier one.
        auth.write_private(self.value, auth.CREDENTIAL)
        with mock.patch.object(auth, 'ROOT_CREDENTIAL', root_file), \
                mock.patch.object(auth, 'root_holds_token', return_value=True), \
                mock.patch.object(auth, 'AGENT_UID', os.getuid()), \
                self.assertRaises(subprocess.CalledProcessError):
            auth.install_as_root(dict(self.value, token='synthetic-newer-token'), agent)

    def test_a_token_file_of_another_account_is_refused(self):
        with mock.patch.object(auth.subprocess, 'run'):
            auth.install(self.value)
        with mock.patch.object(auth.os, 'geteuid', return_value=os.geteuid() + 1), \
                self.assertRaises(ValueError):
            auth.read_grants()

    def test_a_marker_the_agent_account_cannot_see_still_means_isolation(self):
        # As the agent account, the root-only directory of the marker refuses a look.
        real = os.stat

        def refused(path, *args, **options):
            if str(path) == str(auth.AGENT_ISOLATION):
                raise PermissionError(13, 'Permission denied')
            return real(path, *args, **options)
        auth.write_private(self.value, auth.CREDENTIAL)
        with mock.patch.object(auth.os, 'stat', refused):
            self.assertTrue(auth.isolated())
            # The agent's restore of the static binding no longer fails on it, and reads no token file.
            with mock.patch.object(auth.subprocess, 'run'), \
                    mock.patch.object(auth, 'read_grants', side_effect=AssertionError('no token file')):
                auth.restore_static({'previous': []})
        with mock.patch.object(auth, 'AGENT_ISOLATION', self.path / 'none'):
            self.assertFalse(auth.isolated())
        with mock.patch.object(auth.os, 'stat', side_effect=OSError(5, 'Input/output error')), \
                self.assertRaises(OSError):
            auth.isolated()

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

    def test_a_service_token_does_not_outlive_the_service_in_a_nested_gh(self):
        # The service stopped answering and its chain replaced the static file: a nested gh must
        # drop the token an outer wrapped gh injected, but keep a token the caller set.
        for inherited, expected in [({'GH_TOKEN': 'ghu_from_service', auth.INJECTED: '1'}, None),
                                    ({'GH_TOKEN': 'own-token'}, 'own-token'),
                                    ({'GH_TOKEN': 'ghu_from_service', auth.INJECTED: '1', 'GITHUB_TOKEN': 'own-token'},
                                     None)]:
            with mock.patch.dict(auth.os.environ, dict(inherited, GH_REPO='example/project'), clear=True), \
                    mock.patch.object(auth.sys, 'argv', ['gh', 'pr', 'list']), \
                    mock.patch.object(auth.os, 'execve') as execute:
                auth.main()
            env = execute.call_args.args[2]
            self.assertEqual(env.get('GH_TOKEN'), expected, inherited)
            self.assertNotIn(auth.INJECTED, env)
            self.assertEqual(env.get('GITHUB_TOKEN'), inherited.get('GITHUB_TOKEN'), 'the caller keeps its own token')

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
                            ('SERVICE_SOCKET', self.path / 'no-service.sock'),
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
        for url in ('https://github.com/example/other.git', 'https://github.com/example/second.git'):
            git('config', '--add', 'remote.origin.url', url, cwd=library, env=self.env)
            git('config', '--add', 'remote.origin.pushurl', url, cwd=library, env=self.env)
        auth.install(self.value)
        self.assertEqual(auth.CREDENTIAL.stat().st_mode & 0o777, 0o600)
        self.assertEqual(auth.read_grants(), (2, self.value['grants']))
        for worktree, grant in [(primary, self.primary), (library, self.sibling)]:
            self.assertEqual(git('remote', 'get-url', 'origin', cwd=worktree, env=self.env),
                             'https://github.com/' + grant['repository'] + '.git')
            self.assertEqual(git('remote', 'get-url', '--push', '--all', 'origin', cwd=worktree, env=self.env),
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

    def test_the_route_sends_github_urls_through_the_proxy_and_keeps_remote_urls_for_gh(self):
        primary = self.bare(auth.GIT_DIR)
        self.bare(auth.SIBLINGS / 'library/repository.git')
        identities = [{key: grant[key] for key in (*auth.IDENTITY_FIELDS, 'target')} for grant in self.value['grants']]

        def routed():
            found = subprocess.run(['git', 'config', '--global', '--includes', '--get-regexp',
                                    r'^(http\..*\.(proxy|sslcainfo)|url\..*)$'],
                                   env=self.env, capture_output=True, text=True).stdout.splitlines()
            return sorted(found)
        expected = sorted(['http.https://github.com/.proxy http://127.0.0.1:47281',
                           'http.https://github.com/.sslcainfo /run/horizon-worker/github-ca.pem',
                           *('url.https://github.com/.insteadof ' + form for form in auth.REWRITTEN)])
        # The agent's own entry that equals one of the route's stays after the route ends.
        git('config', '--global', 'url.https://github.com/.insteadOf', 'git@github.com:', env=self.env)
        auth.route(self.env, True)
        auth.route(self.env, True)
        self.assertEqual(routed(), sorted(expected + ['url.https://github.com/.insteadof git@github.com:']),
                         'set once, however often the service starts')
        git('config', '--global', '--unset-all', 'url.https://github.com/.insteadOf', env=self.env)
        self.assertEqual(routed(), expected)
        # Installs and restores leave the route to the service that holds the proxy's port.
        auth.configure_chain({'grants': identities, 'previous': []})
        auth.install(self.value)
        auth.CREDENTIAL.unlink()
        auth.restore_static({'previous': identities})
        self.assertEqual(routed(), expected)
        # gh finds the repository from the remote URL, which stays a github.com URL.
        self.assertEqual(git('ls-remote', '--get-url', 'origin', cwd=primary, env=self.env),
                         'https://github.com/example/consumer.git')
        checkout = self.path / 'checkout'
        git('init', '-q', str(checkout), env=self.env)
        cwd = os.getcwd()
        self.addCleanup(os.chdir, cwd)
        os.chdir(checkout)
        for remote in ('https://github.com/example/consumer.git', 'git@github.com:example/consumer.git',
                       'http://github.com/example/consumer'):
            subprocess.run(['git', 'remote', 'remove', 'origin'], env=self.env, capture_output=True)
            git('remote', 'add', 'origin', remote, env=self.env)
            self.assertEqual(auth.gh_repository(self.env, ['pr', 'create']), 'example/consumer', remote)
        git('config', '--global', 'url.https://example.invalid/.insteadOf', 'https://other.invalid/', env=self.env)
        git('config', '--global', '--add', 'url.https://github.com/.insteadOf', 'git@github.com:', env=self.env)
        auth.route(self.env, False)
        self.assertEqual(routed(), ['url.https://example.invalid/.insteadof https://other.invalid/',
                                    'url.https://github.com/.insteadof git@github.com:'],
                         'only the route the helper wrote is removed')
        self.assertFalse((Path(self.env['HOME']) / auth.ROUTE_FILE).exists())
        self.assertEqual(subprocess.run(['git', 'config', '--global', '--get-all', 'include.path'], env=self.env,
                                        capture_output=True).returncode, 1)
        auth.route(self.env, False)

    @unittest.skipUnless(Path(auth.GH).exists(), 'needs gh')
    def test_the_route_points_gh_at_the_broker_and_removes_only_its_own_value(self):
        # The agents' gh reads its configuration under HOME, whatever the caller's variables say.
        env = dict(self.env, GH_CONFIG_DIR=str(self.path / 'elsewhere'), XDG_CONFIG_HOME=str(self.path / 'xdg'))
        auth.route_gh(env, True)
        auth.route_gh(env, True)
        config = self.path / 'home/.config/gh/config.yml'
        self.assertIn('http_unix_socket: ' + auth.API_SOCKET, config.read_text())
        self.assertFalse((self.path / 'elsewhere').exists())
        auth.route_gh(env, False)
        self.assertEqual(auth.gh_config(env, 'get', 'http_unix_socket'), '')
        auth.gh_config(env, 'set', 'http_unix_socket', str(self.path / 'own.sock'))
        auth.route_gh(env, False)
        self.assertEqual(auth.gh_config(env, 'get', 'http_unix_socket'), str(self.path / 'own.sock'),
                         'a socket the agent chose stays')

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
                     ['api', 'https://api.github.com/repos/example/consumer/issues'],
                     ['api', '/repos/{owner}/{repo}/pulls'], ['repo', 'clone', 'example/consumer', 'a/b/c'],
                     ['repo', 'view', '--json', 'name'], ['repo', 'view', '-b', 'feature/x', '--jq', '.name'],
                     ['repo', 'sync', '--branch', 'main', 'example/consumer'],
                     ['repo', 'clone', '-u', 'upstream', 'example/consumer', '--', '--depth', '1']):
            env, _ = self.gh({}, argv, cwd=primary)
            self.assertEqual(env['GH_TOKEN'], self.primary['token'], argv)
        for argv in (['repo', 'view', 'example/library'], ['repo', 'clone', 'example/library'],
                     ['api', 'repos/example/library/contents/x'],
                     ['api', 'https://api.github.com/repos/example/library/issues'],
                     ['api', 'HTTPS://API.GITHUB.COM/repos/example/library'],
                     ['pr', 'view', 'https://github.com/example/library/pull/1'],
                     ['pr', 'create', '--body', 'https://github.com/example/library/pull/3'],
                     ['repo', 'view', 'library'], ['repo', 'view', '--json', 'name', 'example/library'],
                     ['repo', 'fork', '--org', 'example', 'example/library']):
            env, message = self.gh({}, argv, cwd=primary)
            self.assertNotIn('GH_TOKEN', env, argv)
            self.assertIn('no Git grant', message)
        env, _ = self.gh({}, ['repo', 'clone', 'example/library'], cwd=self.path)
        self.assertEqual((env['GH_TOKEN'], env['GH_REPO']), (self.sibling['token'], 'example/library'))
        env, _ = self.gh({'GH_REPO': 'example/library'}, ['pr', 'view', 'https://github.com/example/consumer/pull/1'])
        self.assertNotIn('GH_TOKEN', env)

    def test_gh_selector_that_may_be_an_option_value_never_redirects(self):
        primary = self.bare(auth.GIT_DIR)
        library = self.bare(auth.SIBLINGS / 'library/repository.git')
        auth.install(self.value)
        for argv in (['repo', 'edit', '--description', '--repo=example/library'],
                     ['pr', 'list', '--web', '-R', 'example/library'],
                     ['pr', 'create', '--title', '-cRexample/library'],
                     ['issue', 'create', '--title', 'x', '--', '--repo', 'example/library']):
            env, message = self.gh({}, argv, cwd=primary)
            self.assertNotIn('GH_TOKEN', env, argv)
            self.assertNotIn('GH_REPO', env, argv)
            self.assertIn('no Git grant', message)
        env, _ = self.gh({}, ['pr', 'list', '--web', '-R', 'example/library'], cwd=library)
        self.assertEqual(env['GH_TOKEN'], self.sibling['token'])
        env, _ = self.gh({}, ['pr', 'list', '-R', 'example/library', '--web'], cwd=primary)
        self.assertEqual((env['GH_TOKEN'], env['GH_REPO']), (self.sibling['token'], 'example/library'))

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

    def test_largest_escape_heavy_payload_installs_and_fits_the_private_file(self):
        # Every token and identity character needs a JSON escape: the largest valid encoding.
        self.bare(auth.GIT_DIR)
        grants = [{'repository': 'o' * 100 + '/' + 'r' * 98 + '%02d' % index, 'token': '"\\' * 1024,
                   'author_name': '\U0001F600' * 50, 'author_email': '\\' * 200,
                   'target': 'sibling:' + 's' * 62 + '%02d' % index} for index in range(16)]
        grants[0]['target'] = 'primary'
        for grant in grants[1:]:
            self.bare(auth.SIBLINGS / grant['target'].removeprefix('sibling:') / 'repository.git')
        value = {'version': 2, 'grants': grants}
        source = json.dumps(value)
        self.assertGreater(len(source.encode()), 65536)
        with mock.patch.object(auth.sys, 'argv', ['horizon-worker-git-auth', 'install']), \
                mock.patch.object(auth.sys, 'stdin', io.StringIO(source)):
            auth.main()
        self.assertLessEqual(auth.CREDENTIAL.stat().st_size, auth.MAX_BYTES)
        self.assertEqual(auth.read_grants(), (2, grants))

    def test_symlinked_or_shared_credential_files_are_refused(self):
        auth.write_private(self.value, auth.CREDENTIAL)
        link = self.path / 'link.json'
        link.symlink_to(auth.CREDENTIAL)
        with mock.patch.object(auth, 'CREDENTIAL', link), self.assertRaises(OSError):
            auth.read_grants()
        auth.CREDENTIAL.chmod(0o640)
        with self.assertRaises(ValueError):
            auth.read_grants()



@unittest.skipIf(os.geteuid() == 0, 'root sees into every directory')
class AgentBehindTheRootOnlyMarkerTests(unittest.TestCase):
    """The agent side of the helper with the isolation marker in a directory that this account
    cannot look into, as /run/horizon-tailnet is on a worker (root, 0700)."""

    def setUp(self):
        self.root = tempfile.TemporaryDirectory()
        self.addCleanup(self.root.cleanup)
        self.path = Path(self.root.name)
        hidden = self.path / 'horizon-tailnet'
        hidden.mkdir()
        (hidden / 'agent-isolation').touch()
        hidden.chmod(0)
        self.addCleanup(hidden.chmod, 0o700)
        home = self.path / 'home'
        home.mkdir()
        subprocess.run(['git', 'init', '-q', '--bare', str(self.path / 'repository.git')], check=True)
        for name, value in [('AGENT_ISOLATION', hidden / 'agent-isolation'),
                            ('CREDENTIAL', self.path / 'credentials/github.json'),
                            ('SERVICE_SOCKET', self.path / 'no-service.sock'),
                            ('GIT_DIR', self.path / 'repository.git'), ('HOME', str(home))]:
            patcher = mock.patch.object(auth, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)
        environment = mock.patch.dict(auth.os.environ, {'HOME': str(home), 'GIT_CONFIG_NOSYSTEM': '1',
                                                        'GIT_CONFIG_GLOBAL': str(home / '.gitconfig')})
        environment.start()
        self.addCleanup(environment.stop)
        self.identity = {'repository': 'example/project', 'author_name': 'Agent',
                         'author_email': 'agent@example.invalid', 'target': 'primary'}

    def test_every_agent_operation_runs(self):
        self.assertFalse(auth.root_holds_token())
        # What the GitHub service runs as the agent at its start and for each install.
        auth.restore_static({'previous': []})
        auth.configure_chain({'grants': [self.identity], 'previous': []})
        auth.install_identities({'version': 2, 'grants': [self.identity]})
        self.assertIsNotNone(auth.previous_grants())
        # Git's credential helper and the gh wrapper without a service.
        with mock.patch.object(auth.sys, 'argv', ['horizon-worker-git-auth', 'get']), \
                mock.patch.object(auth.sys, 'stdin', io.StringIO('protocol=https\nhost=github.com\npath=a/b\n')), \
                mock.patch.object(auth.sys, 'stdout', io.StringIO()):
            auth.main()
        with mock.patch.object(auth.sys, 'argv', ['gh', 'pr', 'list']), \
                mock.patch.object(auth.os, 'execve') as execute:
            auth.main()
        self.assertEqual(execute.call_args.args[2]['GH_CONFIG_DIR'], str(Path(auth.HOME) / '.config/gh'))
        self.assertTrue(auth.isolated())

if __name__ == '__main__':
    unittest.main()
