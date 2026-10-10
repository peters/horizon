"""The service's API broker: raw HTTP and a real gh reach it on its Unix socket, and it sends
what the policy allows to a fake GitHub on the loopback address. Synthetic tokens only."""
import contextlib
import http.server
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import threading
import time
import unittest
from unittest import mock

from test_github_git_proxy import STATIC, bind_static
from test_github_graphql_policy import TYPES
from test_horizon_worker_github import ACCESS, NOW, ServiceTestCase, service

broker = service.broker
rest = broker.rest
auth = service.auth
SCHEMA = {'data': {'__schema': {'queryType': {'name': 'Query'}, 'mutationType': {'name': 'Mutation'},
                                'types': TYPES}}}
IDS = {'111': 'example/project', '222': 'example/secret'}


class FakeAPI(http.server.ThreadingHTTPServer):
    """GitHub's API, uploads and content hosts on one port, told apart by the Host field.
    Every request is recorded with its Authorization; `graphql` answers GraphQL bodies."""

    def __init__(self):
        super().__init__(('127.0.0.1', 0), APIHandler)
        self.seen = []
        self.graphql = lambda body: {'data': {}}
        threading.Thread(target=self.serve_forever, daemon=True).start()

    @property
    def url(self):
        return 'http://127.0.0.1:%d' % self.server_address[1]


class APIHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.answer()

    def do_POST(self):
        self.answer()

    def do_PATCH(self):
        self.answer()

    def do_DELETE(self):
        self.answer()

    def answer(self):
        data = self.rfile.read(int(self.headers.get('Content-Length', '0')))
        host, path = self.headers.get('Host'), self.path
        self.server.seen.append((self.command, host, path, self.headers.get('Authorization'), data))
        if host == 'codeload.github.com':
            return self.reply(200, b'archive', 'application/x-gzip')
        if path == '/graphql':
            body = json.loads(data)
            reply = SCHEMA if body['query'] == broker.graphql.INTROSPECTION else self.server.graphql(body)
            return self.reply(200, json.dumps(reply).encode())
        if path.startswith('/repositories/') and path.count('/') == 2:
            name = IDS.get(path.split('/')[2])
            return self.reply(200 if name else 404, json.dumps({'full_name': name}).encode())
        if path.endswith('/tarball'):
            self.send_response(302)
            self.send_header('Location', 'https://codeload.github.com/example/project/legacy.tar.gz/main?sig=x')
            self.send_header('Content-Length', '0')
            self.end_headers()
            return
        self.reply(200, json.dumps({'path': path}).encode(), link='<https://api.github.com/repositories/111/issues'
                                                                  '?page=2>; rel="next"')

    def reply(self, status, data, content_type='application/json', link=None):
        self.send_response(status)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(data)))
        self.send_header('Set-Cookie', 'session=synthetic')
        if link:
            self.send_header('Link', link)
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


class BrokerTestCase(ServiceTestCase):
    def setUp(self):
        super().setUp()
        self.install()
        self.credential = self.root / 'credentials/github.json'
        static = mock.patch.object(auth, 'CREDENTIAL', self.credential)
        static.start()
        self.addCleanup(static.stop)
        self.api = FakeAPI()
        self.addCleanup(self.api.server_close)
        self.addCleanup(self.api.shutdown)
        self.socket = self.root / 'run/api.sock'
        server = broker.listen(self.socket)
        self.addCleanup(server.close)
        self.addCleanup(lambda: server.shutdown(socket.SHUT_RDWR) if server.fileno() >= 0 else None)
        self.allowed = [os.getuid()]
        self.broker = broker.Broker(self.store, {service.TEST_URL: self.api.url})
        threading.Thread(target=broker.accept_forever, args=(server, self.broker, self.allowed), daemon=True).start()
        clock = mock.patch.object(broker.time, 'time', return_value=NOW)
        clock.start()
        self.addCleanup(clock.stop)

    def send(self, method, target, host='api.github.com', body=None, fields=()):
        """(status, fields, body) of one raw request on the broker's socket."""
        data = b'' if body is None else (body if isinstance(body, bytes) else json.dumps(body).encode())
        head = ['%s %s HTTP/1.1' % (method, target), 'Host: ' + host, 'Authorization: token ' + broker.PLACEHOLDER,
                'Content-Length: %d' % len(data), 'Connection: close', *fields]
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.settimeout(10)
            client.connect(str(self.socket))
            client.sendall(('\r\n'.join(head) + '\r\n\r\n').encode() + data)
            reply = b''
            while chunk := client.recv(65536):
                reply += chunk
        head, _, payload = reply.partition(b'\r\n\r\n')
        lines = head.decode('latin-1').split('\r\n')
        fields = {}
        for line in lines[1:]:
            name, _, value = line.partition(':')
            fields[name.lower()] = value.strip()
        return int(lines[0].split()[1]), fields, payload

    def records(self, count=1, seconds=5):
        log = self.store.runtime / service.agents.LOG
        deadline = time.monotonic() + seconds
        while True:
            lines = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
            lines = [line for line in lines if line.get('kind') == 'api']
            if len(lines) >= count or time.monotonic() > deadline:
                return lines

    def graphql(self, query, variables=None):
        status, _, payload = self.send('POST', '/graphql', body={'query': query, 'variables': variables or {}})
        self.assertEqual(status, 200)
        return json.loads(payload)


