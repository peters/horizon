"""Agent requests for more GitHub access, the person's decision and the github_access MCP tool,
with synthetic tokens, a fake GitHub on the loopback address and a fake session mapping."""
import functools
import http.server
import io
import json
import os
import threading
import time
import unittest
from unittest import mock

from test_horizon_worker_github import ACCESS, NOW, ServiceTestCase, agents, auth, chain, installation, service

ALPHA = ('@1', 'agent-alpha', 'claude')
BETA = ('@2', 'agent-beta', 'codex')
DAY = 24 * 3600


class FakeApi(http.server.ThreadingHTTPServer):
    """Answers GET /repos/... with the next queued (status, body) and records the requests."""

    def __init__(self):
        super().__init__(('127.0.0.1', 0), ApiHandler)
        self.seen, self.replies = [], []
        threading.Thread(target=self.serve_forever, daemon=True).start()


class ApiHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.server.seen.append((self.path, self.headers['Authorization']))
        status, body = self.server.replies.pop(0)
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(body.encode())

    def log_message(self, *args):
        pass


def repository(push):
    return 200, json.dumps({'full_name': 'example/extra', 'permissions': {'pull': True, 'push': push}})


class AccessRequestTests(ServiceTestCase):
    def setUp(self):
        super().setUp()
        self.install()

    def ask(self, who, repository='example/extra', access='push', reason='Open a PR for the fix', now=NOW):
        return self.answer({'request': 'request', 'repository': repository, 'access': access, 'reason': reason},
                           now, who)[0]

    def api(self, *replies):
        github = FakeApi()
        self.addCleanup(github.server_close)
        self.addCleanup(github.shutdown)
        github.replies.extend(replies)
        return github, functools.partial(agents.reachable, environ={service.TEST_URL: 'http://127.0.0.1:%d'
                                                                    % github.server_address[1]})

    def decide(self, identifier, decision, check=None, now=NOW):
        return agents.decide(self.store, self.book, identifier, decision, now=lambda: now,
                             check=check or mock.Mock(return_value=None))

    def token(self, who, repository='example/extra', path=None, now=NOW):
        request = ({'request': 'credential', 'protocol': 'https', 'host': 'github.com', 'path': path}
                   if path else {'request': 'gh-token', 'repository': repository})
        return self.answer(request, now, who)[0]

    def test_requests_are_validated_and_only_agent_sessions_may_ask(self):
        for repository, access, reason in [('../escape', 'push', 'x'), ('example/extra', 'admin', 'x'),
                                           ('example/extra', 'push', ''), ('example/extra', 'push', 'x' * 301),
                                           ('example/extra', 'push', 'two\nlines'), ('example/extra', 'push', None)]:
            reply = self.ask(ALPHA, repository, access, reason)
            self.assertFalse(reply['ok'], (repository, access, reason))
        self.assertTrue(self.ask(ALPHA, reason='x' * 300)['ok'])
        reply = self.ask(None)
        self.assertEqual((reply['ok'], reply['state']), (False, 'refused'))

    def test_one_pending_request_per_session_and_repository_with_a_bound(self):
        first = self.ask(ALPHA)
        self.assertEqual((first['status'], len(first['id'])), ('pending', 16))
        self.assertEqual(self.ask(ALPHA, access='read')['id'], first['id'])
        self.assertNotEqual(self.ask(BETA)['id'], first['id'])
        # A repository the installed grants already cover needs no request; wider access does.
        self.assertEqual(self.ask(BETA, 'Example/Project'), {'ok': True, 'state': 'ok', 'status': 'allowed',
                                                              'scope': 'cloud', 'id': None})
        self.assertEqual(self.ask(BETA, 'example/library', 'push')['status'], 'pending')
        for index in range(agents.MAX_PENDING - 3):
            self.assertTrue(self.ask(ALPHA, 'example/r%d' % index)['ok'])
        self.assertFalse(self.ask(ALPHA, 'example/one-too-many')['ok'])

    def test_the_host_lists_requests_without_secrets_and_status_counts_them(self):
        identifier = self.ask(ALPHA)['id']
        report = service.agents.requests_report(self.book, self.store.load()[0], lambda: NOW + 5)
        self.assertEqual(report, {'requests': [{'id': identifier, 'repository': 'example/extra', 'access': 'push',
                                                'reason': 'Open a PR for the fix', 'session': 'agent-alpha',
                                                'agent': 'claude', 'created_at': NOW}]})
        self.assertNotIn(ACCESS, json.dumps(report))
        self.assertEqual(service.status(self.store, lambda: NOW)['pending_requests'], 1)
        self.assertEqual(self.book.runtime.joinpath(agents.BOOK).stat().st_mode & 0o777, 0o600)
        # A request without a decision expires after a day.
        self.assertEqual(agents.requests_report(self.book, self.store.load()[0], lambda: NOW + DAY)['requests'], [])
        self.assertEqual(self.answer({'request': 'request-status', 'id': identifier}, NOW + DAY, ALPHA)[0]['status'],
                         'expired')

    def test_only_the_asking_session_reads_its_request(self):
        identifier = self.ask(ALPHA)['id']
        self.assertEqual(self.answer({'request': 'request-status', 'id': identifier}, NOW, ALPHA)[0]['status'],
                         'pending')
        for who in (BETA, None):
            self.assertFalse(self.answer({'request': 'request-status', 'id': identifier}, NOW, who)[0]['ok'])

    def test_deny_is_final(self):
        identifier = self.ask(ALPHA)['id']
        result = self.decide(identifier, 'deny')
        self.assertEqual(result, {'ok': True, 'id': identifier, 'decision': 'deny', 'repository': 'example/extra',
                                  'access': 'push', 'status': 'denied'})
        self.assertEqual(self.answer({'request': 'request-status', 'id': identifier}, NOW, ALPHA)[0]['status'],
                         'denied')
        self.assertEqual(self.decide(identifier, 'allow-cloud')['error'], 'not_pending')
        self.assertEqual(self.decide('0' * 16, 'deny')['error'], 'unknown_request')
        self.assertFalse(self.token(ALPHA)['ok'])

    def test_access_is_per_cloud_and_there_is_no_task_decision(self):
        identifier = self.ask(ALPHA, access='read')['id']
        github, check = self.api(repository(False))
        with self.assertRaises(ValueError):
            self.decide(identifier, 'allow-task', check)
        self.assertEqual(self.decide(identifier, 'allow-cloud', check)['status'], 'allowed')
        self.assertEqual(github.seen, [('/repos/example/extra', 'Bearer ' + ACCESS)])
        # Every session, and a process outside any session, gets the same token.
        for who in (ALPHA, BETA, None):
            reply = self.token(who)
            self.assertEqual((reply['token'], reply['access']), (ACCESS, 'read'), who)
        self.assertFalse(self.token(ALPHA, path='example/extra.git/git-receive-pack')['ok'], 'read only')
        self.assertEqual(self.answer({'request': 'request-status', 'id': identifier}, NOW, ALPHA)[0]['scope'],
                         'cloud')

    def test_a_pending_read_request_becomes_a_push_request_when_push_is_asked(self):
        first = self.ask(ALPHA, access='read', reason='Read the API')
        second = self.ask(ALPHA, access='push', reason='Push the fix')
        self.assertEqual((second['id'], second['access']), (first['id'], 'push'))
        waiting = agents.requests_report(self.book, self.store.load()[0], lambda: NOW)['requests']
        self.assertEqual([(item['access'], item['reason']) for item in waiting], [('push', 'Push the fix')])
        self.assertEqual(self.ask(ALPHA, access='read')['access'], 'push', 'a read request never weakens it')

    def test_a_grant_carries_over_only_for_a_known_same_account(self):
        self.assertTrue(service.same_account({'client_id': 'a', 'login': 'octo'}, {'client_id': 'a', 'login': 'octo'}))
        self.assertFalse(service.same_account({'client_id': 'a'}, {'client_id': 'a'}), 'no login, no account')
        self.assertFalse(service.same_account({'client_id': 'a', 'login': 'octo'}, {'client_id': 'a'}))
        self.assertFalse(service.same_account({'client_id': 'a', 'login': 'octo'}, {'client_id': 'b', 'login': 'octo'}))

    def test_a_reason_that_reorders_text_is_refused(self):
        for reason in ('fix \u202etsurt', 'fix \u2066x', 'fix \u200fx', 'line\nbreak', 'one\u2028two',
                       'one\u2029two', 'lone \ud800 surrogate'):
            self.assertFalse(self.ask(ALPHA, reason=reason)['ok'], repr(reason))

    def test_allow_for_the_cloud_persists_for_every_session_and_the_same_account(self):
        self.install(installation(login='octo-cat'))
        identifier = self.ask(ALPHA)['id']
        _, check = self.api(repository(True))
        self.assertEqual(self.decide(identifier, 'allow-cloud', check)['status'], 'allowed')
        for who in (ALPHA, BETA, None):
            self.assertEqual(self.token(who)['access'], 'push', who)
        restarted = service.Store(self.store.persistent, self.root / 'run/after-restart')
        self.assertEqual(restarted.load()[0]['cloud_grants'], [{'repository': 'example/extra', 'access': 'push'}])
        self.assertIn({'repository': 'example/extra', 'target': None, 'access': 'push'},
                      service.status(self.store)['repositories'])
        self.assertEqual(self.ask(BETA)['status'], 'allowed')
        self.install(installation(chain=chain(access='ghu_synthetic-new'), login='octo-cat'))
        self.assertTrue(self.token(None)['ok'], 'a new chain of the same account keeps the grant')
        self.install(installation(login='other-account'))
        self.assertFalse(self.token(None)['ok'], 'another account loses it')

    def test_github_must_reach_the_repository_before_an_allow(self):
        identifier = self.ask(ALPHA)['id']
        _, check = self.api((404, '{"message":"Not Found"}'), repository(False), (401, '{}'), (403, '{}'),
                            (500, 'oops'), (200, 'not json'))
        for code in ('not_installed', 'no_push', 'token_invalid', 'forbidden', 'unreachable', 'unreachable'):
            result = self.decide(identifier, 'allow-cloud', check)
            self.assertEqual((result['ok'], result['error']), (False, code))
            self.assertTrue(result['message'])
        self.assertEqual(self.answer({'request': 'request-status', 'id': identifier}, NOW, ALPHA)[0]['status'],
                         'pending')
        expiry = chain()['access_expires_at']
        self.assertEqual(self.decide(identifier, 'allow-cloud', now=expiry)['error'], 'token_expired')
        _, post = self.fake_github((200, '{"error":"bad_refresh_token"}'))
        service.refresh_once(self.store, lambda: expiry, post)
        self.assertEqual(self.decide(identifier, 'allow-cloud')['error'], 'no_chain')
        self.assertEqual(self.ask(ALPHA)['state'], 'revoked')

    def test_cloud_grants_are_bounded(self):
        with self.store.lock():
            self.store.save(dict(self.stored(), cloud_grants=[{'repository': 'example/r%d' % n, 'access': 'read'}
                                                              for n in range(agents.MAX_CLOUD_GRANTS)]))
        identifier = self.ask(ALPHA)['id']
        check = mock.Mock(side_effect=AssertionError('no GitHub call over the limit'))
        self.assertEqual(self.decide(identifier, 'allow-cloud', check)['error'], 'too_many_grants')
        self.assertEqual(self.answer({'request': 'request-status', 'id': identifier}, NOW, ALPHA)[0]['status'],
                         'pending')

    def test_a_grant_never_lands_on_a_chain_that_github_did_not_check(self):
        identifier = self.ask(ALPHA)['id']

        def check(*args):
            # Another account's chain is stored while GitHub is asked. (A real install waits
            # for the request book, which this decision holds, so the chain is swapped here.)
            with self.store.lock():
                self.store.save(dict(self.stored(), chain=chain(access='ghu_synthetic-other'), login='someone-else'))
        self.assertEqual(self.decide(identifier, 'allow-cloud', check)['error'], 'chain_changed')
        self.assertNotIn('cloud_grants', self.stored())
        # The new chain acts for another account, so the request does not carry over to it.
        self.assertEqual(self.answer({'request': 'request-status', 'id': identifier}, NOW, ALPHA)[0]['status'],
                         'expired')

    def test_a_request_belongs_to_the_account_it_was_made_under(self):
        self.install(installation(login='octo-cat'))
        old = self.ask(ALPHA)['id']
        # Another account's chain; its own requests are not cleared away after the install.
        self.install(installation(chain=chain(access='ghu_synthetic-other'), login='someone-else'))
        fresh = self.ask(BETA, repository='example/other')['id']
        waiting = agents.requests_report(self.book, self.store.load()[0], lambda: NOW)['requests']
        self.assertEqual([item['id'] for item in waiting], [fresh])
        self.assertEqual(self.answer({'request': 'request-status', 'id': old}, NOW, ALPHA)[0]['status'], 'expired')
        self.assertEqual(self.decide(old, 'allow-cloud')['error'], 'not_pending')
        self.assertEqual(self.decide(fresh, 'allow-cloud')['status'], 'allowed')

    def test_a_record_the_host_could_not_parse_is_left_out_of_the_report(self):
        self.ask(ALPHA)
        with self.book.edit() as data:
            data['requests'][0]['agent'] = 'cl\ud800aude'
        self.assertEqual(agents.requests_report(self.book, self.store.load()[0], lambda: NOW)['requests'], [])

    def test_clear_removes_requests_and_grants(self):
        self.decide(self.ask(ALPHA)['id'], 'allow-cloud')
        self.ask(BETA, repository='example/other')
        service.clear(self.store, retire=lambda: None)
        self.assertEqual(self.book.read(), {'requests': []})
        self.assertEqual(service.status(self.store)['pending_requests'], 0)


