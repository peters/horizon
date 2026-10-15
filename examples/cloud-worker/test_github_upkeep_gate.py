"""The merge gate of an Upkeep: what GitHub must show before the broker lets a merge go, and
which blockers the person's approval lifts. Synthetic tokens and loopback servers only."""
import json
import unittest
from unittest import mock

from test_github_api_broker import BrokerTestCase
from test_github_upkeep_policy import saved
from test_horizon_worker_github import service

upkeep = service.upkeep
checks = service.broker.checks
HEAD = 'a' * 40
BASE = 'c' * 40


def approval(**changes):
    return {'id': 31, 'user': {'login': 'Owner'}, 'state': 'APPROVED', 'commit_id': HEAD, **changes}


class Fake:
    """GitHub's answers for the gate, by path; records the paths asked."""

    def __init__(self, **changes):
        self.pull = {'state': 'open', 'draft': False, 'user': {'login': 'dependabot[bot]'},
                     'head': {'sha': HEAD, 'ref': 'dependabot/cargo/x', 'repo': {'full_name': 'example/project'}},
                     'base': {'sha': BASE, 'ref': 'main', 'repo': {'default_branch': 'main'}},
                     'mergeable_state': 'clean'}
        self.runs = [{'name': 'ci', 'status': 'completed', 'conclusion': 'success'},
                     {'name': 'lint', 'status': 'completed', 'conclusion': 'skipped'}]
        self.status = {'state': 'pending', 'statuses': []}
        self.files = [{'filename': 'Cargo.lock'}]
        self.commits = [{'author': {'login': 'dependabot[bot]'}, 'committer': {'login': 'web-flow'},
                         'commit': {'verification': {'verified': True}}}]
        self.reviews = []
        self.total = None
        self.__dict__.update(changes)
        self.asked = []

    def __call__(self, method, path):
        self.asked.append(path)
        route = path.split('?')[0]
        if route == '/repos/example/project/compare/%s...%s' % (BASE, HEAD):
            return 200, {'files': self.files, 'commits': self.commits,
                         'total_commits': len(self.commits) if self.total is None else self.total}
        answers = {'/user': {'login': 'owner'}, '/pulls/7': self.pull, '/check-runs': {'check_runs': self.runs},
                   '/status': self.status, '/reviews': self.reviews}
        for end, answer in answers.items():
            if route.endswith(end):
                return 200, answer
        return 404, None