class RouteTests(unittest.TestCase):
    def test_changes_that_a_push_grant_allows_and_hosts_that_get_no_token(self):
        for method, target in [('POST', '/repos/o/r/pulls'), ('PATCH', '/repos/o/r/issues/1'),
                               ('DELETE', '/repos/o/r/git/refs/heads/topic'), ('PUT', '/repos/o/r/contents/a%20b.md'),
                               ('POST', '/repos/o/r/actions/workflows/ci.yml/dispatches'),
                               ('POST', '/repos/o/r/actions/runs/1/rerun'), ('GET', '/repos/o/r/actions/secrets')]:
            route = rest.route(method, 'api.github.com', target)
            self.assertEqual((route.repository, route.access), ('o/r', 'read' if method == 'GET' else 'push'))
        route = rest.route('GET', 'objects.githubusercontent.com', '/github-production-release-asset/1?sig=x')
        self.assertEqual((route.kind, route.token), ('content', False))
        for method, host, target in [('GET', 'objects.githubusercontent.com.example.invalid', '/'),
                                     ('GET', 'api.github.com', '/repos/o/r/%2e%2E/x'),
                                     ('GET', 'api.github.com', '/repos/o/r/contents/a%5cb'),
                                     ('GET', 'api.github.com', '/repos/o/r?'),
                                     ('GET', 'api.github.com', '/repos/o/r/issues?q=a b'),
                                     ('PUT', 'api.github.com', '/repos/o/r/topics'),
                                     ('POST', 'api.github.com', '/repos/o/r/forks'),
                                     ('GET', 'api.github.com', '/search/labels?q=repo%3Ao%2Fr'),
                                     ('GET', 'api.github.com', '/search/issues?q=repo%3Ao%2Fr&q=repo%3Ax%2Fy'),
                                     ('GET', 'api.github.com', '/graphql'), ('GET', 'api.github.com', '/user')]:
            with self.assertRaises(rest.Refused, msg=(method, host, target)):
                rest.route(method, host, target)


