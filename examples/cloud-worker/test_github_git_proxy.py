"""The service's Git proxy: a real Git client fetches and pushes through it to a fake GitHub
that runs `git http-backend` on the loopback address. Synthetic tokens only."""
import base64
import contextlib
import http.server
import json
import os
from pathlib import Path
import shutil
import socket
import ssl
import subprocess
import threading
import time
import unittest
from unittest import mock

from test_horizon_worker_github import ACCESS, NOW, REFRESH, ServiceTestCase, chain, installation, service

gitproxy = service.gitproxy
relay = gitproxy.relay
auth = service.auth
PRIVATE = ('example/project', 'example/library', 'example/secret', 'example/bound')
# The token of a static binding from cloud settings.
STATIC = 'ghp_synthetic-static'


class FakeGitHub(http.server.ThreadingHTTPServer):
    """Smart HTTP for bare repositories under `root`. A private repository answers only the
    access token as Basic credentials; every request's Authorization is recorded."""

    def __init__(self, root):
        super().__init__(('127.0.0.1', 0), GitHandler)
        self.root, self.seen = root, []
        # How Git replies end: `length`, `chunked`, or `close` (the end of the connection).
        self.framing = 'length'
        threading.Thread(target=self.serve_forever, daemon=True).start()

    @property
    def url(self):
        return 'http://127.0.0.1:%d' % self.server_address[1]


class GitHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.backend()

    def do_POST(self):
        self.backend()

    def body(self):
        if self.headers.get('Transfer-Encoding', '').lower() == 'chunked':
            data = b''
            while True:
                size = int(self.rfile.readline().split(b';')[0], 16)
                if size == 0:
                    self.rfile.readline()
                    return data, True
                data += self.rfile.read(size)
                self.rfile.read(2)
        return self.rfile.read(int(self.headers.get('Content-Length', '0'))), False

    def backend(self):
        path, _, query = self.path.partition('?')
        repository = '/'.join(path.split('/')[1:3]).removesuffix('.git')
        authorization = self.headers.get('Authorization')
        data, chunked = self.body()
        self.server.seen.append((self.command, path, authorization, chunked))
        expected = ['Basic ' + base64.b64encode(('x-access-token:' + token).encode()).decode()
                    for token in (ACCESS, STATIC)]
        if repository == 'example/renamed':
            self.send_response(301)
            self.send_header('Location', 'https://github.com/example/public.git' + path.split('.git', 1)[1]
                             + ('?' + query if query else ''))
            self.send_header('Content-Length', '0')
            self.end_headers()
            return
        if repository in PRIVATE and authorization not in expected:
            self.send_response(401)
            self.send_header('WWW-Authenticate', 'Basic realm="GitHub"')
            self.send_header('Content-Type', 'text/plain')
            self.end_headers()
            self.wfile.write(b'Authentication required\n')
            return
        if path.endswith('/info/lfs/objects/batch'):
            reply = json.dumps({'objects': [], 'echo': authorization or ''}).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/vnd.git-lfs+json')
            self.send_header('Set-Cookie', 'session=synthetic')
            self.end_headers()
            self.wfile.write(reply)
            return
        bare = repository + '.git'
        environment = dict(os.environ, GIT_PROJECT_ROOT=str(self.server.root), GIT_HTTP_EXPORT_ALL='1',
                           PATH_INFO='/' + bare + path.split(bare, 1)[1] if bare in path else path,
                           REQUEST_METHOD=self.command, QUERY_STRING=query, REMOTE_USER='synthetic',
                           CONTENT_TYPE=self.headers.get('Content-Type', ''), CONTENT_LENGTH=str(len(data)),
                           HTTP_CONTENT_ENCODING=self.headers.get('Content-Encoding', ''),
                           GIT_PROTOCOL=self.headers.get('Git-Protocol', ''))
        output = subprocess.run(['git', 'http-backend'], input=data, env=environment, capture_output=True).stdout
        head, _, payload = output.partition(b'\r\n\r\n')
        status = 200
        fields = []
        for line in head.decode().split('\r\n'):
            name, _, value = line.partition(':')
            if name.lower() == 'status':
                status = int(value.split()[0])
            elif name:
                fields.append((name, value.strip()))
        if self.server.framing == 'chunked':
            self.protocol_version = 'HTTP/1.1'
        self.send_response(status)
        for field in fields:
            self.send_header(*field)
        if self.server.framing == 'length':
            self.send_header('Content-Length', str(len(payload)))
        elif self.server.framing == 'chunked':
            self.send_header('Transfer-Encoding', 'chunked')
            self.send_header('Connection', 'close')
            payload = b'%X\r\n%s\r\n0\r\n\r\n' % (len(payload), payload) if payload else b'0\r\n\r\n'
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *args):
        pass


