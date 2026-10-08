"""The worker GitHub token chain service, with synthetic tokens and a fake GitHub on the
loopback address. Never use account credentials here."""
import contextlib
import functools
import http.server
import importlib.machinery
import importlib.util
import io
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import unittest
from unittest import mock

HERE = Path(__file__).parent


def load(name, module):
    loader = importlib.machinery.SourceFileLoader(module, str(HERE / name))
    spec = importlib.util.spec_from_loader(loader.name, loader)
    loaded = importlib.util.module_from_spec(spec)
    loader.exec_module(loaded)
    return loaded


service = load('horizon-worker-github', 'worker_github')
auth = service.auth
common = service.common
NOW = 1_800_000_000
ACCESS = 'ghu_synthetic-access-one'
REFRESH = 'ghr_synthetic-refresh-one'


def chain(access=ACCESS, refresh=REFRESH, access_in=8 * 3600, refresh_in=182 * 86400):
    return {'access_token': access, 'access_expires_at': NOW + access_in,
            'refresh_token': refresh, 'refresh_expires_at': NOW + refresh_in}


def installation(**overrides):
    value = {'version': 1, 'client_id': 'Iv23synthetic', 'author_name': 'Test Author',
             'author_email': 'author@example.invalid',
             'grants': [{'repository': 'example/project', 'target': 'primary', 'access': 'push'},
                        {'repository': 'example/library', 'target': 'sibling:library', 'access': 'read'}],
             'chain': chain()}
    value.update(overrides)
    return value


class FakeGitHub(http.server.ThreadingHTTPServer):
    """Answers each refresh with the next queued (status, body) and records the forms."""

    def __init__(self):
        super().__init__(('127.0.0.1', 0), Handler)
        self.forms, self.replies = [], []
        threading.Thread(target=self.serve_forever, daemon=True).start()

    @property
    def url(self):
        return 'http://127.0.0.1:%d' % self.server_address[1]


class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers['Content-Length'])
        form = dict(item.split('=', 1) for item in self.rfile.read(length).decode().split('&'))
        self.server.forms.append((self.path, self.headers['Accept'], form))
        status, body = self.server.replies.pop(0)
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(body.encode())

    def log_message(self, *args):
        pass


def rotated(number):
    return json.dumps({'access_token': 'ghu_synthetic-access-%d' % number, 'expires_in': 28800,
                       'refresh_token': 'ghr_synthetic-refresh-%d' % number,
                       'refresh_token_expires_in': 15724800, 'token_type': 'bearer', 'scope': ''})