class RestTests(BrokerTestCase):
    def test_a_granted_repository_gets_the_token_that_gh_never_holds(self):
        status, fields, payload = self.send('GET', '/repos/example/project/pulls?state=open')
        self.assertEqual((status, json.loads(payload)), (200, {'path': '/repos/example/project/pulls?state=open'}))
        method, host, path, authorization, _ = self.api.seen[-1]
        self.assertEqual((method, host, authorization), ('GET', 'api.github.com', 'token ' + ACCESS))
        self.assertNotIn('set-cookie', fields)
        self.assertIn('repositories/111', fields['link'])
        record = self.records()[-1]
        self.assertEqual((record['request'], record['repository'], record['granted'], record['outcome']),
                         ('repository', 'example/project', True, 'relayed'))
        self.assertNotIn(ACCESS, json.dumps(self.records()))

    def test_requests_without_a_grant_or_outside_a_repository_never_reach_github(self):
        for method, target, status, reason in [
                ('GET', '/repos/example/secret/issues', 403, 'example/secret has no GitHub grant'),
                ('POST', '/repos/example/library/issues', 403, 'granted for reading only'),
                ('DELETE', '/repos/example/project', 403, 'only pull requests'),
                ('PATCH', '/repos/example/project', 403, 'only pull requests'),
                ('POST', '/repos/example/project/hooks', 403, 'only pull requests'),
                ('PUT', '/repos/example/project/actions/secrets/X', 403, 'only pull requests'),
                ('POST', '/repos/example/project/actions/workflows/ci.yml/disable', 403, 'only pull requests'),
                ('GET', '/user/repos', 403, 'only a granted repository'),
                ('GET', '/orgs/example/repos', 403, 'only a granted repository'),
                ('GET', '/repos/example/project/../secret', 400, 'dot segment'),
                ('GET', '/repos/example/project/contents/a%2F..%2F..', 400, 'encoded slash'),
                ('GET', '/search/issues?q=is%3Aopen', 403, 'needs a repo: qualifier'),
                ('GET', '/search/issues?q=repo%3Aexample%2Fproject+OR+repo%3Aexample%2Fsecret', 403, 'cannot use OR'),
                ('GET', '/search/issues?q=repo%3Aexample%2Fproject+repo%3Aexample%2Fsecret', 403, 'example/secret'),
                ('GET', '/repositories/222/issues', 403, 'has no GitHub grant'),
                ('GET', '/repositories/999', 403, 'has no GitHub grant')]:
            seen = len([item for item in self.api.seen if not item[2].startswith('/repositories/')])
            status_seen, _, payload = self.send(method, target)
            self.assertEqual(status_seen, status, (target, payload))
            message = json.loads(payload)['message']
            self.assertIn(reason, message, target)
            self.assertTrue(message.startswith('Horizon: '))
            self.assertEqual(len([item for item in self.api.seen if not item[2].startswith('/repositories/')]),
                             seen, target)
        self.assertEqual(self.send('GET', '/', host='evil.example')[0], 403)

    def test_paths_by_id_searches_and_uploads(self):
        status, _, payload = self.send('GET', '/repositories/111/issues?page=2')
        self.assertEqual((status, json.loads(payload)['path']), (200, '/repositories/111/issues?page=2'))
        self.assertEqual(self.send('GET', '/search/issues?q=repo%3Aexample%2Fproject+is%3Aopen')[0], 200)
        self.assertEqual(self.send('GET', '/rate_limit')[0], 200)
        self.assertEqual(self.api.seen[-1][3], 'token ' + ACCESS)
        status, _, _ = self.send('POST', '/repos/example/project/releases/7/assets?name=a.zip',
                                 host='uploads.github.com', body=b'zip')
        self.assertEqual(status, 200)
        self.assertEqual(self.api.seen[-1][1:4], ('uploads.github.com', '/repos/example/project/releases/7/assets'
                                                  '?name=a.zip', 'token ' + ACCESS))
        self.assertEqual(self.send('POST', '/repos/example/library/releases/7/assets', host='uploads.github.com',
                                   body=b'zip')[0], 403)

    def test_a_redirect_to_a_content_host_comes_back_without_a_token(self):
        status, fields, _ = self.send('GET', '/repos/example/project/tarball')
        self.assertEqual(status, 302)
        location = fields['location']
        status, _, payload = self.send('GET', location.split('codeload.github.com', 1)[1],
                                       host='codeload.github.com')
        self.assertEqual((status, payload), (200, b'archive'))
        self.assertIsNone(self.api.seen[-1][3])
        self.assertEqual(self.send('POST', '/x', host='codeload.github.com')[0], 405)

    def test_the_static_binding_serves_its_repository_after_the_chain(self):
        bind_static(self.credential, 'example/bound')
        self.assertEqual(self.send('GET', '/repos/example/bound')[0], 200)
        self.assertEqual(self.api.seen[-1][3], 'token ' + STATIC)
        self.send('GET', '/repos/example/project')
        self.assertEqual(self.api.seen[-1][3], 'token ' + ACCESS)

    def test_other_accounts_and_a_missing_sign_in_are_refused(self):
        self.allowed[:] = [os.getuid() + 1]
        status, _, payload = self.send('GET', '/repos/example/project')
        self.assertEqual(status, 403)
        self.assertIn('this account may not use', json.loads(payload)['message'])
        self.allowed[:] = [os.getuid()]
        service.clear(self.store, retire=lambda: None)
        status, _, payload = self.send('GET', '/repos/example/project')
        self.assertEqual(status, 403)
        self.assertIn('no GitHub sign-in', json.loads(payload)['message'])
        self.assertEqual(self.api.seen, [])


