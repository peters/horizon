"""The API broker's GraphQL policy against a small synthetic copy of GitHub's schema. Nothing
here reaches GitHub."""
import importlib.machinery
import importlib.util
from pathlib import Path
import unittest

HERE = Path(__file__).parent


def load(name, module):
    loader = importlib.machinery.SourceFileLoader(module, str(HERE / name))
    loaded = importlib.util.module_from_spec(importlib.util.spec_from_loader(loader.name, loader))
    loader.exec_module(loaded)
    return loaded


policy = load('horizon-worker-github-graphql-policy', 'worker_github_graphql_policy')
graphql = policy.graphql


def named(name):
    kind = 'SCALAR' if name in ('ID', 'String', 'Int', 'Boolean') else \
        'ENUM' if name in ('SearchType', '__TypeKind') else 'OBJECT'
    return {'kind': kind, 'name': name, 'ofType': None}


def nonnull(reference):
    return {'kind': 'NON_NULL', 'name': None, 'ofType': reference}


def listed(reference):
    return {'kind': 'LIST', 'name': None, 'ofType': reference}


def reference(text):
    """An introspection type reference from GraphQL type text, such as `[ID!]!`."""
    if text.endswith('!'):
        return nonnull(reference(text[:-1]))
    if text.startswith('['):
        return listed(reference(text[1:-1]))
    return named(text)


def field(text):
    """`name(arg: Type, ...): Type` as an introspection field."""
    head, kind = text.rsplit(':', 1)
    name, _, rest = head.partition('(')
    args = []
    for part in filter(None, (piece.strip() for piece in rest.rstrip(')').split(','))):
        arg, arg_kind = (item.strip() for item in part.split(':'))
        args.append({'name': arg, 'type': reference(arg_kind)})
    return {'name': name.strip(), 'args': args, 'type': reference(kind.strip())}


def kind(name, kind_name, fields=(), possible=(), inputs=()):
    return {'kind': kind_name, 'name': name, 'possibleTypes': [{'name': item} for item in possible] or None,
            'fields': [field(item) for item in fields] or None,
            'inputFields': [field(item) for item in inputs] or None}


def obj(name, *fields):
    return kind(name, 'OBJECT', fields)


