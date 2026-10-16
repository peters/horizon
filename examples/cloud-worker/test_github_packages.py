"""The package read access of the worker's GitHub access: root keeps and renews its chain, the
REST policy names the package reads, and the API broker adds its token only to those reads for
an owner of the cloud's own repositories, and filters their replies. Synthetic tokens only."""
import itertools
import json
import os
import unittest
from unittest import mock

from test_github_api_broker import APIHandler, BrokerTestCase
from test_github_git_proxy import bind_static
from test_horizon_worker_github import ACCESS, ServiceTestCase, chain, service

broker = service.broker
rest = broker.rest
packages = service.packages
TOKEN = 'gho_synthetic-packages'
OTHER = 'gho_synthetic-packages-two'
CLIENT = 'Ov23synthetic'
VERSIONS = '/orgs/example/packages/nuget/Operator.Push/versions'
READ = ['read:packages']


def request(token=TOKEN, repositories=('example/project', 'Example/Library')):
    """Horizon's request: the OAuth app's client ID, the cloud's repositories and a chain, or a
    null chain."""
    if token is None:
        return json.dumps({'version': 1, 'chain': None})
    return json.dumps({'version': 1, 'client_id': CLIENT, 'repositories': list(repositories),
                       'chain': chain(access=token)})


def install(store, source=None, scopes=READ):
    return packages.install(store, source or request(), check=lambda token: scopes)


class RouteTests(unittest.TestCase):
    def test_package_reads_of_nuget_and_npm(self):
        for method, target in [('GET', VERSIONS), ('GET', VERSIONS + '/123?per_page=1'),
                               ('GET', '/orgs/Example/packages/nuget/Operator.Api.Client'),
                               ('GET', '/users/someone/packages/npm/tool/versions?page=2'),
                               ('GET', '/orgs/example/packages?package_type=nuget&per_page=100'),
                               ('GET', '/users/example/packages?package_type=npm'),
                               ('GET', '/orgs/example/packages/nuget/Operator.Push?package_type=nuget')]:
            route = rest.route(method, 'api.github.com', target)
            self.assertEqual((route.kind, route.access), ('package', 'read'), target)
        self.assertEqual(rest.route('GET', 'API.GITHUB.COM', '/orgs/Example/packages/npm/x').owner, 'Example')

    def test_changes_other_types_and_odd_paths_are_refused(self):
        for method, target, status in [
                ('DELETE', VERSIONS + '/123', 403), ('PUT', VERSIONS, 403), ('PATCH', VERSIONS, 403),
                ('HEAD', VERSIONS, 403),
                ('POST', '/orgs/example/packages/nuget/Operator.Push/restore', 403),
                ('POST', '/orgs/example/packages/nuget/Operator.Push/versions/1/restore', 403),
                ('GET', '/orgs/example/packages/container/mongo-enterprise/versions', 403),
                ('GET', '/orgs/example/packages/maven/x', 403), ('GET', '/orgs/example/packages/NuGet/x', 403),
                ('GET', '/orgs/example/packages/n%75get/x', 403),
                ('GET', '/orgs/example/packages?package_type=container', 403),
                ('GET', '/orgs/example/packages?package_type=NUGET', 403),
                ('GET', '/orgs/example/packages', 403),
                ('GET', '/orgs/example/packages?package_type=nuget&package_type=container', 403),
                ('GET', '/orgs/example/packages/nuget/x?package_type=container', 403),
                ('GET', '/orgs/example/packages?package_type=nuget;package_type=container', 400),
                ('GET', '/user/packages/nuget/x', 403), ('GET', '/orgs/example/packages/nuget/x/versions/latest', 403),
                ('GET', '/orgs/example/packages/nuget/x%40y', 403), ('GET', '/orgs/-x/packages/nuget/y', 403),
                ('GET', '/orgs/example/packages/nuget/../container/x', 400),
                ('GET', '/orgs/example/packages/nuget/%2e%2e/container', 400),
                ('GET', '/orgs/example/packages/nuget/a%2Fb', 400),
                ('GET', '/orgs/example/packages/nuget/a%5cb', 400),
                ('GET', '/orgs/example//packages/nuget/x', 400),
                ('GET', '/orgs/example/packages/nuget/x\\y', 400),
                ('GET', '/orgs/example/packages/nuget/%252e%252e/container', 403),
                ('GET', '/orgs/example/packages/%6Euget/x', 403),
                ('GET', '/orgs/example/packages?package_type=nuget&q=x', 400),
                ('GET', '/orgs/example/packages?package_type=nuget&visibility[]=private', 400),
                ('GET', '/orgs/example/packages?package_type=nuget&package%255Ftype=container', 400),
                ('GET', '/orgs/example/packages/docker/x', 403), ('GET', '/orgs/example/packages/rubygems/x', 403)]:
            with self.assertRaises(rest.Refused, msg=(method, target)) as refused:
                rest.route(method, 'api.github.com', target)
            self.assertEqual(refused.exception.status, status, target)