class ToolTests(unittest.TestCase):
    def test_the_tool_waits_for_the_decision(self):
        replies = iter([{'ok': True, 'status': 'pending', 'id': 'abc'}, {'ok': True, 'status': 'pending'},
                        {'ok': True, 'status': 'allowed', 'scope': 'cloud'}])
        asked = []
        success, text = agents.call_tool({'repository': 'example/extra', 'access': 'push', 'reason': 'PR'},
                                         lambda request: asked.append(request) or next(replies), sleep=lambda _: None)
        self.assertTrue(success)
        self.assertIn('this cloud', text)
        self.assertEqual(asked[0], {'request': 'request', 'repository': 'example/extra', 'access': 'push',
                                    'reason': 'PR'})
        self.assertEqual(asked[1], {'request': 'request-status', 'id': 'abc'})

    def test_the_tool_reports_denial_waiting_and_no_connection(self):
        arguments = {'repository': 'example/extra', 'access': 'read', 'reason': 'Read the docs'}
        denied = iter([{'ok': True, 'status': 'pending', 'id': 'abc'}, {'ok': True, 'status': 'denied'}])
        self.assertIn('denied', agents.call_tool(arguments, lambda _: next(denied), sleep=lambda _: None)[1])
        clock = iter(range(0, 10000, 300))
        success, text = agents.call_tool(arguments, lambda _: {'ok': True, 'status': 'pending', 'id': 'abc'},
                                         sleep=lambda _: None, clock=lambda: next(clock))
        self.assertTrue(success)
        self.assertIn('Still waiting for the person', text)
        self.assertIn('abc', text)
        self.assertFalse(agents.call_tool(arguments, lambda _: None)[0])
        refused = {'ok': False, 'message': 'Only an agent session on this worker can ask for GitHub access.'}
        self.assertEqual(agents.call_tool(arguments, lambda _: refused), (False, refused['message']))

    def test_mcp_framing_matches_the_stop_tool(self):
        replies = []
        lines = [json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize',
                             'params': {'protocolVersion': '2025-06-18'}}),
                 json.dumps({'jsonrpc': '2.0', 'method': 'notifications/initialized'}), 'not json',
                 json.dumps({'jsonrpc': '2.0', 'id': 2, 'method': 'tools/list'}),
                 json.dumps({'jsonrpc': '2.0', 'id': 3, 'method': 'tools/call', 'params': {
                     'name': 'github_access', 'arguments': {'repository': 'example/extra'}}}),
                 json.dumps({'jsonrpc': '2.0', 'id': 4, 'method': 'unknown'})]
        output = io.StringIO()
        agents.serve_mcp(io.StringIO('\n'.join(lines) + '\n'), output, call=lambda arguments: (True, 'done'))
        replies = [json.loads(line) for line in output.getvalue().splitlines()]
        self.assertEqual([reply['id'] for reply in replies], [1, 2, 3, 4])
        self.assertEqual(replies[0]['result']['protocolVersion'], '2025-06-18')
        tool = replies[1]['result']['tools'][0]
        self.assertEqual((tool['name'], tool['inputSchema']['required']), ('github_access', ['repository', 'access',
                                                                                              'reason']))
        self.assertEqual(replies[2]['result'], {'content': [{'type': 'text', 'text': 'done'}], 'isError': False})
        self.assertEqual(replies[3]['error']['code'], -32601)

    def test_agents_may_run_only_the_tool_without_root(self):
        for argv, status in [(['requests'], 1), (['decide', 'abc', 'allow-cloud'], 1), (['decide', 'abc', 'maybe'], 2),
                             (['decide', 'abc', 'allow-task'], 2), (['decide', 'abc'], 2)]:
            with mock.patch.object(service.os, 'geteuid', return_value=1000), \
                    mock.patch('sys.stderr', io.StringIO()):
                self.assertEqual(service.main(argv), status, argv)