TYPES = [
    obj('Query', 'repository(owner: String!, name: String!): Repository', 'node(id: ID!): Node',
        'nodes(ids: [ID!]!): [Node]!', 'search(query: String!, type: SearchType!, first: Int): SearchConnection!',
        'viewer: User!', 'rateLimit: RateLimit', 'organization(login: String!): Organization'),
    obj('Mutation', 'addComment(input: AddCommentInput!): AddCommentPayload',
        'createPullRequest(input: CreatePullRequestInput!): CreatePullRequestPayload',
        'createRepository(input: CreateRepositoryInput!): CreateRepositoryPayload'),
    obj('Repository', 'id: ID!', 'name: String!', 'nameWithOwner: String!', 'isPrivate: Boolean!',
        'owner: RepositoryOwner!', 'parent: Repository', 'pullRequest(number: Int!): PullRequest',
        'issues(first: Int): IssueConnection!'),
    obj('PullRequest', 'id: ID!', 'number: Int!', 'title: String!', 'repository: Repository!',
        'headRepository: Repository', 'author: Actor', 'comments(first: Int): IssueCommentConnection!'),
    obj('Issue', 'id: ID!', 'number: Int!', 'title: String!', 'repository: Repository!'),
    obj('IssueComment', 'id: ID!', 'body: String!', 'repository: Repository!'),
    obj('IssueConnection', 'nodes: [Issue]'),
    obj('IssueCommentConnection', 'nodes: [IssueComment]'),
    obj('RepositoryConnection', 'totalCount: Int!', 'nodes: [Repository]'),
    obj('Topic', 'id: ID!', 'name: String!', 'repositories(first: Int): RepositoryConnection!'),
    obj('User', 'id: ID!', 'login: String!', 'name: String', 'repository(name: String!): Repository',
        'repositories(first: Int): RepositoryConnection!'),
    obj('Organization', 'id: ID!', 'login: String!', 'repository(name: String!): Repository',
        'repositories(first: Int): RepositoryConnection!'),
    obj('Bot', 'id: ID!', 'login: String!'),
    obj('MarketplaceListing', 'id: ID!', 'name: String!'),
    obj('ProjectV2', 'id: ID!', 'title: String!', 'readme: String'),
    obj('SearchConnection', 'issueCount: Int!', 'nodes: [SearchResultItem]'),
    obj('RateLimit', 'remaining: Int!'),
    obj('AddCommentPayload', 'subject: Node', 'clientMutationId: String'),
    obj('CreatePullRequestPayload', 'pullRequest: PullRequest'),
    obj('CreateRepositoryPayload', 'repository: Repository'),
    kind('Node', 'INTERFACE', ['id: ID!'],
         ['Repository', 'PullRequest', 'Issue', 'IssueComment', 'User', 'Organization', 'Bot', 'Topic',
          'MarketplaceListing', 'ProjectV2']),
    kind('RepositoryOwner', 'INTERFACE', ['login: String!', 'repositories(first: Int): RepositoryConnection!'],
         ['User', 'Organization']),
    kind('Actor', 'INTERFACE', ['login: String!'], ['User', 'Bot']),
    kind('SearchResultItem', 'UNION', (), ['Issue', 'PullRequest', 'Repository', 'User']),
    kind('AddCommentInput', 'INPUT_OBJECT', inputs=['subjectId: ID!', 'body: String!', 'clientMutationId: String']),
    kind('CreatePullRequestInput', 'INPUT_OBJECT',
         inputs=['repositoryId: ID!', 'baseRefName: String!', 'headRefName: String!', 'title: String!']),
    kind('CreateRepositoryInput', 'INPUT_OBJECT', inputs=['name: String!']),
    obj('__Schema', 'types: [__Type!]!'),
    obj('__Type', 'name: String', 'kind: __TypeKind!', 'fields(includeDeprecated: Boolean): [__Field!]'),
    obj('__Field', 'name: String!'),
    *(kind(name, 'SCALAR') for name in ('ID', 'String', 'Int', 'Boolean')),
    kind('SearchType', 'ENUM'), kind('__TypeKind', 'ENUM'),
]
# The input object kinds above are written as OBJECT references; fix them by name.
INPUTS = {'AddCommentInput', 'CreatePullRequestInput', 'CreateRepositoryInput'}
INTERFACES = {'Node': 'INTERFACE', 'RepositoryOwner': 'INTERFACE', 'Actor': 'INTERFACE', 'SearchResultItem': 'UNION'}


def fix_kinds(value):
    if isinstance(value, dict):
        if value.get('name') in INPUTS and 'ofType' in value:
            value['kind'] = 'INPUT_OBJECT'
        elif value.get('name') in INTERFACES and 'ofType' in value:
            value['kind'] = INTERFACES[value['name']]
        for item in value.values():
            fix_kinds(item)
    elif isinstance(value, list):
        for item in value:
            fix_kinds(item)


fix_kinds(TYPES)
SCHEMA = graphql.Schema({'__schema': {'queryType': {'name': 'Query'}, 'mutationType': {'name': 'Mutation'},
                                      'types': TYPES}})
GRANTS = {'example/project': 'push', 'example/library': 'read'}


def allowed(repository, access):
    granted = GRANTS.get(repository.lower())
    return granted == 'push' or granted == access == 'read'


def plan(query, variables=None, **body):
    return policy.plan(SCHEMA, dict(query=query, variables=variables or {}, **body), allowed)


class ParserTests(unittest.TestCase):
    def test_documents_are_read_strictly(self):
        for text in ['{ a ', 'query { a } }', 'query { a(x: 1, x: 2) }', 'query($a: Int, $a: Int) { b }',
                     'fragment f on X { a } fragment f on X { a } { b }', '{ a } \x00', 'fragment on on X { a }',
                     '{' * 50 + '}' * 50, '{ a(x: "\\q") }']:
            with self.assertRaises(graphql.Invalid, msg=text):
                graphql.parse(text)

    def test_strings_and_offsets(self):
        document = graphql.parse('query Q($n: [ID!]! = ["a"]) { x(s: "t\\u0041\\n", b: """\n  one\n    two\n""") { y } }')
        operation = document.operations[0]
        self.assertEqual(operation.variables['n'][0], '[ID!]!')
        field = operation.selection_set.selections[0]
        self.assertEqual(field.arguments['s'], ('string', 'tA\n'))
        self.assertEqual(field.arguments['b'], ('string', 'one\n  two'))
        self.assertEqual(document.text[field.selection_set.end], '}')
        self.assertIn('Q', document.names)