class ServiceTestCase(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        (self.root / 'workspace').mkdir()
        (self.root / 'run').mkdir()
        self.store = service.Store(self.root / 'workspace/.horizon-root/github', self.root / 'run/github')
        self.configured = []
        self.retired = []

    def install(self, value=None, now=NOW):
        return service.install(value or installation(), self.store, now=lambda: now,
                               configure=self.configured.append, retire=lambda: self.retired.append(True))

    def fake_github(self, *replies):
        github = FakeGitHub()
        self.addCleanup(github.server_close)
        self.addCleanup(github.shutdown)
        github.replies.extend(replies)
        return github, functools.partial(service.post_refresh, environ={service.TEST_URL: github.url})

    def stored(self):
        return self.store.load()[0]


class InstallationTests(ServiceTestCase):
    def test_malformed_installations_are_refused_without_echoing_secrets(self):
        grant = {'repository': 'example/project', 'target': 'primary', 'access': 'push'}
        cases = [
            installation(extra=True), {key: value for key, value in installation().items() if key != 'chain'},
            installation(version=2), installation(version=True), installation(client_id='bad id'),
            installation(client_secret='secret\nline'), installation(chain=dict(chain(), extra='x')),
            installation(chain=chain(access='ghu_new\nline')), installation(chain=dict(chain(), access_expires_at=True)),
            installation(chain=chain(refresh_in=0)), installation(grants=[]),
            installation(grants=[dict(grant, extra='x')]), installation(grants=[dict(grant, access='admin')]),
            installation(grants=[dict(grant, repository='../escape')]),
            installation(grants=[dict(grant, target='sibling:../escape')]),
            installation(grants=[grant, dict(grant, target='sibling:other', repository='Example/Project')]),
            installation(grants=[grant, dict(grant, repository='example/other')]),
            installation(grants=[dict(grant, repository='example/r%d' % index, target='sibling:s%d' % index)
                                 for index in range(17)]),
            installation(author_name='name\ninjection'), installation(author_email=''), [installation()],
            installation(login='bad login'), installation(login='-leading'), installation(login=7)]
        for value in cases:
            with self.assertRaises(ValueError) as refused:
                service.validate_install(value, NOW)
            self.assertNotIn('synthetic', str(refused.exception))
            with self.assertRaises(ValueError):
                self.install(value)
        self.assertEqual(self.configured, [])
        self.assertIsNone(self.stored())

    def test_duplicate_fields_and_oversized_input_are_refused(self):
        with self.assertRaises(ValueError):
            auth.parse('{"version":1,"version":1}')
        self.assertEqual(self.run_main('install', ' ' * (service.MAX_INSTALL + 1)), 1)
        self.assertIsNone(self.stored())

    def run_main(self, operation, stdin=''):
        source = io.TextIOWrapper(io.BytesIO(stdin.encode()))
        with mock.patch.object(service.os, 'geteuid', return_value=0), \
                mock.patch.object(service, 'Store', return_value=self.store), \
                mock.patch.object(service.sys, 'stdin', source), \
                contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            try:
                return service.main([operation])
            except ValueError:
                return 1

    def test_install_stores_a_private_persistent_chain_and_configures_without_tokens(self):
        report = self.install(installation(client_secret='synthetic-client-secret'))
        self.assertEqual(report['state'], 'ok')
        self.assertTrue(report['persistent'])
        state_file = self.store.persistent / service.STATE
        self.assertEqual(state_file.stat().st_mode & 0o777, 0o600)
        self.assertEqual(state_file.parent.stat().st_mode & 0o777, 0o700)
        self.assertEqual(state_file.parent.parent.stat().st_mode & 0o777, 0o700)
        self.assertEqual(self.stored()['chain'], chain())
        payload = json.dumps(self.configured)
        for secret in (ACCESS, REFRESH, 'synthetic-client-secret'):
            self.assertNotIn(secret, payload)
            self.assertNotIn(secret, json.dumps(report))
        self.assertEqual(self.configured[0]['previous'], [])
        self.assertEqual(self.configured[0]['grants'][1], {'repository': 'example/library', 'target': 'sibling:library',
                                                           'author_name': 'Test Author',
                                                           'author_email': 'author@example.invalid'})
        # A later install forgets the identities of the chain it replaces.
        self.install(installation(author_name='Other Author'))
        self.assertEqual(self.configured[1]['previous'], self.configured[0]['grants'])

    def test_the_login_is_optional_and_reported_in_status(self):
        self.assertIsNone(self.install()['login'])
        self.assertEqual(self.install(installation(login='octo-cat'))['login'], 'octo-cat')
        self.assertEqual(service.status(self.store)['login'], 'octo-cat')
        self.assertIsNone(service.status(service.Store(self.root / 'none', self.root / 'none-run'))['login'])

    def test_a_failed_configuration_puts_the_previous_chain_back_and_keeps_the_static_binding(self):
        self.install()
        before = self.stored()

        def refuse(payload):
            raise subprocess.CalledProcessError(1, 'configure')
        with self.assertRaises(subprocess.CalledProcessError):
            service.install(installation(author_name='Other Author', chain=chain(access='ghu_other')), self.store,
                            now=lambda: NOW, configure=refuse, retire=lambda: self.retired.append(True))
        # The rollback writes the previous chain again, under a new storage serial.
        without_serial = lambda state: {key: value for key, value in state.items() if key != 'serial'}
        self.assertEqual(without_serial(self.stored()), without_serial(before))
        self.assertEqual(self.retired, [True], 'only the first, successful install retired the static binding')

    def test_a_failed_first_configuration_leaves_no_chain(self):
        def refuse(payload):
            raise subprocess.CalledProcessError(1, 'configure')
        with self.assertRaises(subprocess.CalledProcessError):
            service.install(installation(), self.store, now=lambda: NOW, configure=refuse,
                            retire=lambda: self.retired.append(True))
        self.assertIsNone(self.stored())
        self.assertEqual(self.retired, [])

    def test_a_failed_save_configures_nothing_and_retires_nothing(self):
        with mock.patch.object(self.store, 'save', side_effect=OSError('read-only')), \
                self.assertRaises(OSError):
            self.install()
        self.assertEqual((self.configured, self.retired), ([], []))

    def test_a_static_binding_that_stays_does_not_fail_the_install(self):
        def stuck():
            raise OSError('busy')
        with contextlib.redirect_stderr(io.StringIO()):
            report = service.install(installation(), self.store, now=lambda: NOW,
                                     configure=self.configured.append, retire=stuck)
        self.assertEqual(report['state'], 'ok')

    def test_a_chain_survives_a_restart_and_clear_removes_it(self):
        self.install()
        restarted = service.Store(self.store.persistent, self.root / 'run/fresh-tmpfs')
        self.assertEqual(restarted.load()[0]['chain'], chain())
        service.clear(restarted)
        self.assertIsNone(self.stored())
        self.assertEqual(service.status(self.store)['state'], 'absent')

    def test_volume_without_private_permissions_falls_back_to_tmpfs(self):
        persistent = self.store.persistent
        persistent.mkdir(parents=True)
        real_chmod = os.chmod
        for directory in (persistent, persistent.parent):
            real_chmod(directory, 0o777)

        def chmod(path, mode):
            if Path(path) not in (persistent, persistent.parent):
                real_chmod(path, mode)
        with mock.patch.object(service.os, 'chmod', side_effect=chmod):
            report = self.install()
        self.assertFalse(report['persistent'])
        self.assertFalse((persistent / service.STATE).exists())
        self.assertEqual((self.store.runtime / service.STATE).stat().st_mode & 0o777, 0o600)
        self.assertEqual(service.status(self.store)['persistent'], False)
        # A file whose effective mode is not private is refused before any token is written.
        shared = os.stat_result((0o100666, 0, 0, 1, os.geteuid(), 0, 0, 0, 0, 0))
        with mock.patch.object(service.os, 'fstat', return_value=shared), self.assertRaises(ValueError):
            common.write_private(self.store.runtime, 'probe', ACCESS)
        self.assertEqual(sorted(path.name for path in self.store.runtime.iterdir()), ['state.json', 'state.lock'])

    def test_git_helper_configures_repositories_and_replaces_the_static_binding(self):
        def git(*args):
            return subprocess.run(['git', *args], check=True, capture_output=True, text=True, env=env).stdout.strip()
        env = dict(os.environ, HOME=str(self.root / 'home'), GIT_CONFIG_NOSYSTEM='1', GIT_TERMINAL_PROMPT='0')
        (self.root / 'home').mkdir()
        primary, library = self.root / 'repository.git', self.root / 'siblings/library/repository.git'
        for directory in (primary, library):
            git('init', '-q', '--bare', str(directory))
            git('--git-dir=' + str(directory), 'config', 'remote.origin.pushurl', 'https://github.com/x/y.git')
        static = self.root / 'credentials/github.json'
        static.parent.mkdir()
        static.write_text('{}')
        with contextlib.ExitStack() as stack:
            for name, value in [('CREDENTIAL', static), ('GIT_DIR', primary), ('SIBLINGS', self.root / 'siblings'),
                                ('HOME', env['HOME'])]:
                stack.enter_context(mock.patch.object(auth, name, value))
            stack.enter_context(mock.patch.dict(auth.os.environ, {'HOME': env['HOME'], 'GIT_CONFIG_NOSYSTEM': '1'}))
            service.install(installation(), self.store, now=lambda: NOW, configure=auth.configure_chain,
                            retire=lambda: auth.CREDENTIAL.unlink(missing_ok=True))
            with self.assertRaises(ValueError):
                auth.configure_chain({'grants': [dict(self.configured_grant(), token=ACCESS)], 'previous': []})
        self.assertFalse(static.exists())
        for directory, repository in ((primary, 'example/project'), (library, 'example/library')):
            self.assertEqual(git('--git-dir=' + str(directory), 'config', 'remote.origin.url'),
                             'https://github.com/' + repository + '.git')
            self.assertEqual(git('--git-dir=' + str(directory), 'config', 'user.name'), 'Test Author')
            self.assertNotIn('pushurl', git('--git-dir=' + str(directory), 'config', '--list'))
        self.assertEqual(git('config', '--global', 'credential.https://github.com.helper'),
                         '/usr/local/bin/horizon-worker-git-auth')

    def configured_grant(self):
        return {'repository': 'example/project', 'target': 'primary', 'author_name': 'Test Author',
                'author_email': 'author@example.invalid'}


class RefreshTests(ServiceTestCase):
    def test_refresh_waits_until_thirty_minutes_before_expiry(self):
        self.install()
        post = mock.Mock(side_effect=AssertionError('no refresh is due'))
        expiry = chain()['access_expires_at']
        self.assertEqual(service.refresh_once(self.store, lambda: expiry - 1801, post), 'fresh')
        self.assertEqual(service.next_check(self.store, NOW), service.POLL_SECONDS)
        self.assertEqual(service.next_check(self.store, expiry - 1810), 10)
        self.assertEqual(service.next_check(self.store, expiry), 1)

    def test_rotation_is_stored_before_any_agent_receives_it(self):
        self.install()
        github, post = self.fake_github((200, rotated(2)), (200, rotated(3)))
        expiry = chain()['access_expires_at']
        self.assertEqual(service.refresh_once(self.store, lambda: expiry - 1800, post), 'refreshed')
        path, accept, form = github.forms[0]
        self.assertEqual((path, accept), ('/login/oauth/access_token', 'application/json'))
        self.assertEqual(form, {'client_id': 'Iv23synthetic', 'grant_type': 'refresh_token', 'refresh_token': REFRESH})
        on_disk = json.loads((self.store.persistent / service.STATE).read_text())['chain']
        self.assertEqual(on_disk, {'access_token': 'ghu_synthetic-access-2', 'access_expires_at': expiry - 1800 + 28800,
                                   'refresh_token': 'ghr_synthetic-refresh-2',
                                   'refresh_expires_at': expiry - 1800 + 15724800})
        reply, _ = service.answer({'request': 'gh-token', 'repository': 'example/project'}, self.stored(), expiry - 1800)
        self.assertEqual(reply['token'], 'ghu_synthetic-access-2')
        # Without any storage the rotated chain stays in memory and is still served.
        later = on_disk['access_expires_at'] - 60 * 10
        with mock.patch.object(common, 'write_private', side_effect=OSError):
            with self.assertRaises(ValueError):
                service.refresh_once(self.store, lambda: later, post)
        self.assertEqual(self.stored()['chain']['refresh_token'], 'ghr_synthetic-refresh-3')
        self.assertEqual(github.forms[1][2]['refresh_token'], 'ghr_synthetic-refresh-2')
        self.assertEqual(service.refresh_once(self.store, lambda: later, post), 'fresh')

    def test_a_web_sign_in_chain_sends_its_client_secret(self):
        self.install(installation(client_secret='synthetic-client-secret'))
        github, post = self.fake_github((200, rotated(2)))
        service.refresh_once(self.store, lambda: NOW + 8 * 3600, post)
        self.assertEqual(github.forms[0][2]['client_secret'], 'synthetic-client-secret')

    def test_bad_refresh_token_revokes_the_chain_and_stops_refreshing(self):
        self.install()
        _, post = self.fake_github((200, '{"error":"bad_refresh_token","error_description":"x"}'))
        self.assertEqual(service.refresh_once(self.store, lambda: NOW + 8 * 3600, post), 'revoked')
        report = service.status(self.store)
        self.assertEqual((report['state'], report['last_error']), ('revoked', 'bad_refresh_token'))
        self.assertEqual(service.refresh_once(self.store, lambda: NOW + 8 * 3600, mock.Mock(side_effect=AssertionError)),
                         'revoked')
        reply, _ = service.answer({'request': 'gh-token', 'repository': 'example/project'}, self.stored(), NOW)
        self.assertEqual((reply['ok'], reply['state']), (False, 'revoked'))

    def test_an_expired_refresh_token_revokes_without_a_request(self):
        self.install()
        expired = chain()['refresh_expires_at']
        self.assertEqual(service.refresh_once(self.store, lambda: expired, mock.Mock(side_effect=AssertionError)),
                         'revoked')
        self.assertEqual(service.status(self.store)['last_error'], 'refresh_expired')

    def test_transport_failures_and_other_errors_are_retried_with_jittered_backoff(self):
        self.install()
        _, post = self.fake_github((500, 'unavailable'), (200, 'not json'),
                                   (200, '{"error":"incorrect_client_credentials"}'), (200, '{"access_token":"x"}'))
        due = lambda: NOW + 8 * 3600 - 60  # noqa: E731
        for expected in ('transport', 'malformed_reply', 'incorrect_client_credentials', 'malformed_reply'):
            self.assertEqual(service.refresh_once(self.store, due, post), 'retry')
            self.assertEqual(service.status(self.store)['last_error'], expected)
        self.assertEqual(self.stored()['chain'], chain(), 'the chain is kept for the next attempt')
        closed = socket.socket()
        closed.bind(('127.0.0.1', 0))
        port = closed.getsockname()[1]
        closed.close()
        unreachable = functools.partial(service.post_refresh, environ={service.TEST_URL: 'http://127.0.0.1:%d' % port})
        delays = []
        failures = 0
        for _ in range(9):
            delay, failures = service.refresh_step(self.store, failures, due, unreachable, jitter=lambda: 0,
                                                   log=lambda *args, **kwargs: None)
            delays.append(delay)
        self.assertEqual(delays[:3], [7.5, 15, 30])
        self.assertEqual(delays[-1], service.RETRY_MAX_SECONDS / 2)
        _, post = self.fake_github((200, rotated(2)))
        self.assertEqual(service.refresh_step(self.store, failures, due, post, log=lambda *a, **k: None)[1], 0)

    def test_only_github_or_a_loopback_test_address_is_contacted(self):
        self.assertEqual(service.token_endpoint({}), ('https://github.com/login/oauth/access_token', False))
        for override in ('http://example.com:80', 'https://127.0.0.1:1', 'http://127.0.0.1.invalid:1',
                         'http://localhost:1', 'http://127.0.0.1:1/path', 'http://10.0.0.1:1'):
            with self.assertRaises(ValueError):
                service.token_endpoint({service.TEST_URL: override})


class SocketTests(ServiceTestCase):
    def setUp(self):
        super().setUp()
        self.install()
        self.path = self.root / 'run/worker/github.sock'
        server = service.listen(self.path)
        self.addCleanup(server.close)
        threading.Thread(target=service.accept_forever, args=(server, self.store), daemon=True).start()
        for name, value in [('ALLOWED_UIDS', (os.getuid(),))]:
            patcher = mock.patch.object(service, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)
        patcher = mock.patch.object(auth, 'SERVICE_SOCKET', self.path)
        patcher.start()
        self.addCleanup(patcher.stop)
        clock = mock.patch.object(service.time, 'time', return_value=NOW)
        clock.start()
        self.addCleanup(clock.stop)

    def test_agents_receive_only_granted_access_tokens(self):
        replies = []
        for path, expected in [('example/project.git', ACCESS), ('Example/Project', ACCESS),
                               ('example/library.git', ACCESS), ('example/other.git', ''),
                               ('example/library.git/git-receive-pack', ''),
                               ('example/project.git/git-receive-pack', ACCESS)]:
            reply = auth.service_credential('protocol=https\nhost=github.com\npath=' + path + '\n')
            self.assertEqual(reply, 'username=x-access-token\npassword=' + expected + '\n\n' if expected else '', path)
            replies.append(reply)
        for request in ({'request': 'credential', 'protocol': 'http', 'host': 'github.com', 'path': 'example/project'},
                        {'request': 'credential', 'protocol': 'https', 'host': 'github.com.invalid',
                         'path': 'example/project'},
                        {'request': 'gh-token', 'repository': 'example/other'}, {'request': 'refresh'}, ['x']):
            reply = auth.ask_service(request)
            self.assertFalse(reply['ok'], request)
            replies.append(json.dumps(reply))
        reply = auth.ask_service({'request': 'gh-token', 'repository': 'example/library'})
        self.assertEqual((reply['token'], reply['repository'], reply['access']), (ACCESS, 'example/library', 'read'))
        replies.append(json.dumps(reply))
        self.assertNotIn(REFRESH, ''.join(replies))
        log = (self.store.runtime / service.LOG).read_text()
        self.assertNotIn(ACCESS, log)
        record = json.loads(log.splitlines()[0])
        self.assertEqual((record['uid'], record['pid'], record['granted']), (os.getuid(), os.getpid(), True))
        self.assertEqual(self.store.runtime.stat().st_mode & 0o777, 0o700)

    def test_other_accounts_are_refused_and_an_expired_token_is_withheld(self):
        with mock.patch.object(service, 'ALLOWED_UIDS', ()):
            reply = auth.ask_service({'request': 'gh-token', 'repository': 'example/project'})
        self.assertEqual((reply['ok'], reply['state']), (False, 'refused'))
        with mock.patch.object(service.time, 'time', return_value=chain()['access_expires_at'] - 30):
            self.assertEqual(auth.service_credential('protocol=https\nhost=github.com\npath=example/project\n'), '')

    def test_without_a_chain_the_helper_falls_back_to_its_private_file(self):
        service.clear(self.store)
        self.assertIsNone(auth.ask_service({'request': 'gh-token', 'repository': 'example/project'}))
        self.assertIsNone(auth.service_credential('protocol=https\nhost=github.com\npath=example/project\n'))
        with mock.patch.object(auth, 'SERVICE_SOCKET', self.root / 'missing.sock'):
            self.assertIsNone(auth.ask_service({'request': 'gh-token', 'repository': 'example/project'}))

    def test_gh_receives_the_targeted_token_and_drops_only_an_injected_one(self):
        env = auth.service_environment({'GH_REPO': 'example/library'}, ['pr', 'list'])
        self.assertEqual((env['GH_TOKEN'], env['GH_REPO'], env['GH_HOST'], env[auth.INJECTED]),
                         (ACCESS, 'example/library', 'github.com', '1'))
        with contextlib.redirect_stderr(io.StringIO()):
            nested = auth.service_environment(dict(env, GH_REPO='example/other'), ['pr', 'list'])
            own = auth.service_environment({'GH_REPO': 'example/other', 'GH_TOKEN': 'own-token'}, ['pr', 'list'])
        self.assertNotIn('GH_TOKEN', nested)
        self.assertNotIn(auth.INJECTED, nested)
        self.assertEqual(own['GH_TOKEN'], 'own-token')


class SupervisionContractTests(unittest.TestCase):
    def test_supervisor_declares_the_chain_service(self):
        declared = subprocess.run(['python3', str(HERE / 'horizon-worker-supervise'), '--github-chain-contract'],
                                  capture_output=True, timeout=10)
        self.assertEqual((declared.returncode, declared.stdout), (0, b'horizon-github-chain-contract=1\n'))

    def test_operations_other_than_usage_need_root(self):
        for argv, status in [(['status'], 1), (['serve'], 1), (['unknown'], 2), ([], 2)]:
            with mock.patch.object(service.os, 'geteuid', return_value=1000), \
                    contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(service.main(argv), status, argv)


if __name__ == '__main__':
    unittest.main()