class GateTests(unittest.TestCase):
    def setUp(self):
        self.rules = upkeep.Policy({'dependabot[bot]'}, ())

    def gate(self, fake, head=HEAD):
        checks.gate(self.rules, fake, 'example/project', 7, head)

    def refused(self, fake, text, **options):
        with self.assertRaises(upkeep.Refused) as raised:
            self.gate(fake, **options)
        self.assertIn(text, str(raised.exception))
        self.assertTrue(str(raised.exception).startswith('this Upkeep'), raised.exception)
        return str(raised.exception)

    def test_a_bot_pull_request_with_green_checks_on_its_head_passes(self):
        fake = Fake()
        self.gate(fake)
        self.assertFalse(any('/reviews' in path for path in fake.asked))

    def test_hard_checks_are_never_lifted(self):
        approved = [approval()]
        self.refused(Fake(reviews=approved), '--match-head-commit', head=None)
        self.refused(Fake(reviews=approved), 'not the commit', head='b' * 40)
        for runs in ([{'name': 'ci', 'status': 'in_progress', 'conclusion': None}],
                     [{'name': 'ci', 'status': 'completed', 'conclusion': 'failure'}]):
            self.refused(Fake(runs=runs, reviews=approved), 'the check ci')
        self.refused(Fake(status={'state': 'failure', 'statuses': [{'context': 'x'}]}, reviews=approved),
                     'commit statuses')
        for state in ('blocked', 'behind', 'dirty', 'unstable', 'unknown', None):
            fake = Fake(reviews=approved)
            fake.pull['mergeable_state'] = state
            self.refused(fake, 'not ready to merge')
        for change in ({'state': 'closed'}, {'draft': True}):
            fake = Fake(reviews=approved)
            fake.pull.update(change)
            self.refused(fake, 'not an open pull request')
        # A pull request on another branch could move to the default branch after the gate.
        for base in ({'sha': BASE, 'ref': 'release', 'repo': {'default_branch': 'main'}},
                     {'sha': BASE, 'ref': 'main', 'repo': {}}, {'sha': BASE}):
            fake = Fake(reviews=approved)
            fake.pull['base'] = base
            self.refused(fake, 'only into the default branch')

    def test_a_plain_approval_lifts_the_author_and_no_checks(self):
        person = Fake()
        person.pull['user'] = {'login': 'someone'}
        text = self.refused(person, 'blocked on author')
        self.assertIn('Approve to merge (head aaaaaaaaaaaa)', text)
        person.reviews = [approval()]
        self.gate(person)
        self.refused(Fake(runs=[]), 'blocked on no-checks')
        self.gate(Fake(runs=[], reviews=[approval()]))
        self.gate(Fake(runs=[], status={'state': 'success', 'statuses': [{'context': 'x'}]}))
        own = Fake()
        own.pull.update(user={'login': 'Owner'})
        own.pull['head']['ref'] = 'upkeep/dotnet-10.0.13'
        self.gate(own)
        for change in ({'ref': 'feature/x'}, {'repo': {'full_name': 'fork/project'}}):
            other = Fake()
            other.pull['user'] = {'login': 'owner'}
            other.pull['head'].update({'ref': 'upkeep/x', **change})
            self.refused(other, 'blocked on author')

    def test_a_workflow_change_needs_the_bots_commits_or_the_approval_line(self):
        touched = [{'filename': '.github/workflows/ci.yml'}]
        self.gate(Fake(files=touched))
        renamed = [{'filename': 'old.yml', 'previous_filename': '.github/workflows/ci.yml'}]
        agent = {'author': {'login': 'owner'}, 'committer': {'login': 'owner'},
                 'commit': {'verification': {'verified': False}}}
        forged = {'author': {'login': 'dependabot[bot]'}, 'committer': {'login': 'owner'},
                  'commit': {'verification': {'verified': True}}}
        unsigned = {'author': {'login': 'dependabot[bot]'}, 'committer': {'login': 'web-flow'},
                    'commit': {'verification': {'verified': False}}}
        for files in (touched, renamed):
            for commits in ([*Fake().commits, agent], [forged], [unsigned], []):
                fake = Fake(files=files, commits=commits, reviews=[approval()])
                text = self.refused(fake, 'blocked on workflows')
                self.assertIn('Approve with the line upkeep-allow: workflows', text)
        for body in ('Looks fine.\nupkeep-allow: workflows', 'Reviewed.\r\nupkeep-allow: workflows\r\nThanks.'):
            self.gate(Fake(files=touched, commits=[agent], reviews=[approval(body=body)]))
        # A list that GitHub cut short is not every commit.
        self.refused(Fake(files=touched, total=251), 'blocked on workflows')
        self.refused(Fake(files=[{'filename': 'x%d' % n} for n in range(300)]), 'blocked on workflows')
        # The comparison is by commit: a base that GitHub does not show is refused.
        unknown = Fake()
        unknown.pull['base'] = {'ref': 'main', 'repo': {'default_branch': 'main'}}
        self.refused(unknown, 'base commit')

    def test_only_the_persons_approval_of_this_head_counts(self):
        person = Fake()
        person.pull['user'] = {'login': 'someone'}
        for review in (approval(user={'login': 'someone'}), approval(state='COMMENTED'), approval(commit_id='b' * 40),
                       approval(user=None)):
            person.reviews = [review]
            self.refused(person, 'blocked on author')

    def test_the_persons_latest_decision_counts_and_an_own_pull_request_is_merged_by_hand(self):
        person = Fake()
        person.pull['user'] = {'login': 'someone'}
        person.reviews = [approval(), approval(id=32, state='CHANGES_REQUESTED')]
        self.refused(person, 'blocked on author')
        person.reviews = [approval(id=30, state='CHANGES_REQUESTED'), approval(),
                          approval(id=33, state='COMMENTED', user={'login': 'owner'})]
        self.gate(person)
        person.reviews = [approval(), approval(id=34, state='DISMISSED')]
        self.refused(person, 'blocked on author')
        own = Fake(runs=[])
        own.pull.update(user={'login': 'owner'})
        own.pull['head']['ref'] = 'upkeep/x'
        self.refused(own, 'merge it yourself')

    def test_every_spelling_of_a_rest_merge_reaches_the_gate(self):
        self.assertEqual(checks.merge_number(['pulls', '12', 'merge'], 'PUT'), 12)
        self.assertIsNone(checks.merge_number(['pulls', '12', 'merge'], 'GET'))
        self.assertIsNone(checks.merge_number(['pulls', '12', 'reviews'], 'PUT'))
        for parts in (['pulls', '007', 'merge'], ['pulls', '7', 'merge', ''], ['pulls', 'merge']):
            with self.assertRaises(upkeep.Refused):
                checks.merge_number(parts, 'PUT')

    def test_a_list_that_does_not_end_is_refused(self):
        class Endless(Fake):
            def __call__(self, method, path):
                return (200, {'check_runs': [{}] * 100}) if '/check-runs' in path else super().__call__(method, path)
        self.refused(Endless(), 'too many entries')