class StorageTests(ServiceTestCase):
    def test_root_keeps_the_chain_in_its_own_private_store_and_removes_it(self):
        self.assertEqual(packages.token_for(self.store, 'example'), (None, packages.MISSING))
        held = install(self.store)['packages']
        self.assertEqual((held['state'], held['persistent']), ('ok', True))
        self.assertEqual(held['repositories'], ['example/project', 'Example/Library'])
        self.assertEqual(packages.token_for(self.store, 'EXAMPLE'), (TOKEN, None))
        chains = packages.store_for(self.store)
        self.assertIs(chains, packages.store_for(self.store))
        self.assertEqual(chains.persistent.name, 'github-packages')
        state = chains.persistent / service.STATE
        self.assertEqual(os.stat(state).st_mode & 0o777, 0o600)
        self.assertEqual(os.stat(chains.persistent).st_mode & 0o777, 0o700)
        report = service.status(self.store)
        self.assertEqual(report['packages']['state'], 'ok')
        self.assertNotIn(TOKEN, json.dumps(report))
        self.assertNotIn(chain()['refresh_token'], json.dumps(report))
        install(self.store, request(OTHER))
        self.assertEqual(packages.token_for(self.store, 'example'), (OTHER, None))
        self.assertEqual(install(self.store, request(None)), {'packages': None})
        self.assertEqual(packages.token_for(self.store, 'example'), (None, packages.MISSING))
        self.assertFalse(state.exists())
        self.assertIsNone(service.status(self.store)['packages'])

    def test_malformed_requests_and_other_scopes_are_refused_without_echoing_a_token(self):
        good = json.loads(request())
        cases = [dict(good, version=2), dict(good, version=True), dict(good, extra=1),
                 dict(good, client_secret='synthetic-secret'),
                 {key: value for key, value in good.items() if key != 'client_id'},
                 {key: value for key, value in good.items() if key != 'repositories'},
                 dict(good, client_id='bad id'), dict(good, repositories=[]),
                 dict(good, repositories=['example/a'] * 2), dict(good, repositories=['Example/a', 'example/A']),
                 dict(good, repositories=['example']), dict(good, repositories=['-x/y']),
                 dict(good, repositories=['a/b'] * 51), dict(good, repositories='example/project'),
                 dict(good, chain=dict(good['chain'], access_token=TOKEN + '\n')),
                 dict(good, chain=dict(good['chain'], refresh_expires_at=1)),
                 {'version': 1, 'chain': None, 'client_id': CLIENT}, [], {'version': 1}]
        for value in cases:
            with self.assertRaises(ValueError, msg=value) as refused:
                install(self.store, json.dumps(value))
            self.assertNotIn(TOKEN, str(refused.exception))
        for scopes in (['write:packages'], ['read:packages', 'repo'], [], None, ['READ:PACKAGES']):
            with self.assertRaises(ValueError, msg=scopes):
                install(self.store, scopes=scopes)
        self.assertEqual(packages.token_for(self.store, 'example'), (None, packages.MISSING))

    def test_the_scope_is_asked_from_github_with_the_token(self):
        seen = []
        with mock.patch.object(packages.common, 'opener') as opener:
            response = opener.return_value.open.return_value.__enter__.return_value
            response.headers = {'X-OAuth-Scopes': 'read:packages'}
            opener.return_value.open.side_effect = lambda request, timeout: (
                seen.append(request), opener.return_value.open.return_value)[1]
            self.assertEqual(packages.scopes_of(TOKEN, {service.TEST_URL: 'http://127.0.0.1:9'}), READ)
        self.assertEqual(seen[0].get_header('Authorization'), 'token ' + TOKEN)
        with mock.patch.object(packages.common, 'opener') as opener:
            opener.return_value.open.side_effect = OSError('unreachable')
            with self.assertRaises(ValueError) as refused:
                packages.scopes_of(TOKEN, {service.TEST_URL: 'http://127.0.0.1:9'})
        self.assertNotIn(TOKEN, str(refused.exception))

    def test_the_root_service_renews_the_chain_with_the_client_id_and_keeps_only_read(self):
        install(self.store)
        chains = packages.store_for(self.store)
        forms, scope = [], ['read:packages']

        def post(form):
            forms.append(form)
            return {'access_token': OTHER, 'refresh_token': 'ghr_synthetic-packages-two', 'expires_in': 28800,
                    'refresh_token_expires_in': 15897600, 'token_type': 'bearer', 'scope': scope[0]}
        due = chain(access=TOKEN)['access_expires_at'] - 60
        self.assertEqual(service.refresh_once(chains, lambda: due, post, packages.SCOPE), 'refreshed')
        self.assertEqual(forms[-1], {'client_id': CLIENT, 'grant_type': 'refresh_token',
                                     'refresh_token': chain()['refresh_token']})
        self.assertEqual(packages.token_for(self.store, 'example', lambda: due), (OTHER, None))
        # A renewal that brings another scope removes the chain: no token that can write stays.
        install(self.store)
        scope[0] = 'read:packages,write:packages'
        self.assertEqual(service.refresh_once(chains, lambda: due, post, packages.SCOPE), 'revoked')
        self.assertEqual(packages.token_for(self.store, 'example'), (None, packages.MISSING))
        self.assertNotIn(OTHER, json.dumps(chains.copies()))
        # Also when the volume copy cannot be removed: the clear mark hides it at once.
        install(self.store)
        with mock.patch.object(packages.common, 'remove_durably', side_effect=OSError('read-only volume')), \
                self.assertRaises(ValueError):
            service.refresh_once(chains, lambda: due, post, packages.SCOPE)
        self.assertEqual(packages.token_for(self.store, 'example'), (None, packages.MISSING))
        install(self.store)
        self.assertEqual(packages.token_for(self.store, 'example', lambda: due + 3600 * 8),
                         (None, packages.EXPIRED))
        self.assertEqual(service.refresh_once(chains, lambda: due, lambda form: {'error': 'bad_refresh_token'},
                                              packages.SCOPE), 'revoked')
        self.assertEqual(packages.token_for(self.store, 'example'), (None, packages.REVOKED))
        self.assertEqual(service.status(self.store)['packages']['last_error'], 'bad_refresh_token')
        logged = []
        service.refresh_step(chains, 0, now=lambda: due, log=lambda *line, **_: logged.append(line[0]),
                             label='Package read access', scope=packages.SCOPE)
        self.assertEqual(logged, ['Package read access revoked'])

    def test_only_the_owners_of_the_cloud_repositories_open_never_a_grant(self):
        self.install()
        install(self.store, request(repositories=['acme/api']))
        self.assertEqual(packages.token_for(self.store, 'ACME'), (TOKEN, None))
        # The chain's grants and an allowed cloud grant open nothing; neither does the static binding.
        state = self.stored()
        self.store.save(dict(state, cloud_grants=[{'repository': 'otherorg/repo', 'access': 'read'}],
                             serial=state['serial'] + 1))
        bind_static(self.root / 'credentials/github.json', 'bound/tool')
        for owner in ('example', 'otherorg', 'bound'):
            token, reason = packages.token_for(self.store, owner)
            self.assertIsNone(token, owner)
            self.assertIn('owns none of this cloud', reason)

    def test_a_task_grant_opens_no_owner(self):
        self.install()
        install(self.store, request(repositories=['acme/api']))
        with mock.patch.object(service.agents.tasks, 'access', return_value='push'):
            self.assertIsNone(packages.token_for(self.store, 'tasked')[0])

    def test_an_unreadable_request_on_the_command_line_stops_the_earlier_chain(self):
        for source in (b'\xff\xfe', b' ' * (packages.MAX_SOURCE + 1), b'{"version":1}'):
            with self.subTest(source=source[:8]):
                install(self.store)
                stdin = mock.Mock()
                stdin.buffer.read.return_value = source
                with mock.patch.object(service, 'Store', return_value=self.store), \
                        mock.patch.object(service.sys, 'stdin', stdin), \
                        mock.patch.object(service.os, 'geteuid', side_effect=itertools.chain([0], itertools.repeat(os.geteuid()))), \
                        mock.patch.object(service.common, 'AGENT_ISOLATION', self.root / 'run'), \
                        self.assertRaises((ValueError, UnicodeDecodeError)):
                    service.main(['packages'])
                self.assertEqual(packages.token_for(self.store, 'example'), (None, packages.MISSING))

    def test_a_renewal_waits_for_an_install_and_never_loses_its_chain(self):
        install(self.store)
        chains = packages.store_for(self.store)
        order = []

        def check(token):
            # The renewal tries to run while the install holds the lock; it must wait.
            order.append('check')
            self.assertFalse(chains_lock_free())
            return READ

        def chains_lock_free():
            import fcntl
            fd = os.open(chains.runtime / 'state.lock', os.O_RDWR)
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                return True
            except BlockingIOError:
                return False
            finally:
                os.close(fd)
        packages.install(self.store, request(OTHER), check=check)
        self.assertEqual(order, ['check'])
        self.assertEqual(packages.token_for(self.store, 'example'), (OTHER, None))

    def test_disconnecting_removes_the_access(self):
        self.install()
        install(self.store)
        service.clear(self.store, retire=lambda: None)
        self.assertEqual(packages.token_for(self.store, 'example'), (None, packages.MISSING))
        self.assertFalse((packages.store_for(self.store).persistent / service.STATE).exists())

    def test_a_removal_or_replacement_that_fails_leaves_nothing_served(self):
        install(self.store)
        chains = packages.store_for(self.store)
        with mock.patch.object(packages.common, 'remove_durably', side_effect=OSError('stuck')):
            with self.assertRaises(ValueError):
                packages.remove(self.store)
        self.assertEqual(packages.token_for(self.store, 'example'), (None, packages.MISSING))
        install(self.store)
        self.assertEqual(packages.token_for(self.store, 'example'), (TOKEN, None))
        with mock.patch.object(chains, 'write', side_effect=ValueError('no storage')):
            with self.assertRaises(ValueError):
                install(self.store, request(OTHER))
        self.assertEqual(packages.token_for(self.store, 'example'), (None, packages.MISSING))
        # A scope lookup that breaks with any error, such as a malformed status line, stops it too.
        install(self.store)
        for error in (packages.http.client.BadStatusLine('x'), packages.http.client.LineTooLong('header'),
                      RuntimeError('unexpected')):
            with self.subTest(error=type(error).__name__):
                install(self.store)

                def broken(token, error=error):
                    raise error
                with self.assertRaises(Exception):
                    packages.install(self.store, request(OTHER), check=broken)
                self.assertEqual(packages.token_for(self.store, 'example'), (None, packages.MISSING))
        with mock.patch.object(packages.common, 'opener') as opener:
            opener.return_value.open.side_effect = packages.http.client.BadStatusLine('x')
            with self.assertRaises(ValueError):
                packages.scopes_of(TOKEN, {service.TEST_URL: 'http://127.0.0.1:9'})
        # A replacement refused before it is stored, for its scope or its request, stops the earlier chain too.
        for source, scopes in ((request(OTHER), ['read:packages', 'write:packages']), ('{"version":1}', READ)):
            install(self.store)
            with self.assertRaises(ValueError):
                install(self.store, source, scopes=scopes)
            self.assertEqual(packages.token_for(self.store, 'example'), (None, packages.MISSING))

    def test_repository_objects_of_other_repositories_are_removed_from_replies(self):
        own = {'example/project'}
        listing = [{'name': 'a', 'repository': {'full_name': 'Example/Project', 'private': True}},
                   {'name': 'b', 'repository': {'full_name': 'example/secret', 'description': 'private plans'}},
                   {'name': 'c', 'repository': 'odd'}, {'name': 'd'}]
        kept = json.loads(packages.filtered(json.dumps(listing).encode(), own))
        self.assertEqual([item.get('repository') for item in kept],
                         [{'full_name': 'Example/Project', 'private': True}, None, None, None])
        self.assertNotIn('private plans', json.dumps(kept))
        single = json.loads(packages.filtered(json.dumps(listing[1]).encode(), own))
        self.assertIsNone(single['repository'])
        addresses = {'name': 'e', 'html_url': 'https://github.com/example/secret/pkgs/nuget/e',
                     'url': 'https://api.github.com/repos/Example/Secret/packages', 'own': 'https://github.com/example/project',
                     'org': 'https://github.com/orgs/example/packages/nuget/package/e',
                     'versions': [{'package_html_url': 'https://github.com/example/secret.git'}],
                     'metadata': {'nested': {'link': 'see https://github.com/other/repo/issues'}}}
        cleaned = json.loads(packages.filtered(json.dumps(addresses).encode(), own))
        self.assertEqual(cleaned, {'name': 'e', 'html_url': None, 'url': None,
                                   'own': 'https://github.com/example/project',
                                   'org': 'https://github.com/orgs/example/packages/nuget/package/e',
                                   'versions': [{'package_html_url': None}], 'metadata': {'nested': {'link': None}}})
        # A reply whose root is a string is examined too.
        self.assertEqual(packages.filtered(b'"https://github.com/example/secret"', own), b'null')
        self.assertEqual(packages.filtered(b'"https://github.com/example/project"', own),
                         b'"https://github.com/example/project"')
        with self.assertRaises(packages.Unchecked):
            packages.filtered(b'<html>', own)
        with self.assertRaises(packages.Unchecked):
            packages.filtered(json.dumps({'note': TOKEN}).encode(), own, (TOKEN,))