def git(*args, cwd=None, env=None, check=True):
    result = subprocess.run(['git', *args], cwd=cwd, env=env, capture_output=True, text=True)
    if check and result.returncode:
        raise AssertionError('git %s failed: %s' % (' '.join(args), result.stderr))
    return result


def bind_static(path, *repositories):
    """A static binding from cloud settings, as the Git helper writes it."""
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    grants = [{'repository': repository, 'token': STATIC, 'author_name': 'Agent', 'author_email': 'a@example.invalid',
               'target': 'primary' if index == 0 else 'sibling:s%d' % index}
              for index, repository in enumerate(repositories)]
    path.write_text(json.dumps({'version': 2, 'grants': grants}))
    path.chmod(0o600)


@unittest.skipUnless(shutil.which('git') and shutil.which('openssl') and Path('/proc/net/tcp').exists(),
                     'needs Git, OpenSSL and Linux /proc')
class GitProxyTests(ServiceTestCase):
    def setUp(self):
        super().setUp()
        self.install()
        static = mock.patch.object(auth, 'CREDENTIAL', self.root / 'credentials/github.json')
        static.start()
        self.addCleanup(static.stop)
        self.repositories = self.root / 'github'
        seed = self.root / 'seed'
        env = self.environment()
        git('init', '-q', '-b', 'main', str(seed), env=env)
        git('-c', 'user.name=Seed', '-c', 'user.email=seed@example.invalid', 'commit', '-q', '--allow-empty',
            '-m', 'seed', cwd=seed, env=env)
        for name in (*PRIVATE, 'example/public'):
            git('clone', '-q', '--bare', str(seed), str(self.repositories / (name + '.git')), env=env)
        self.github = FakeGitHub(self.repositories)
        self.addCleanup(self.github.server_close)
        self.addCleanup(self.github.shutdown)
        self.server = gitproxy.listen(('127.0.0.1', 0))
        private = self.root / 'private'
        private.mkdir(mode=0o700)
        self.authority = self.root / 'authority/ca.pem'
        proxy = gitproxy.Proxy(self.server, gitproxy.make_identity(private, self.authority))
        self.addCleanup(proxy.close)
        self.allowed = [os.getuid()]
        threading.Thread(target=gitproxy.accept_forever, daemon=True, args=(
            proxy, self.store, self.allowed, {service.TEST_URL: self.github.url})).start()
        clock = mock.patch.object(gitproxy.time, 'time', return_value=NOW)
        clock.start()
        self.addCleanup(clock.stop)

    def environment(self):
        home = self.root / 'home'
        home.mkdir(exist_ok=True)
        return dict(os.environ, HOME=str(home), GIT_CONFIG_NOSYSTEM='1', GIT_TERMINAL_PROMPT='0',
                    GIT_CONFIG_GLOBAL=str(home / '.gitconfig'))

    def routed(self):
        """Git configured the way horizon-worker-git-auth routes it, at the test's port."""
        env = self.environment()
        route = (('http.https://github.com/.proxy', 'http://127.0.0.1:%d' % self.server.getsockname()[1]),
                 ('http.https://github.com/.sslCAInfo', str(self.authority)))
        with mock.patch.object(auth, 'ROUTE', route):
            auth.route(env, True)
        git('config', '--global', 'user.name', 'Agent', env=env)
        git('config', '--global', 'user.email', 'agent@example.invalid', env=env)
        return env

    def clone(self, repository, env):
        target = self.root / 'clones' / repository
        return git('clone', '-q', 'https://github.com/%s.git' % repository, str(target), env=env, check=False), target

    def push(self, worktree, env):
        git('commit', '-q', '--allow-empty', '-m', 'change', cwd=worktree, env=env)
        return git('push', '-q', 'origin', 'HEAD:main', cwd=worktree, env=env, check=False)

    def test_git_fetches_and_pushes_granted_repositories_with_a_token_it_never_sees(self):
        env = self.routed()
        result, project = self.clone('example/project', env)
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.push(project, env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(git('--git-dir=' + str(self.repositories / 'example/project.git'), 'log', '-1',
                             '--format=%s', 'main', env=env).stdout.strip(), 'change')
        # A push larger than Git's post buffer streams chunked through the proxy.
        (project / 'large').write_bytes(os.urandom(256 * 1024))
        git('add', 'large', cwd=project, env=env)
        git('-c', 'http.postBuffer=4096', 'commit', '-q', '-m', 'large', cwd=project, env=env)
        result = git('-c', 'http.postBuffer=4096', 'push', '-q', 'origin', 'HEAD:main', cwd=project, env=env,
                     check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(True, [chunked for *_, chunked in self.github.seen])
        outputs = json.dumps([result.stdout, result.stderr])
        self.assertNotIn(ACCESS, outputs)
        for config in (self.root / 'home').rglob('*'):
            if config.is_file():
                self.assertNotIn(ACCESS, config.read_text(errors='replace'))
        log = (self.store.runtime / service.agents.LOG).read_text()
        self.assertNotIn(ACCESS, log)
        self.assertNotIn(REFRESH, log)
        records = [json.loads(line) for line in log.splitlines()]
        self.assertIn({'request': 'git-push', 'repository': 'example/project', 'granted': True, 'uid': os.getuid()},
                      [{key: record[key] for key in ('request', 'repository', 'granted', 'uid')} for record in records])

    def test_replies_of_unknown_length_reach_git_with_a_clear_end(self):
        # GitHub sends Git replies chunked; the proxy chunks them again and closes TLS with
        # close_notify, so GnuTLS does not report a cut connection.
        env = self.routed()
        for framing in ('chunked', 'close'):
            with self.subTest(framing=framing):
                self.github.framing = framing
                for repository in ('example/project', 'example/public'):
                    result, worktree = self.clone(repository, env)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    result = git('fetch', '-q', 'origin', cwd=worktree, env=env, check=False)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    shutil.rmtree(worktree)
                result, project = self.clone('example/project', env)
                result = self.push(project, env)
                self.assertEqual(result.returncode, 0, result.stderr)
                shutil.rmtree(project)

    def test_reads_of_other_repositories_never_carry_the_token_and_pushes_are_refused(self):
        env = self.routed()
        result, public = self.clone('example/public', env)
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.push(public, env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('remote: Horizon: example/public has no GitHub grant on this worker', result.stderr)
        result, _ = self.clone('example/secret', env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('remote: Horizon: example/secret has no GitHub grant on this worker', result.stderr)
        self.assertNotIn('Username', result.stderr, 'Git must never prompt for credentials')
        authorized = {path.split('/')[2] for _, path, authorization, _ in self.github.seen if authorization}
        self.assertEqual(authorized, set(), 'only granted repositories get the token')
        self.assertNotIn('git-receive-pack', ''.join(path for _, path, _, _ in self.github.seen))

    def test_a_read_grant_fetches_but_never_pushes(self):
        env = self.routed()
        result, library = self.clone('example/library', env)
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.push(library, env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('remote: Horizon: example/library is granted for reading only', result.stderr)

    def test_a_revoked_or_missing_chain_serves_no_token(self):
        env = self.routed()
        self.store.save(dict(self.stored(), state='revoked'))
        result, _ = self.clone('example/project', env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('remote: Horizon: GitHub access on this worker was revoked', result.stderr)
        result, public = self.clone('example/public', env)
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.push(public, env)
        self.assertIn('remote: Horizon: GitHub access on this worker was revoked', result.stderr)
        service.clear(self.store, retire=lambda: None)
        result = self.push(public, env)
        self.assertIn('remote: Horizon: this worker has no GitHub sign-in', result.stderr)
        self.assertEqual([authorization for _, _, authorization, _ in self.github.seen], [None] * len(self.github.seen))

    def test_a_static_binding_serves_repositories_the_chain_has_no_grant_for(self):
        env = self.routed()
        bind_static(auth.CREDENTIAL, 'example/bound', 'example/project')
        result, bound = self.clone('example/bound', env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.push(bound, env).returncode, 0)
        # The chain's grant comes first for a repository both cover.
        self.clone('example/project', env)
        tokens = {path.split('/')[2]: base64.b64decode(authorization.split()[1]).decode().split(':')[1]
                  for _, path, authorization, _ in self.github.seen if authorization}
        self.assertEqual(tokens, {'bound.git': STATIC, 'project.git': ACCESS})
        # Without a chain the binding still reaches GitHub only through the proxy.
        service.clear(self.store, retire=lambda: None)
        self.assertEqual(self.push(bound, env).returncode, 0)
        result = self.push(self.clone('example/public', env)[1], env)
        self.assertIn('remote: Horizon: this worker has no GitHub sign-in', result.stderr)

    def test_a_renamed_repository_is_followed_and_checked_again(self):
        env = self.routed()
        result, _ = self.clone('example/renamed', env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('/example/public.git/info/refs', [path for _, path, _, _ in self.github.seen])

    def test_only_the_agent_account_may_open_a_tunnel_and_only_to_github(self):
        self.assertTrue(self.tunnel(b'example.com:443').startswith(b'HTTP/1.1 403'))
        self.assertTrue(self.tunnel(b'github.com:443 extra').startswith(b'HTTP/1.1 400'))
        self.allowed[:] = [os.getuid() + 1]
        self.assertTrue(self.tunnel().startswith(b'HTTP/1.1 403'))
        result, _ = self.clone('example/public', self.routed())
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.github.seen, [])

    def test_a_slow_client_loses_its_connection(self):
        with mock.patch.object(gitproxy, 'HEAD_SECONDS', 1):
            started = time.monotonic()
            with self.tunnel() as connection:
                connection.sendall(b'GET /example/public.git/info/refs?service=git-upload-pack HTTP/1.1\r\n')
                with contextlib.suppress(ssl.SSLError, OSError):
                    self.assertEqual(connection.recv(1), b'')
        self.assertLess(time.monotonic() - started, 8)

    def test_an_lfs_batch_is_read_once_with_unique_keys_and_after_a_continue(self):
        def batch(body, expect=False):
            with self.tunnel() as connection:
                connection.sendall(b'POST /example/library.git/info/lfs/objects/batch HTTP/1.1\r\nHost: github.com\r\n'
                                   b'Content-Length: %d\r\n%s\r\n' % (len(body), b'Expect: 100-continue\r\n' * expect))
                if expect:
                    self.assertTrue(connection.recv(64).startswith(b'HTTP/1.1 100 Continue'))
                connection.sendall(body)
                data = b''
                with contextlib.suppress(ssl.SSLError):
                    while chunk := connection.recv(65536):
                        data += chunk
            return data
        self.assertIn(b'malformed Git LFS batch request', batch(b'{"operation":"upload","operation":"download"}'))
        self.assertIn(b'granted for reading only', batch(b'{"operation":"upload","objects":[]}', expect=True))

    def tunnel(self, target=b'github.com:443'):
        """A connection with an open tunnel to the proxy's github.com, or the refusal."""
        connection = socket.create_connection(self.server.getsockname(), timeout=10)
        connection.sendall(b'CONNECT %s HTTP/1.1\r\nHost: %s\r\n\r\n' % (target, target))
        head = b''
        while not head.endswith(b'\r\n\r\n'):
            byte = connection.recv(1)
            if not byte:
                break
            head += byte
        if not head.startswith(b'HTTP/1.1 200'):
            connection.close()
            return head
        client = ssl.create_default_context(cafile=str(self.authority))
        return client.wrap_socket(connection, server_hostname='github.com')

    def exchange(self, raw):
        with self.tunnel() as connection:
            connection.sendall(raw)
            data = b''
            with contextlib.suppress(ssl.SSLError):
                while chunk := connection.recv(65536):
                    data += chunk
        return data

    def test_requests_that_parsers_could_read_differently_or_that_leave_the_repository_are_refused(self):
        def request(target, *fields, method=b'GET', body=b''):
            return b'%s %s HTTP/1.1\r\nHost: github.com\r\n%s\r\n%s' % (
                method, target, b''.join(field + b'\r\n' for field in fields), body)
        refs = b'/example/project.git/info/refs?service=git-upload-pack'
        for raw, status in [
                (request(b'/example/project/../secret/info/refs?service=git-upload-pack'), b'404'),
                (request(b'/example/%2e%2e/info/refs?service=git-upload-pack'), b'404'),
                (request(b'/example/project.git/info/refs?service=git-upload-pack&x=1'), b'405'),
                (request(b'/example/project.git/objects/info/packs'), b'404'),
                (request(b'/example/project.git/git-receive-pack', method=b'GET'), b'405'),
                (request(b'http://github.com/example/project.git/info/refs?service=git-upload-pack'), b'400'),
                (request(refs, b'Content-Length: 0', b'Transfer-Encoding: chunked'), b'400'),
                (request(refs, b'Transfer-Encoding: gzip, chunked'), b'400'),
                (request(refs, b'Content-Length: 1', b'Content-Length: 1'), b'400'),
                (request(refs, b'X-Folded: a', b' continued'), b'400'),
                (request(refs, b'X-Bytes: \xff'), b'400'),
                (request(refs, b'X-Long: ' + b'a' * 20000), b'431'),
                (b'GET ' + refs + b' HTTP/1.1\nHost: x\n\n', b'400')]:
            reply = self.exchange(raw)
            self.assertTrue(reply.startswith(b'HTTP/1.1 ' + status), (raw[:80], reply[:120]))
        self.assertEqual(self.github.seen, [], 'nothing malformed reaches GitHub')
        # The client's own credentials never reach GitHub; the proxy's replace them.
        reply = self.exchange(request(refs, b'Authorization: Basic c3ludGhldGljOnN5bnRoZXRpYw==',
                                      b'Cookie: session=x'))
        self.assertTrue(reply.startswith(b'HTTP/1.1 200'), reply[:200])
        self.assertTrue(self.github.seen[-1][2].startswith('Basic '))
        self.assertNotIn(b'synthetic', base64.b64decode(self.github.seen[-1][2].split()[1]).split(b':')[0])

    def test_git_lfs_batch_access_follows_the_operation_and_hides_echoed_credentials(self):
        def batch(repository, operation):
            body = json.dumps({'operation': operation, 'objects': []}).encode()
            return self.exchange(b'POST /%s.git/info/lfs/objects/batch HTTP/1.1\r\nHost: github.com\r\n'
                                 b'Content-Type: application/vnd.git-lfs+json\r\nContent-Length: %d\r\n\r\n%s'
                                 % (repository.encode(), len(body), body))
        self.assertIn(b'granted for reading only', batch('example/library', 'upload'))
        self.assertIn(b'malformed Git LFS batch request', batch('example/library', 'delete'))
        # The fake echoes the Authorization it got, which a relay must never pass on.
        reply = batch('example/library', 'download')
        self.assertTrue(reply.startswith(b'HTTP/1.1 502'), reply[:200])
        self.assertNotIn(ACCESS.encode(), reply)
        self.assertNotIn(base64.b64encode(('x-access-token:' + ACCESS).encode()), reply)
        reply = batch('example/public', 'download')
        self.assertTrue(reply.startswith(b'HTTP/1.1 200'), reply[:200])
        self.assertNotIn(b'Set-Cookie', reply)


class OperationTests(ServiceTestCase):
    def test_only_git_and_lfs_endpoints_of_one_repository_are_relayed(self):
        for method, target, expected in [
                ('GET', '/example/project.git/info/refs?service=git-upload-pack', ('example/project', 'read', 'git')),
                ('GET', '/Example/Project/info/refs?service=git-receive-pack', ('Example/Project', 'push', 'git')),
                ('POST', '/example/project.git/git-upload-pack', ('example/project', 'read', 'git')),
                ('POST', '/example/project.git/git-receive-pack', ('example/project', 'push', 'git')),
                ('POST', '/example/project.git/info/lfs/objects/batch', ('example/project', None, 'lfs')),
                ('GET', '/example/project.git/info/lfs/locks?path=a%2Fb&limit=10', ('example/project', 'read', 'lfs')),
                ('POST', '/example/project.git/info/lfs/locks', ('example/project', 'push', 'lfs')),
                ('POST', '/example/project.git/info/lfs/locks/verify', ('example/project', 'push', 'lfs')),
                ('POST', '/example/project.git/info/lfs/locks/abc-1/unlock', ('example/project', 'push', 'lfs'))]:
            self.assertEqual(gitproxy.operation(method, target), expected, target)
        for method, target in [('GET', '/example/../info/refs?service=git-upload-pack'),
                               ('GET', '/example/..git/info/refs?service=git-upload-pack'),
                               ('GET', '/example/project.git/info/refs'),
                               ('GET', '/example/project.git/info/refs?service=git-upload-pack#x'),
                               ('POST', '/example/project.git/info/refs?service=git-upload-pack'),
                               ('POST', '/example/project.git/git-upload-pack?x'),
                               ('PUT', '/example/project.git/git-receive-pack'),
                               ('GET', '/example/project.git/info/lfs/locks?path=<script>'),
                               ('GET', '/example/project/HEAD'), ('GET', '/repos/example/project')]:
            with self.assertRaises(relay.Refusal, msg=target):
                gitproxy.operation(method, target)

    def test_the_plan_adds_the_token_only_within_the_grants(self):
        self.install()

        def chosen(repository, access, now=NOW):
            result = gitproxy.plan(self.store, repository, access, now)
            return result.repository, result.token
        self.assertEqual(chosen('Example/Project', 'push'), ('example/project', ACCESS))
        self.assertEqual(chosen('example/library', 'read'), ('example/library', ACCESS))
        self.assertEqual(chosen('example/other', 'read'), ('example/other', None))
        self.assertIn('no GitHub grant', gitproxy.plan(self.store, 'example/other', 'read', NOW).refusal)
        for repository, access in [('example/library', 'push'), ('example/other', 'push')]:
            with self.assertRaises(relay.Refusal):
                gitproxy.plan(self.store, repository, access, NOW)
        with self.assertRaisesRegex(relay.Refusal, 'could not refresh'):
            gitproxy.plan(self.store, 'example/project', 'read', chain()['access_expires_at'] - 30)
        self.install(installation(chain=chain(access='ghu_synthetic-two')))
        self.store.pending = dict(self.stored(), serial=self.stored()['serial'] + 1, chain=chain(access='ghu_unwritten'))
        with self.assertRaisesRegex(relay.Refusal, 'not stored yet'):
            gitproxy.plan(self.store, 'example/project', 'push', NOW)
        unstored = gitproxy.plan(self.store, 'example/project', 'read', NOW)
        self.assertEqual((unstored.token, unstored.refusal),
                         (None, 'the GitHub sign-in of this worker is not stored yet; try again in a minute.'))

    def test_a_closed_or_half_open_socket_names_no_account(self):
        tables = (self.root / 'tcp', self.root / 'tcp6')
        server = gitproxy.listen(('127.0.0.1', 0))
        self.addCleanup(server.close)
        with socket.create_connection(server.getsockname()) as client:
            accepted, _ = server.accept()
            self.addCleanup(accepted.close)
            local = '0100007F:%04X' % client.getsockname()[1]
            remote = '0100007F:%04X' % server.getsockname()[1]
            mapped = '0000000000000000FFFF0000' + local
            head = 'sl local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode\n'
            for row, owner in [('%s %s 01 0:0 00:0 0 1000 0 4242', 1000), ('%s %s 06 0:0 00:0 0 0 0 0', None),
                               ('%s %s 01 0:0 00:0 0 0 0 0', None), ('%s %s 08 0:0 00:0 0 1000 0 4242', None)]:
                tables[0].write_text(head + '0: ' + row % (local, remote) + '\n')
                tables[1].write_text(head)
                self.assertEqual(gitproxy.socket_owner(accepted, tables), owner, row)
            tables[0].write_text(head)
            tables[1].write_text(head + '0: %s %s 01 0:0 00:0 0 1000 0 4242\n' % (
                mapped, '0000000000000000FFFF0000' + remote))
            self.assertEqual(gitproxy.socket_owner(accepted, tables), 1000, 'an IPv6 client of the IPv4 port')
            self.assertFalse(gitproxy.owner_allowed(accepted, (1000,), (self.root / 'missing',)))

    def test_github_progress_is_passed_on_as_it_arrives(self):
        class Reply:
            status, reason, length = 200, 'OK', None
            pieces = [b'progress 1', b'progress 2']

            def getheaders(self):
                return []

            def read1(self, _):
                return self.pieces.pop(0) if self.pieces else b''

        class Output:
            writes, flushed = [], []

            def write(self, data):
                self.writes.append(data)

            def flush(self):
                self.flushed.append(len(self.writes))
        output = Output()
        relay.relay(output, Reply(), set())
        self.assertEqual(output.writes[1:], [b'progress 1', b'progress 2'])
        self.assertTrue({2, 3} <= set(output.flushed), 'each piece is flushed when it arrives')

    def test_the_service_routes_git_after_the_repositories_are_reconciled(self):
        steps = []
        with mock.patch.object(service, 'reconcile', lambda store: steps.append('reconcile')):
            service.prepare(self.store, route=lambda: steps.append('route'))
        self.assertEqual(steps, ['reconcile', 'route'])

    def test_the_kernel_names_the_account_that_opened_a_loopback_connection(self):
        if not Path('/proc/net/tcp').exists():
            self.skipTest('needs Linux /proc')
        server = gitproxy.listen(('127.0.0.1', 0))
        self.addCleanup(server.close)
        with socket.create_connection(server.getsockname()) as client:
            accepted, _ = server.accept()
            with accepted:
                self.assertEqual(gitproxy.socket_owner(accepted), os.getuid())
            client.close()

    def test_the_socket_never_hands_a_token_to_git(self):
        self.install()
        reply, _ = self.answer({'request': 'credential', 'protocol': 'https', 'host': 'github.com',
                                'path': 'example/project.git'}, NOW, ('@1', 'agent-alpha', 'claude'))
        self.assertFalse(reply['ok'])
        self.assertNotIn(ACCESS, json.dumps(reply))


if __name__ == '__main__':
    unittest.main()