class QueryTests(unittest.TestCase):
    def test_a_granted_repository_is_read_and_each_object_is_checked(self):
        query = 'query($o: String!, $n: String!) { repository(owner: $o, name: $n) { name pullRequest(number: 1) ' \
                '{ title comments(first: 5) { nodes { body } } } } }'
        sent = plan(query, {'o': 'example', 'n': 'library'})
        held, own, _ = sent.markers
        self.assertIn('%s: nameWithOwner' % own, sent.text)
        self.assertEqual(sent.text.count('%s: repository { nameWithOwner }' % held), 2)
        self.assertEqual(sent.body({'o': 'example', 'n': 'library'})['query'], sent.text)
        reply = {'data': {'repository': {'name': 'library', own: 'example/library', 'pullRequest': {
            'title': 't', held: {'nameWithOwner': 'example/library'},
            'comments': {'nodes': [{'body': 'b', held: {'nameWithOwner': 'Example/Library'}}]}}}}}
        self.assertEqual(policy.verify(sent, reply, allowed), {'data': {'repository': {
            'name': 'library', 'pullRequest': {'title': 't', 'comments': {'nodes': [{'body': 'b'}]}}}}})
        reply['data']['repository']['pullRequest']['comments']['nodes'][0][held] = {'nameWithOwner': 'example/secret'}
        with self.assertRaisesRegex(policy.Refused, 'example/secret'):
            policy.verify(sent, reply, allowed)
        failed = {'data': {'repository': None}, 'errors': [{'path': ['repository', 'pullRequest', held]}]}
        with self.assertRaisesRegex(policy.Refused, 'did not say which repository'):
            policy.verify(sent, failed, allowed)
        # An object that GitHub puts in no repository is not shown either.
        reply['data']['repository']['pullRequest']['comments']['nodes'][0][held] = None
        with self.assertRaisesRegex(policy.Refused, 'another repository'):
            policy.verify(sent, reply, allowed)

    def test_a_repository_without_a_grant_is_refused_before_github(self):
        with self.assertRaisesRegex(policy.Refused, 'example/secret has no GitHub grant'):
            plan('{ repository(owner: "example", name: "secret") { name } }')
        with self.assertRaisesRegex(policy.Refused, 'owner and a name'):
            plan('query($o: String) { repository(owner: $o, name: "x") { name } }')

    def test_only_listed_entry_fields_and_plain_owner_fields(self):
        self.assertEqual(plan('query UserCurrent { viewer { login } }').text, 'query UserCurrent { viewer { login } }')
        for query in ['{ organization(login: "example") { login } }',
                      '{ viewer { repositories(first: 5) { nodes { name } } } }',
                      '{ viewer { repository(name: "secret") { name } } }',
                      '{ node(id: "T") { ...on Topic { repositories { totalCount } } } }',
                      '{ repository(owner: "example", name: "library") { owner { repositories { nodes { name } } } } }',
                      '{ repository(owner: "example", name: "library") { pullRequest(number: 1) '
                      '{ author { ...on User { repositories { nodes { name } } } } } } }']:
            with self.assertRaises(policy.Refused, msg=query):
                plan(query)
        plan('{ repository(owner: "example", name: "library") { owner { login } pullRequest(number: 1) '
             '{ author { login ...on User { name } } } } }')

    def test_search_needs_granted_repositories_and_no_operators(self):
        plan('{ search(query: "is:open repo:example/library", type: ISSUE) { issueCount } }')
        for text, reason in [('is:open', 'needs a repo: qualifier'), ('repo:example/secret', 'example/secret'),
                             ('repo:example/library OR repo:example/secret', 'cannot use OR'),
                             ('repo:example/library (x)', 'parentheses'),
                             ('repo:example/library "open', 'quotation mark'),
                             ('repo:example/library org:example', 'org:, user: or owner:'),
                             ('repo:example/library -user:x', 'org:, user: or owner:')]:
            with self.assertRaisesRegex(policy.Refused, reason):
                plan('query($q: String!) { search(query: $q, type: ISSUE) { issueCount } }', {'q': text})

    def test_a_node_is_read_only_as_a_thing_in_a_repository_or_an_owner(self):
        sent = plan('query($id: ID!) { node(id: $id) { ...on PullRequest { title } } }', {'id': 'PR_1'})
        held = sent.markers[0]
        reply = {'data': {'node': {'title': 'x', held: {'nameWithOwner': 'example/secret'}}}}
        with self.assertRaises(policy.Refused):
            policy.verify(sent, reply, allowed)
        # A plain `id` on the interface still asks which repository each possible type is in,
        # and what type each object is.
        sent = plan('{ nodes(ids: ["a", "b"]) { id } }')
        held, _, kind = sent.markers
        self.assertIn('...on PullRequest', sent.text)
        self.assertIn('...on Repository', sent.text)
        self.assertIn('%s: __typename' % kind, sent.text)
        reply = {'data': {'nodes': [{'id': 'a', kind: 'PullRequest', held: {'nameWithOwner': 'example/library'}},
                                    {'id': 'b', kind: 'User'}]}}
        self.assertEqual(policy.verify(sent, reply, allowed), {'data': {'nodes': [{'id': 'a'}, {'id': 'b'}]}})
        for other in ['ProjectV2', 'MarketplaceListing', None]:
            with self.assertRaisesRegex(policy.Refused, 'cannot be', msg=other):
                policy.verify(sent, {'data': {'nodes': [{'id': 'b', kind: other}]}}, allowed)
        for query in ['{ node(id: "a") { ...on MarketplaceListing { name } } }',
                      '{ node(id: "a") { ...on Node { id } } }', '{ node(id: "a") { ...L } } fragment L on '
                      'MarketplaceListing { name }', '{ node(id: "a") { ...on ProjectV2 { readme } } }']:
            with self.assertRaisesRegex(policy.Refused, 'MarketplaceListing|Node|ProjectV2', msg=query):
                plan(query)
        plan('{ node(id: "a") { ...on User { login } ...on Organization { login } } }')

    def test_a_fork_is_named_without_a_check_until_its_data_is_read(self):
        query = '{ repository(owner: "example", name: "library") { pullRequest(number: 1) { headRepository { %s } } } }'
        own = 1
        for identity in ['nameWithOwner', '...on Repository { nameWithOwner }', '...F', 'isPrivate ...on Repository { ...F }']:
            text = query % identity + (' fragment F on Repository { nameWithOwner }' if 'F' in identity else '')
            sent = plan(text)
            self.assertEqual(sent.text.count(sent.markers[own]), 1, 'only the granted repository: ' + text)
        for read in ['issues(first: 1) { nodes { title } }', '...on Repository { issues(first: 1) { nodes { title } } }',
                     '...F']:
            text = query % read + (' fragment F on Repository { nameWithOwner issues(first: 1) { nodes { title } } }'
                                   if 'F' in read else '')
            sent = plan(text)
            self.assertGreaterEqual(sent.text.count(sent.markers[own]), 2, text)
        # A fragment that names a fork and also reads a repository is checked where it reads.
        text = ('{ repository(owner: "example", name: "library") { ...F pullRequest(number: 1) { headRepository '
                '{ ...F } } } } fragment F on Repository { nameWithOwner }')
        sent = plan(text)
        self.assertIn('fragment F on Repository { nameWithOwner  %s: nameWithOwner }' % sent.markers[own], sent.text)

    def test_requests_that_could_hide_or_widen_what_they_read_are_refused(self):
        for body, reason in [
                ({'query': '{ viewer { login } } query B { viewer { login } }'}, 'exactly one operation'),
                ({'query': 'subscription { viewer { login } }'}, 'subscriptions'),
                ({'query': 'query A { viewer { login } }', 'operationName': 'B'}, 'does not match'),
                ({'query': '{ viewer { login } }', 'extensions': {}}, 'object with a query'),
                ({'query': '{ viewer { login } }', 'variables': []}, 'variables are an object'),
                ({'query': '{ viewer { login } } fragment F on User { login }'}, 'not used'),
                ({'query': '{ viewer { ...F } } fragment F on User { ...G } fragment G on User { ...F }'},
                 'spreads itself'),
                ({'query': '{ repository(owner: "example", name: $n) { name } }'}, 'does not declare'),
                ({'query': '{ viewer { nope } }'}, 'has no field nope'),
                ({'query': '{ repository(owner: "example", name: "library") { pullRequest(number: 1) } }'},
                 'needs a selection')]:
            with self.assertRaisesRegex(policy.Refused, reason, msg=body):
                policy.plan(SCHEMA, body, allowed)

    def test_added_aliases_are_not_names_of_the_request(self):
        sent = plan('{ repository(owner: "example", name: "library") { name } }')
        for marker in sent.markers:
            self.assertNotIn(marker, graphql.parse('{ repository(owner: "example", name: "library") { name } }').names)
        self.assertEqual(len(set(sent.markers)), 3)

    def test_introspection_is_allowed(self):
        plan('{ __type(name: "PullRequest") { fields(includeDeprecated: true) { name } } }')
        plan('{ __schema { types { name } } }')