class GateBrokerTests(BrokerTestCase):
    """Merges through the broker of an Upkeep cloud, in front of a fake GitHub."""

    def setUp(self):
        super().setUp()
        saved(self.store)
        self.log = upkeep.log_directory(self.store) / upkeep.LOG

    def test_a_rest_merge_goes_only_through_the_gate(self):
        with mock.patch.object(checks, 'gate', side_effect=upkeep.Refused('the check ci did not pass.')) as gate:
            status, _, payload = self.send('PUT', '/repos/example/project/pulls/7/merge', body={'sha': HEAD})
        self.assertEqual((status, json.loads(payload)['message']), (403, 'Horizon: the check ci did not pass.'))
        self.assertEqual(gate.call_args.args[2:5], ('example/project', 7, HEAD))
        self.assertIn('"kind": "merge"', self.log.read_text())
        self.assertFalse(any(path.endswith('/merge') for _, _, path, *_ in self.api.seen))
        with mock.patch.object(checks, 'gate') as gate:
            status, _, _ = self.send('PUT', '/repos/example/project/pulls/7/merge',
                                     body={'merge_method': 'squash', 'sha': HEAD})
        self.assertEqual(status, 200)
        method, _, path, _, data = self.api.seen[-1]
        self.assertEqual((method, path, json.loads(data)['sha']), ('PUT', '/repos/example/project/pulls/7/merge', HEAD))
        before = len(self.api.seen)
        status, _, payload = self.send('PUT', '/repos/example/project/pulls/7/merge',
                                       body=b'{"sha": "%s", "sha": "%s"}' % (HEAD.encode(), b'b' * 40))
        self.assertEqual(status, 403)
        with mock.patch.object(checks, 'gate'):
            for target, fields in [('/repos/example/project/pulls/7/merge?sha=' + 'b' * 40, ()),
                                   ('/repos/example/project/pulls/7/merge', ('Content-Type: text/plain',))]:
                status, _, payload = self.send('PUT', target, body={'sha': HEAD}, fields=fields)
                self.assertEqual(status, 403, target)
                self.assertIn('Horizon: this Upkeep', json.loads(payload)['message'])
        self.assertEqual(len(self.api.seen), before)

    def test_a_graphql_merge_goes_through_the_gate(self):
        def answer(body):
            if 'nodes(ids:' in body['query']:
                return {'data': {'nodes': [{'__typename': 'PullRequest', 'held': {'nameWithOwner': 'example/project'}}]}}
            return {'data': {'node': {'__typename': 'PullRequest', 'number': 7,
                                      'repository': {'nameWithOwner': 'example/project'}}}}
        self.api.graphql = answer
        merge = ('mutation($id: ID!, $head: String) { mergePullRequest(input: {pullRequestId: $id, '
                 'expectedHeadOid: $head, mergeMethod: "SQUASH"}) { clientMutationId } }')
        with mock.patch.object(checks, 'gate', side_effect=upkeep.Refused('#7 is blocked on author.')) as gate:
            reply = self.graphql(merge, {'id': 'PR_1', 'head': HEAD})
        self.assertEqual(reply['errors'][0]['message'], 'Horizon: #7 is blocked on author.')
        self.assertEqual(gate.call_args.args[2:5], ('example/project', 7, HEAD))
        self.assertFalse(any(b'mergePullRequest' in (data or b'') for *_, data in self.api.seen))
        # GitHub would run the first mutation after the gate read the pull request.
        both = ('mutation($id: ID!, $head: String) { a: addComment(input: {subjectId: $id, body: "x"}) '
                '{ clientMutationId } b: mergePullRequest(input: {pullRequestId: $id, expectedHeadOid: $head}) '
                '{ clientMutationId } }')
        with mock.patch.object(checks, 'gate') as gate:
            reply = self.graphql(both, {'id': 'PR_1', 'head': HEAD})
        self.assertIn('without other mutations', reply['errors'][0]['message'])
        gate.assert_not_called()
        self.assertFalse(any(b'mergePullRequest' in (data or b'') for *_, data in self.api.seen))

    def test_an_agent_cannot_change_the_base_that_the_gate_compares_with(self):
        # The gate compares the pull request with its base; a new base after that check would
        # let the merge bring in what the gate never saw.
        before = len(self.api.seen)
        for target, body in [('/repos/example/project/pulls/7', {'title': 'x', 'base': 'main'}),
                             ('/repos/example/project/pulls/7', b'{"title": "x", "base": null}'),
                             ('/repos/example/project/pulls/07', {'title': 'x'}),
                             ('/repos/example/project/pulls/7/', {'title': 'x', 'base': 'main'}),
                             ('/repos/example/project/pulls/7/x', {'title': 'x'})]:
            status, _, payload = self.send('PATCH', target, body=body)
            self.assertEqual(status, 403, target)
            # The broker's routes refuse a trailing slash before the Upkeep policy does.
            self.assertIn('Horizon: this', json.loads(payload)['message'])
        self.assertEqual(len(self.api.seen), before)
        status, _, _ = self.send('PATCH', '/repos/example/project/pulls/7', body={'title': 'x', 'body': 'y'})
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(self.api.seen[-1][-1]), {'title': 'x', 'body': 'y'})
        self.api.graphql = lambda body: {'data': {'nodes': [{'__typename': 'PullRequest',
                                                             'held': {'nameWithOwner': 'example/project'}}]}}
        edit = ('mutation($id: ID!, $base: String) { updatePullRequest(input: {pullRequestId: $id, '
                'baseRefName: $base}) { clientMutationId } }')
        reply = self.graphql(edit, {'id': 'PR_1', 'base': 'main'})
        self.assertIn('base branch', reply['errors'][0]['message'])
        self.assertFalse(any(b'updatePullRequest' in (data or b'') for *_, data in self.api.seen))
        reply = self.graphql(edit, {'id': 'PR_1'})
        self.assertNotIn('errors', reply)
        self.assertTrue(any(b'updatePullRequest' in (data or b'') for *_, data in self.api.seen))


if __name__ == '__main__':
    unittest.main()