class EndToEndTests(ServiceTestCase):
    """The tool, the socket and a decision by the host, with a fake session mapping."""

    def setUp(self):
        super().setUp()
        self.install()
        path = self.root / 'run/worker/github.sock'
        server = agents.listen(path)
        self.addCleanup(server.close)
        threading.Thread(target=agents.accept_forever, args=(server, self.store, self.book), daemon=True).start()
        for target, name, value in [(agents, 'ALLOWED_UIDS', (os.getuid(),)), (auth, 'SERVICE_SOCKET', path),
                                    (agents, 'identify_session', lambda pid: ALPHA)]:
            patcher = mock.patch.object(target, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def test_an_agent_asks_the_person_allows_and_git_gets_the_token(self):
        def host():
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                waiting = agents.requests_report(self.book, self.store.load()[0])['requests']
                if waiting:
                    decided.append(agents.decide(self.store, self.book, waiting[0]['id'], 'allow-cloud',
                                                 now=lambda: NOW, check=lambda *args: None))
                    return
                time.sleep(.02)
        decided = []
        thread = threading.Thread(target=host)
        thread.start()
        request = 'protocol=https\nhost=github.com\npath=example/extra.git\n'
        with mock.patch.object(agents.time, 'time', return_value=NOW):
            self.assertEqual(auth.service_credential(request), '')
            success, text = agents.call_tool({'repository': 'example/extra', 'access': 'push', 'reason': 'Push the fix'},
                                             poll=.02)
            thread.join()
            self.assertTrue(success, text)
            self.assertIn('this cloud', text)
            self.assertEqual(decided[0]['status'], 'allowed')
            self.assertEqual(auth.service_credential(request), 'username=x-access-token\npassword=' + ACCESS + '\n\n')
        log = [json.loads(line) for line in (self.store.runtime / agents.LOG).read_text().splitlines()]
        self.assertIn('request', [record['request'] for record in log])
        self.assertNotIn(ACCESS, (self.store.runtime / agents.LOG).read_text())


if __name__ == '__main__':
    unittest.main()