class MutationTests(unittest.TestCase):
    def test_only_listed_mutations(self):
        for text in ['mutation { createRepository(input: {name: "x"}) { repository { name } } }',
                     'mutation { ...on Mutation { createRepository(input: {name: "x"}) { repository { name } } } }',
                     'mutation { ... { createRepository(input: {name: "x"}) { clientMutationId } } }',
                     'mutation { ...M } fragment M on Mutation { createRepository(input: {name: "x"}) '
                     '{ repository { name } } }']:
            with self.assertRaisesRegex(policy.Refused, 'createRepository is not allowed', msg=text):
                plan(text)

    def test_fragments_at_the_top_get_the_checks_of_root_fields(self):
        sent = plan('mutation { ...M } fragment M on Mutation { addComment(input: {subjectId: "I_1", body: "b"}) '
                    '{ clientMutationId } }')
        self.assertEqual(sent.ids, ['I_1'])
        with self.assertRaisesRegex(policy.Refused, 'example/secret has no GitHub grant'):
            plan('{ ...Q } fragment Q on Query { repository(owner: "example", name: "secret") { name } }')
        with self.assertRaisesRegex(policy.Refused, 'organization is not allowed'):
            plan('{ ...on Query { organization(login: "example") { login } } }')
        with self.assertRaisesRegex(policy.Refused, 'not Query'):
            plan('{ ...on User { login } }')
        plan('{ ...on Query { repository(owner: "example", name: "library") { name } } }')

    def test_each_input_id_is_looked_up_and_needs_push(self):
        sent = plan('mutation($input: AddCommentInput!) { addComment(input: $input) { clientMutationId } }',
                    {'input': {'subjectId': 'I_1', 'body': 'hello'}})
        self.assertTrue(sent.mutation)
        self.assertEqual(sent.ids, ['I_1'])
        lookup = policy.lookup(SCHEMA, sent.ids + ['I_1'])
        self.assertEqual(lookup['variables'], {'ids': ['I_1']})
        self.assertIn('...on Issue { held: repository { nameWithOwner } }', lookup['query'])
        self.assertNotIn('Payload', lookup['query'], 'only types that a node can be')
        policy.check_lookup({'data': {'nodes': [{'__typename': 'Issue', 'held': {'nameWithOwner': 'example/project'}}]}},
                            sent.ids, allowed)
        for nodes, reason in [([{'__typename': 'Issue', 'held': {'nameWithOwner': 'example/library'}}], 'no GitHub grant'),
                              ([{'__typename': 'MarketplaceListing'}], 'MarketplaceListing'),
                              ([None], 'does not show'), ([], 'did not say')]:
            with self.assertRaisesRegex(policy.Refused, reason):
                policy.check_lookup({'data': {'nodes': nodes}}, sent.ids, allowed)
        policy.check_lookup({'data': {'nodes': [{'__typename': 'User'}]}}, sent.ids, allowed)

    def test_inputs_follow_the_schema(self):
        for variables, reason in [({'input': {'subjectId': 1, 'body': 'x'}}, 'not a string'),
                                  ({'input': {'subjectId': 'I', 'body': 'x', 'extra': 1}}, 'has no field extra'),
                                  ({'input': ['I']}, 'not an object')]:
            with self.assertRaisesRegex(policy.Refused, reason):
                plan('mutation($input: AddCommentInput!) { addComment(input: $input) { clientMutationId } }',
                     variables)
        sent = plan('mutation { createPullRequest(input: {repositoryId: "R_1", baseRefName: "main", '
                    'headRefName: "x", title: "t"}) { pullRequest { number } } }')
        self.assertEqual(sent.ids, ['R_1'])
        self.assertIn(sent.markers[0], sent.text)


if __name__ == '__main__':
    unittest.main()
