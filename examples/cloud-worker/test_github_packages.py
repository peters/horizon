"""The package token of the worker's GitHub access: root keeps it, the REST policy names the
package reads, and the API broker adds it only to those reads for an owner of a granted
repository. Synthetic tokens only."""
import json
import os
import unittest
from unittest import mock

from test_github_api_broker import APIHandler, BrokerTestCase
from test_github_git_proxy import STATIC, bind_static
from test_horizon_worker_github import ACCESS, ServiceTestCase, service

broker = service.broker
rest = broker.rest
packages = service.packages
TOKEN = 'ghp_synthetic-packages'
OTHER = 'ghp_synthetic-packages-two'
VERSIONS = '/orgs/example/packages/nuget/Operator.Push/versions'


def request(token=TOKEN):
    return json.dumps({'version': 1, 'token': token})


class RouteTests(unittest.TestCase):
    def test_package_reads_of_nuget_and_npm(self):
        for method, target in [('GET', VERSIONS), ('HEAD', VERSIONS), ('GET', VERSIONS + '/123?per_page=1'),
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
                ('GET', '/orgs/example//packages/nuget/x', 400)]:
            with self.assertRaises(rest.Refused, msg=(method, target)) as refused:
                rest.route(method, 'api.github.com', target)
            self.assertEqual(refused.exception.status, status, target)


class StorageTests(ServiceTestCase):
    def test_root_keeps_the_token_in_private_copies_and_removes_them(self):
        self.assertIsNone(packages.load(self.store))
        self.assertEqual(packages.install(self.store, request()), {'packages': True})
        self.assertEqual(packages.load(self.store), TOKEN)
        for directory in packages.directories(self.store):
            self.assertEqual(os.stat(directory / packages.TOKEN).st_mode & 0o777, 0o600)
        self.assertTrue(service.status(self.store)['packages'])
        self.assertEqual(packages.install(self.store, request(OTHER)), {'packages': True})
        self.assertEqual(packages.load(self.store), OTHER)
        self.assertEqual(packages.install(self.store, request(None)), {'packages': False})
        self.assertIsNone(packages.load(self.store))
        for directory in packages.directories(self.store):
            self.assertFalse((directory / packages.TOKEN).exists())
        self.assertFalse(service.status(self.store)['packages'])

    def test_a_storage_that_does_not_take_a_new_token_loses_its_older_one(self):
        packages.install(self.store, request())
        write = packages.common.write_private

        def refusing(*refused):
            def attempt(directory, name, data):
                if directory in refused:
                    raise OSError('synthetic write failure')
                return write(directory, name, data)
            return mock.patch.object(packages.common, 'write_private', attempt)
        with refusing(self.store.runtime):
            self.assertEqual(packages.install(self.store, request(OTHER)), {'packages': True})
        self.assertFalse((self.store.runtime / packages.TOKEN).exists())
        self.assertEqual(packages.load(self.store), OTHER)
        with refusing(self.store.runtime, self.store.persistent), self.assertRaises(ValueError):
            packages.install(self.store, request())
        self.assertFalse((self.store.persistent / packages.TOKEN).exists())
        self.assertIsNone(packages.load(self.store))

    def test_malformed_requests_are_refused_without_echoing_the_token(self):
        for source in [json.dumps({'version': 2, 'token': TOKEN}), json.dumps({'token': TOKEN}),
                       json.dumps({'version': 1, 'token': TOKEN, 'extra': 1}),
                       json.dumps({'version': True, 'token': TOKEN}), request(TOKEN + '\n'), request(''),
                       request('x' * 2049), request(7), '[]']:
            with self.assertRaises(ValueError) as refused:
                packages.install(self.store, source)
            self.assertNotIn(TOKEN, str(refused.exception))
        self.assertIsNone(packages.load(self.store))

    def test_a_copy_that_differs_or_cannot_be_read_serves_no_token(self):
        packages.install(self.store, request())
        runtime = self.store.runtime / packages.TOKEN
        runtime.write_text(request(OTHER))
        self.assertIsNone(packages.load(self.store))
        runtime.write_text(request())
        self.assertEqual(packages.load(self.store), TOKEN)
        runtime.chmod(0o644)
        self.assertIsNone(packages.load(self.store))
        runtime.chmod(0o600)
        runtime.write_text('{')
        self.assertIsNone(packages.load(self.store))

    def test_a_revoked_chain_opens_no_owner_and_disconnecting_removes_the_token(self):
        self.install()
        packages.install(self.store, request())
        state = self.stored()
        self.store.save(dict(state, state='revoked', serial=state['serial'] + 1))
        self.assertEqual(packages.owners(self.store, lambda: None), set())
        self.assertIsNone(packages.token_for(self.store, 'example', lambda: None)[0])
        service.clear(self.store, retire=lambda: None)
        self.assertIsNone(packages.load(self.store))
        for directory in packages.directories(self.store):
            self.assertFalse((directory / packages.TOKEN).exists())

    def test_owners_come_from_the_cloud_grants_and_the_static_binding(self):
        self.assertEqual(packages.owners(self.store, lambda: None), set())
        self.install()
        static = (2, [{'repository': 'Bound-Owner/tool'}])
        self.assertEqual(packages.owners(self.store, lambda: static), {'example', 'bound-owner'})
        packages.install(self.store, request())
        self.assertEqual(packages.token_for(self.store, 'EXAMPLE', lambda: None), (TOKEN, None))
        token, reason = packages.token_for(self.store, 'elsewhere', lambda: None)
        self.assertIsNone(token)
        self.assertIn('elsewhere owns no repository', reason)
        packages.install(self.store, request(None))
        self.assertEqual(packages.token_for(self.store, 'example', lambda: None), (None, packages.MISSING))