class BrokerTests(BrokerTestCase):
    def answer_with(self, status, data, fields=()):
        """Makes the fake GitHub answer package paths with `status`, `data` and `fields`, and
        keeps the request fields of each in `self.heads`."""
        answer = APIHandler.answer
        self.heads = []

        def scripted(handler):
            if '/packages' not in handler.path:
                return answer(handler)
            self.heads.append({name.lower(): value for name, value in handler.headers.items()})
            handler.server.seen.append((handler.command, handler.headers.get('Host'), handler.path,
                                        handler.headers.get('Authorization'), b''))
            handler.send_response(status)
            for name, value in fields:
                handler.send_header(name, value)
            handler.send_header('Content-Length', str(len(data)))
            handler.end_headers()
            handler.wfile.write(data)
        patch = mock.patch.object(APIHandler, 'answer', scripted)
        patch.start()
        self.addCleanup(patch.stop)

    def test_without_package_read_access_a_package_read_never_reaches_github(self):
        status, _, payload = self.send('GET', VERSIONS)
        self.assertEqual(status, 403)
        self.assertIn(b'no package read access', payload)
        self.assertIn(b'in Horizon', payload)
        self.assertEqual(self.api.seen, [])

    def test_a_package_read_of_an_own_owner_gets_only_the_package_token(self):
        install(self.store)
        status, _, payload = self.send('GET', VERSIONS + '?per_page=5', fields=['Accept-Encoding: gzip'])
        self.assertEqual((status, json.loads(payload)), (200, {'path': VERSIONS + '?per_page=5'}))
        self.assertEqual(self.api.seen[-1][:4], ('GET', 'api.github.com', VERSIONS + '?per_page=5', 'token ' + TOKEN))
        self.assertEqual(self.send('GET', '/orgs/EXAMPLE/packages?package_type=nuget')[0], 200)
        self.assertEqual(self.api.seen[-1][3], 'token ' + TOKEN)
        # A repository's own paths keep the chain's token.
        self.send('GET', '/repos/example/project')
        self.assertEqual(self.api.seen[-1][3], 'token ' + ACCESS)
        records = self.records(3)
        self.assertEqual((records[0]['request'], records[0]['granted'], records[0]['outcome']),
                         ('package', True, 'relayed'))
        self.assertNotIn(TOKEN, json.dumps(records))

    def test_other_owners_changes_bodies_and_other_types_are_refused_before_github(self):
        install(self.store)
        for method, target, status, reason, fields in [
                ('GET', '/orgs/elsewhere/packages/nuget/x/versions', 403, b'owns none of this cloud', ()),
                ('DELETE', VERSIONS + '/1', 403, b'never a change of a package', ()),
                ('POST', VERSIONS, 403, b'never a change of a package', ()),
                ('GET', '/orgs/example/packages/container/image/versions', 403, b'only NuGet and npm', ()),
                ('GET', VERSIONS + '?q=x', 400, b'only the query parameters', ())]:
            status_seen, _, payload = self.send(method, target, fields=fields)
            self.assertEqual(status_seen, status, target)
            self.assertIn(reason, payload)
        status, _, payload = self.send('GET', VERSIONS, body=b'{}')
        self.assertEqual(status, 400)
        self.assertIn(b'has no body', payload)
        self.assertEqual(self.api.seen, [])

    def test_a_redirect_goes_back_without_the_token_its_body_or_other_fields(self):
        install(self.store)
        location = 'https://pkg-containers.githubusercontent.com/blob?sig=synthetic'
        self.answer_with(302, b'{"repository":{"full_name":"example/secret"}}',
                         [('Location', location), ('ETag', '"x"'), ('Content-Type', 'application/json')])
        status, fields, payload = self.send('GET', VERSIONS)
        self.assertEqual((status, fields['location'], payload), (302, location, b''))
        self.assertNotIn('etag', fields)
        self.assertNotIn('content-type', fields)
        self.assertEqual(len(self.api.seen), 1)
        self.assertEqual(self.api.seen[0][1], 'api.github.com')
        status, _, _ = self.send('GET', '/blob?sig=synthetic', host='pkg-containers.githubusercontent.com')
        self.assertEqual(status, 200)
        self.assertIsNone(self.api.seen[-1][3])

    def test_replies_lose_other_repositories_and_unreadable_replies_are_refused(self):
        install(self.store)
        listing = [{'name': 'a', 'repository': {'full_name': 'example/project'}},
                   {'name': 'b', 'repository': {'full_name': 'example/secret'}}]
        self.answer_with(200, json.dumps(listing).encode(), [('Content-Type', 'application/json')])
        status, _, payload = self.send('GET', '/orgs/example/packages?package_type=nuget')
        self.assertEqual(status, 200)
        self.assertEqual([item['repository'] for item in json.loads(payload)],
                         [{'full_name': 'example/project'}, None])
        self.assertNotIn(b'example/secret', payload)

    def test_a_reply_that_is_not_json_or_echoes_the_token_is_refused(self):
        install(self.store)
        for data, reason in [(b'<html>oops</html>', b'not JSON'),
                             (json.dumps({'echo': 'token ' + TOKEN}).encode(), b'held the package read access')]:
            with self.subTest(data=data):
                self.answer_with(200, data)
                status, _, payload = self.send('GET', VERSIONS)
                self.assertEqual(status, 502)
                self.assertIn(reason, payload)
                self.assertNotIn(TOKEN.encode(), payload)

    def test_ranges_and_conditions_never_reach_github_and_partial_replies_are_refused(self):
        install(self.store)
        self.answer_with(200, b'[{"name":"a"}]', [('Content-Type', 'application/json'), ('Accept-Ranges', 'bytes'),
                                                   ('ETag', '"unfiltered"'), ('Last-Modified', 'Fri, 16 Oct 2026 07:00:00 GMT')])
        status, fields, _ = self.send('GET', VERSIONS, fields=['Range: bytes=10-40', 'If-Range: "x"',
                                                               'If-None-Match: "x"', 'Accept-Encoding: gzip',
                                                               'Accept: application/vnd.github+json',
                                                               'X-GitHub-Api-Version: 2099-01-01'])
        self.assertEqual(status, 200)
        # Validators and ranges describe the unfiltered reply, so they do not go on.
        for name in ('accept-ranges', 'etag', 'last-modified'):
            self.assertNotIn(name, fields, name)
        # The API version is pinned to the one whose fields the filter knows.
        self.assertEqual(self.heads[-1]['x-github-api-version'], packages.API_VERSION)
        for name in ('range', 'if-range', 'if-none-match', 'if-modified-since', 'accept-encoding'):
            self.assertNotIn(name, self.heads[-1], name)
        self.assertEqual(self.heads[-1]['accept'], 'application/vnd.github+json')
        for status_sent, extra in [(206, []), (200, [('Content-Range', 'bytes 0-9/100')])]:
            with self.subTest(status=status_sent):
                self.answer_with(status_sent, b'{"name":"slice"}', extra)
                status, fields, payload = self.send('GET', VERSIONS)
                self.assertEqual(status, 502)
                self.assertIn(b'part of a package reply', payload)
                self.assertNotIn('content-range', fields)

    def test_head_is_refused_on_package_routes(self):
        install(self.store)
        status, _, payload = self.send('HEAD', VERSIONS)
        self.assertEqual(status, 403)
        self.assertEqual(self.api.seen, [])

    def test_a_token_that_github_refuses_asks_for_new_access(self):
        install(self.store)
        self.answer_with(401, b'{"message":"Bad credentials"}')
        status, _, payload = self.send('GET', VERSIONS)
        self.assertEqual(status, 403)
        self.assertIn(b'refused the package read access', payload)

    def test_a_token_replaced_while_the_request_is_read_does_not_go_out(self):
        install(self.store)
        candidate = self.broker.package_candidate('example')
        exchange = broker.Exchange(self.broker, None, None, {})
        self.assertEqual(exchange.fresh(candidate, owner='example').token, TOKEN)
        install(self.store, request(OTHER))
        self.assertIsNone(exchange.fresh(candidate, owner='example'))
        install(self.store, request(None))
        self.assertIsNone(exchange.fresh(candidate, owner='example'))


if __name__ == '__main__':
    unittest.main()
