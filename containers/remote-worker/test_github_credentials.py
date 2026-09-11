#!/usr/bin/env python3
"""Synthetic-only worker credentials; no network, local login or real tokens."""

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
TOKEN = 'github_pat_SYNTHETIC_NOT_A_CREDENTIAL'


class CredentialsTests(unittest.TestCase):
    def setUp(self):
        self.fixture = tempfile.TemporaryDirectory(prefix='horizon-github-credentials-')
        self.root = Path(self.fixture.name)
        self.runtime = self.root / 'runtime'
        self.runtime.mkdir(mode=0o700)
        self.token = self.runtime / 'github-token'
        self.token.write_text(TOKEN + '\n')
        self.token.chmod(0o600)
        self.runtime_patch = patch.object(C, 'RUNTIME', self.runtime)
        self.runtime_patch.start()

    def tearDown(self):
        self.runtime_patch.stop()
        self.fixture.cleanup()

    def get(self, request=b'protocol=https\nhost=github.com\n\n', operation='get'):
        output = io.StringIO()
        C.git_credential(operation, io.BytesIO(request), output)
        return output.getvalue()

    def test_valid_token_and_single_optional_file_newline(self):
        for value in (TOKEN, TOKEN + '\n'):
            self.token.write_text(value)
            self.assertEqual(TOKEN, C.read_token())
            self.assertEqual('username=x-access-token\npassword=' + TOKEN + '\n\n', self.get())

    def test_bad_token_shapes_are_rejected_without_output(self):
        for value in ('', TOKEN + '\n\n', TOKEN + '\r\n', 'private secret', 'x\0y', 'x' * 16385):
            self.token.write_text(value)
            with self.assertRaises(C.CredentialError):
                self.get()

    def test_missing_token_is_noninteractive_and_never_recreated(self):
        self.token.unlink()
        self.assertIsNone(C.read_token())
        self.assertEqual('quit=true\n\n', self.get())
        self.assertFalse(self.token.exists())

    def test_protected_directory_and_file_modes(self):
        for mode in (0o644, 0o666, 0o777):
            self.token.chmod(mode)
            with self.assertRaises(C.CredentialError):
                C.read_token()
        self.token.chmod(0o600)
        self.runtime.chmod(0o755)
        with self.assertRaises(C.CredentialError):
            C.read_token()
        self.runtime.chmod(0o700)
        with patch.object(C.os, 'geteuid', return_value=os.geteuid() + 1):
            with self.assertRaises(C.CredentialError):
                C.read_token()

    def test_link_fifo_and_directory_tokens_rejected(self):
        self.token.unlink()
        target = self.root / 'target'
        target.write_text(TOKEN)
        target.chmod(0o600)
        self.token.symlink_to(target)
        with self.assertRaises(OSError):
            C.read_token()
        self.token.unlink()
        os.link(target, self.token)
        with self.assertRaises(C.CredentialError):
            C.read_token()
        self.token.unlink()
        os.mkfifo(self.token, 0o600)
        with self.assertRaises(C.CredentialError):
            C.read_token()
        self.token.unlink()
        self.token.mkdir(mode=0o700)
        with self.assertRaises(C.CredentialError):
            C.read_token()

    def test_runtime_directory_symlink_rejected(self):
        link = self.root / 'runtime-link'
        link.symlink_to(self.runtime, target_is_directory=True)
        with patch.object(C, 'RUNTIME', link):
            with self.assertRaises(OSError):
                C.read_token()

    def test_replacement_during_read_rejected(self):
        read = os.read
        def replace(descriptor, size):
            data = read(descriptor, size)
            self.token.unlink()
            self.token.write_text(TOKEN)
            self.token.chmod(0o600)
            return data
        with patch.object(C.os, 'read', side_effect=replace):
            with self.assertRaises(C.CredentialError):
                C.read_token()

    def test_other_hosts_protocols_and_lookalikes_never_read_token(self):
        with patch.object(C, 'read_token', side_effect=AssertionError('must not read token')):
            for protocol, host in [('http', 'github.com'), ('https', 'github.com.evil'),
                                   ('https', 'gitlab.com'), ('https', 'github.com:443'),
                                   ('ssh', 'github.com'), ('https', 'user@github.com')]:
                self.assertEqual('', self.get(f'protocol={protocol}\nhost={host}\n\n'.encode()))

    def test_invalid_protocol_input_is_bounded_and_not_echoed(self):
        for value in (b'host=github.com\nhost=other\n\n', b'bad-line\n', b'host=x\0y\n',
                      b'host=github.com\r\n', b'x=' + b'x' * 65536 + b'\n'):
            with self.assertRaises(C.CredentialError):
                self.get(value)
        self.assertEqual('', self.get(b'\n'))

    def test_store_erase_and_unknown_operations_are_noops(self):
        before = self.token.read_bytes()
        with patch.object(C, 'read_token', side_effect=AssertionError('must not read token')):
            for operation in ('store', 'erase', 'future-operation'):
                self.assertEqual('', self.get(b'password=other-secret\n\n', operation))
        self.assertEqual(before, self.token.read_bytes())

    def test_gh_exec_has_only_file_token_in_child_environment(self):
        argv = ['api', 'user', '--jq', '.login']
        parent = {'PATH': '/usr/bin:/bin', 'GH_TOKEN': 'ambient-secret', 'GITHUB_TOKEN': 'other',
                  'GH_ENTERPRISE_TOKEN': 'enterprise', 'GH_DEBUG': 'api', 'GH_CONFIG_DIR': '/unrelated'}
        with patch.dict(C.os.environ, parent, clear=True), patch.object(C.os, 'execve') as execute:
            C.run_gh(argv)
            self.assertEqual('ambient-secret', os.environ['GH_TOKEN'])
        binary, arguments, environment = execute.call_args.args
        self.assertEqual('/usr/bin/gh', binary)
        self.assertEqual(['/usr/bin/gh', *argv], arguments)
        self.assertEqual(TOKEN, environment['GH_TOKEN'])
        self.assertEqual('github.com', environment['GH_HOST'])
        self.assertEqual('/run/horizon/github-cli', environment['GH_CONFIG_DIR'])
        self.assertNotIn('GITHUB_TOKEN', environment)
        self.assertNotIn('GH_ENTERPRISE_TOKEN', environment)
        self.assertNotIn('GH_DEBUG', environment)
        self.assertNotIn(TOKEN, ' '.join(arguments))

    def test_foreign_gh_targets_refused_before_secret_read(self):
        with patch.dict(C.os.environ, {}, clear=True), patch.object(C, 'read_token', side_effect=AssertionError('must not read token')):
            for args in (['api', '--hostname', 'evil.ghe.com'], ['api', '--hostname=evil.com'],
                         ['api', 'https://evil.com/user'], ['api', 'http://github.com/user']):
                with self.assertRaises(C.CredentialError):
                    C.run_gh(args)
            with patch.dict(C.os.environ, GH_HOST='evil.ghe.com'):
                with self.assertRaises(C.CredentialError):
                    C.run_gh(['api', 'user'])

    def test_gh_content_is_not_interpreted_as_host_flags(self):
        for args in (['pr', 'create', '--body', 'See https://example.com'],
                     ['issue', 'create', '--title', '--hostname=example.com'],
                     ['api', '-f', 'url=https://example.com', 'repos/a/b/issues'],
                     ['api', 'repos/a/b/issues', '--field', 'body=See https://example.com']):
            with patch.dict(C.os.environ, {}, clear=True), patch.object(C.os, 'execve') as execute:
                C.run_gh(args)
                self.assertEqual([C.GH, *args], execute.call_args.args[1])

    def test_existing_gh_config_is_not_an_alternative_credential_source(self):
        with patch.dict(C.os.environ, {}, clear=True), patch.object(C.os.path, 'lexists', return_value=True):
            with self.assertRaises(C.CredentialError):
                C.run_gh(['api', 'user'])

    def test_missing_token_keeps_help_but_fails_authenticated_operation(self):
        self.token.unlink()
        with patch.dict(C.os.environ, {}, clear=True), patch.object(C.os, 'execve') as execute:
            # execve does not return in production; emulate that control flow.
            execute.side_effect = SystemExit(0)
            for args in (['--version'], ['help'], ['repo', 'view', '--help']):
                with self.assertRaises(SystemExit):
                    C.run_gh(args)
                self.assertNotIn('GH_TOKEN', execute.call_args.args[2])
            with self.assertRaises(C.CredentialError):
                C.run_gh(['api', 'user'])

    def test_session_does_not_export_any_token_and_preserves_literal_argv(self):
        value = 'literal ; $(false) argument'
        code = 'import os,sys,json; print(json.dumps([sys.argv[1],os.environ.get("HORIZON"),[x for x in os.environ if x in '+repr(C.TOKEN_VARIABLES)+']]))'
        environment = {'PATH': '/usr/bin:/bin', **{key: TOKEN for key in C.TOKEN_VARIABLES}}
        result = subprocess.run(['/bin/sh', str(HERE / 'session.sh'), '/usr/bin/python3', '-c', code, value],
                                env=environment, capture_output=True, timeout=5, check=True)
        self.assertEqual([value, '1', []], json.loads(result.stdout))
        self.assertNotIn(TOKEN.encode(), result.stderr)

    def test_gh_authentication_failure_only_exits_its_child(self):
        fake = self.root / 'packaged-gh'
        fake.write_text('#!/usr/bin/python3\nimport os,sys\n'
                        + 'assert os.environ["GH_TOKEN"] == ' + repr(TOKEN) + '\n'
                        + 'assert "GITHUB_TOKEN" not in os.environ\n'
                        + 'print("synthetic authentication rejected", file=sys.stderr)\nsys.exit(42)\n')
        fake.chmod(0o700)
        wrapper = self.root / 'gh'
        source = (HERE / 'github-credentials.py').read_text()
        wrapper.write_text(source.replace("RUNTIME = Path('/run/horizon')", 'RUNTIME = Path(' + repr(str(self.runtime)) + ')')
                           .replace("GH = '/usr/bin/gh'", 'GH = ' + repr(str(fake))))
        environment = {'PATH': '/usr/bin:/bin', 'GITHUB_TOKEN': 'ignored-ambient-secret'}
        before = set(self.root.iterdir())
        result = subprocess.run(['/usr/bin/python3', '-I', str(wrapper), 'api', 'user'],
                                env=environment, capture_output=True, timeout=5, check=False)
        self.assertEqual(42, result.returncode)
        self.assertEqual(b'', result.stdout)
        self.assertEqual(b'synthetic authentication rejected\n', result.stderr)
        self.assertNotIn(TOKEN.encode(), result.stdout + result.stderr)
        self.assertEqual(before, set(self.root.iterdir()))
        self.assertEqual(TOKEN + '\n', self.token.read_text())

    def test_image_and_startup_contract_uses_absolute_readonly_helper(self):
        dockerfile = (HERE / 'Dockerfile').read_text()
        entrypoint = (HERE / 'entrypoint.sh').read_text()
        self.assertIn('COPY containers/remote-worker/github-credentials.py /usr/local/bin/horizon-github-credential', dockerfile)
        self.assertIn('ln -s horizon-github-credential /usr/local/bin/gh', dockerfile)
        self.assertIn('credential.https://github.com.helper /usr/local/bin/horizon-github-credential', entrypoint)
        self.assertNotIn('gh auth setup-git', entrypoint)
        self.assertNotIn('$(cat /run/horizon/github-token)', entrypoint)
        self.assertIn('!containers/remote-worker/github-credentials.py', (HERE.parents[1] / '.dockerignore').read_text())
        smoke = (HERE.parents[1] / 'scripts/run-remote-worker-smoke.sh').read_text()
        self.assertIn('slow-identity,dst=/usr/local/bin/horizon-worker-host-identity,readonly', smoke)
        self.assertNotIn('slow-gh', smoke)

    def test_real_git_fill_concurrent_no_config_or_secret_storage(self):
        helper = self.root / 'helper.py'
        source = (HERE / 'github-credentials.py').read_text()
        helper.write_text(source.replace("RUNTIME = Path('/run/horizon')", 'RUNTIME = Path(' + repr(str(self.runtime)) + ')'))
        environment = {'PATH': '/usr/bin:/bin', 'LC_ALL': 'C', 'HOME': str(self.root),
                       'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1', 'GIT_TERMINAL_PROMPT': '0'}
        command = ['/usr/bin/git', '-c', 'credential.helper=', '-c',
                   'credential.https://github.com.helper=/usr/bin/python3 -I ' + str(helper), 'credential', 'fill']
        def fill(_):
            result = subprocess.run(command, input=b'protocol=https\nhost=github.com\n\n',
                                    env=environment, capture_output=True, timeout=5, check=True)
            self.assertIn(('password=' + TOKEN).encode(), result.stdout)
            self.assertEqual(b'', result.stderr)
        before = set(self.root.iterdir())
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
            list(pool.map(fill, range(8)))
        for operation in ('approve', 'reject'):
            result = subprocess.run([*command[:-1], operation],
                input=b'protocol=https\nhost=github.com\nusername=x-access-token\npassword=synthetic_other\n\n',
                env=environment, capture_output=True, timeout=5, check=True)
            self.assertEqual(b'', result.stdout + result.stderr)
        self.assertEqual(TOKEN + '\n', self.token.read_text())
        self.assertEqual(before, set(self.root.iterdir()))
        self.assertFalse((self.root / '.gitconfig').exists())
        self.assertFalse((self.root / '.git-credentials').exists())


if __name__ == '__main__':
    unittest.main()