class BrokerTests(BrokerTestCase):
    def test_without_a_package_token_a_package_read_never_reaches_github(self):
        status, _, payload = self.send('GET', VERSIONS)
        self.assertEqual(status, 403)
        self.assertIn(b'no package token', payload)
        self.assertIn(b'Cloud settings', payload)
        self.assertEqual(self.api.seen, [])

    def test_a_package_read_of_a_granted_owner_gets_only_the_package_token(self):
        packages.install(self.store, request())
        status, _, payload = self.send('GET', VERSIONS + '?per_page=5')
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

    def test_other_owners_changes_and_other_types_are_refused_before_github(self):
        packages.install(self.store, request())
        for method, target, reason in [
                ('GET', '/orgs/elsewhere/packages/nuget/x/versions', b'elsewhere owns no repository'),
                ('DELETE', VERSIONS + '/1', b'never a change of a package'),
                ('GET', '/orgs/example/packages/container/image/versions', b'only NuGet and npm')]:
            status, _, payload = self.send(method, target)
            self.assertEqual(status, 403, target)
            self.assertIn(reason, payload)
        self.assertEqual(self.api.seen, [])

    def test_the_static_binding_opens_its_owner(self):
        packages.install(self.store, request())
        self.assertEqual(self.send('GET', '/users/bound/packages/npm/tool')[0], 403)
        bind_static(self.credential, 'bound/tool')
        self.assertEqual(self.send('GET', '/users/bound/packages/npm/tool')[0], 200)
        self.assertEqual(self.api.seen[-1][3], 'token ' + TOKEN)
        self.assertNotIn('token ' + STATIC, [seen[3] for seen in self.api.seen])

    def test_a_token_that_github_refuses_asks_for_a_new_one(self):
        packages.install(self.store, request())
        answer = APIHandler.answer

        def refuse(handler):
            if '/packages/' not in handler.path:
                return answer(handler)
            handler.server.seen.append((handler.command, handler.headers.get('Host'), handler.path,
                                        handler.headers.get('Authorization'), b''))
            return handler.reply(401, b'{"message":"Bad credentials"}')
        with mock.patch.object(APIHandler, 'answer', refuse):
            status, _, payload = self.send('GET', VERSIONS)
        self.assertEqual(status, 403)
        self.assertIn(b'refused the package token', payload)

    def test_a_token_replaced_while_the_request_is_read_does_not_go_out(self):
        packages.install(self.store, request())
        candidate = self.broker.package_candidate('example')
        exchange = broker.Exchange(self.broker, None, None, {})
        self.assertEqual(exchange.fresh(candidate, owner='example').token, TOKEN)
        packages.install(self.store, request(OTHER))
        self.assertIsNone(exchange.fresh(candidate, owner='example'))
        packages.install(self.store, request(None))
        self.assertIsNone(exchange.fresh(candidate, owner='example'))


if __name__ == '__main__':
    unittest.main()