class GraphQLTests(BrokerTestCase):
    def test_a_query_is_checked_and_the_added_fields_are_removed(self):
        def answer(body):
            held = next(word.split(':')[0] for word in body['query'].split() if word.startswith('hzr'))
            return {'data': {'repository': {'pullRequest': {'title': 't', held: {'nameWithOwner': 'example/project'}},
                                            next(word.split(':')[0] for word in body['query'].split()
                                                 if word.startswith('hzn')): 'example/project'}}}
        self.api.graphql = answer
        reply = self.graphql('{ repository(owner: "example", name: "project") { pullRequest(number: 1) { title } } }')
        self.assertEqual(reply, {'data': {'repository': {'pullRequest': {'title': 't'}}}})
        self.assertEqual(self.api.seen[-1][3], 'token ' + ACCESS)

    def test_refusals_are_graphql_errors_and_never_reach_github(self):
        reply = self.graphql('{ repository(owner: "example", name: "secret") { name } }')
        self.assertEqual(reply['data'], None)
        self.assertIn('Horizon: example/secret has no GitHub grant', reply['errors'][0]['message'])
        reply = self.graphql('mutation { createRepository(input: {name: "x"}) { repository { name } } }')
        self.assertIn('createRepository is not allowed', reply['errors'][0]['message'])
        self.assertEqual([item[4] for item in self.api.seen if b'__schema' not in item[4]], [])

    def test_a_reply_with_data_of_another_repository_is_refused(self):
        def answer(body):
            held = next(word.split(':')[0] for word in body['query'].split() if word.startswith('hzr'))
            return {'data': {'node': {'title': 'secret', held: {'nameWithOwner': 'example/secret'}}}}
        self.api.graphql = answer
        reply = self.graphql('query($id: ID!) { node(id: $id) { ...on PullRequest { title } } }', {'id': 'PR_1'})
        self.assertNotIn('secret"', json.dumps(reply['data']))
        self.assertIn('example/secret', reply['errors'][0]['message'])

    def test_a_mutation_is_sent_only_after_its_ids_resolve_to_a_pushable_repository(self):
        sent = []

        def answer(body):
            sent.append(body)
            if 'nodes(ids: $ids)' in body['query']:
                repository = 'example/project' if body['variables']['ids'] == ['I_1'] else 'example/library'
                return {'data': {'nodes': [{'__typename': 'Issue', 'held': {'nameWithOwner': repository}}]}}
            return {'data': {'addComment': {'clientMutationId': None}}}
        self.api.graphql = answer
        query = 'mutation($input: AddCommentInput!) { addComment(input: $input) { clientMutationId } }'
        reply = self.graphql(query, {'input': {'subjectId': 'I_1', 'body': 'hello'}})
        self.assertEqual(reply, {'data': {'addComment': {'clientMutationId': None}}})
        self.assertEqual(len(sent), 2)
        reply = self.graphql(query, {'input': {'subjectId': 'I_2', 'body': 'hello'}})
        self.assertIn('example/library has no GitHub grant', reply['errors'][0]['message'])
        self.assertEqual(len(sent), 3, 'the lookup only')


@unittest.skipUnless(shutil.which('gh'), 'needs gh')
class RealGhTests(BrokerTestCase):
    def gh(self, *args):
        config = self.root / 'gh'
        config.mkdir(exist_ok=True)
        (config / 'config.yml').write_text('version: "1"\nhttp_unix_socket: %s\n' % self.socket)
        env = {key: value for key, value in os.environ.items() if not key.startswith(('GH_', 'GITHUB_'))}
        env.update(GH_CONFIG_DIR=str(config), GH_TOKEN=broker.PLACEHOLDER, GH_HOST='github.com',
                   GH_PROMPT_DISABLED='1', GH_NO_UPDATE_NOTIFIER='1', HOME=str(self.root))
        return subprocess.run(['gh', *args], env=env, capture_output=True, text=True, timeout=60)

    def test_gh_reaches_granted_repositories_and_prints_refusals(self):
        result = self.gh('api', 'repos/example/project/issues')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), {'path': '/repos/example/project/issues?per_page=100'}
                         if 'per_page' in result.stdout else {'path': '/repos/example/project/issues'})
        self.assertEqual(self.api.seen[-1][3], 'token ' + ACCESS)
        result = self.gh('api', 'repos/example/secret/issues')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('example/secret has no GitHub grant', result.stderr)
        result = self.gh('api', 'graphql', '-f', 'query={ repository(owner: "example", name: "secret") { name } }')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('example/secret has no GitHub grant', result.stderr)
        result = self.gh('api', 'repos/example/project/tarball')
        self.assertEqual((result.returncode, result.stdout), (0, 'archive'), result.stderr)
        self.assertIsNone(self.api.seen[-1][3])
        self.assertTrue(all(broker.PLACEHOLDER not in (item[3] or '') for item in self.api.seen))


if __name__ == '__main__':
    unittest.main()
