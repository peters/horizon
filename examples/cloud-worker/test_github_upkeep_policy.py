"""The Upkeep policy of the worker's GitHub access: what Horizon sets, and what the API broker
refuses with it. Synthetic tokens and loopback servers only."""
import json
import os
from pathlib import Path
import stat
import unittest
from unittest import mock

from test_github_api_broker import BrokerTestCase
from test_horizon_worker_github import ServiceTestCase, service

upkeep = service.upkeep
common = service.common
POLICY = {'version': 1, 'session': 'upkeep-c1', 'repositories': ['example/project', 'example/other'],
          'bots': ['dependabot[bot]', 'app/renovate'],
          'workflows': [{'repository': 'example/project', 'workflow': 'release.yml'}]}
HEAD = 'a' * 40


def saved(store, value=None):
    return upkeep.install(store, common, json.dumps(value or POLICY), store.runtime.parent / 'marker')


class PolicyTests(ServiceTestCase):
    def test_horizon_sets_a_checked_policy_that_every_storage_keeps(self):
        self.assertEqual(saved(self.store), {
            'session': 'upkeep-c1', 'repositories': ['example/project', 'example/other'],
            'bots': ['dependabot[bot]', 'renovate[bot]'],
            'workflows': [{'repository': 'example/project', 'workflow': 'release.yml'}]})
        policy = upkeep.load(self.store, common)
        self.assertEqual(policy.bots, {'dependabot[bot]', 'renovate[bot]'})
        self.assertEqual(policy.workflows, {('example/project', 'release.yml')})
        for directory in (self.store.persistent, self.store.runtime):
            self.assertEqual(stat.S_IMODE((directory / upkeep.POLICY).stat().st_mode), 0o600)
        marker = self.store.runtime.parent / 'marker'
        self.assertEqual((marker.read_text(), stat.S_IMODE(marker.stat().st_mode)),
                         (upkeep.digest(POLICY) + '\n', 0o644))
        self.assertEqual(upkeep.load(self.store, common, marker).workflows, {('example/project', 'release.yml')})
        for invalid in [{**POLICY, 'extra': 1}, {**POLICY, 'version': 2}, {**POLICY, 'version': True},
                        {**POLICY, 'bots': ['-x']}, {**POLICY, 'session': 'other'},
                        {**POLICY, 'session': 'upkeep-' + 'x' * 91}, {**POLICY, 'repositories': []},
                        {**POLICY, 'repositories': ['example/a', 'Example/A']},
                        {**POLICY, 'repositories': ['example/%d' % n for n in range(51)]},
                        {**POLICY, 'workflows': [{'repository': 'example/third', 'workflow': 'a.yml'}]},
                        {**POLICY, 'bots': ['a'] * 21},
                        {**POLICY, 'workflows': [{'repository': 'example/project', 'workflow': '../x.yml'}]},
                        {**POLICY, 'workflows': [{'repository': 'example', 'workflow': 'a.yml'}]},
                        {**POLICY, 'workflows': [{'repository': 'example/project', 'workflow': 'a.sh'}]}]:
            with self.assertRaises(ValueError, msg=invalid):
                saved(self.store, invalid)

    def test_a_storage_that_misses_a_new_policy_keeps_no_older_copy(self):
        saved(self.store)
        changed = {**POLICY, 'workflows': []}
        write = common.write_private

        def failing(directory, name, data):
            if Path(directory) == self.store.persistent:
                raise OSError('disk full')
            return write(directory, name, data)
        with mock.patch.object(common, 'write_private', failing):
            saved(self.store, changed)
        self.assertFalse((self.store.persistent / upkeep.POLICY).exists())
        self.assertEqual(upkeep.load(self.store, common).workflows, frozenset())
        # A storage that keeps its older copy: the marker names the new policy, so the old
        # copy no longer counts, and the policy is the strictest.
        marker = self.store.runtime.parent / 'marker'
        saved(self.store)
        with mock.patch.object(common, 'write_private', failing), \
                mock.patch.object(common, 'remove_durably', side_effect=OSError('read-only')):
            with self.assertRaises(ValueError):
                saved(self.store, {**POLICY, 'bots': []})
        self.assertEqual(upkeep.marked(marker), upkeep.digest({**POLICY, 'bots': []}))
        self.assertEqual(upkeep.load(self.store, common, marker).workflows, frozenset())
        upkeep.mark(upkeep.digest(POLICY), marker)
        self.assertEqual(upkeep.load(self.store, common, marker).workflows, {('example/project', 'release.yml')})
        # A policy that loses every copy fails closed: the marker stays, and with it the
        # strictest policy; without a marker there is none and no agent starts.
        with mock.patch.object(common, 'write_private', side_effect=OSError('disk full')):
            with self.assertRaises(ValueError):
                saved(self.store, changed)
        self.assertTrue(marker.exists())
        self.assertEqual(upkeep.load(self.store, common, marker).workflows, frozenset())
        marker.unlink()
        self.assertIsNone(upkeep.load(self.store, common, marker))

    def test_a_cloud_without_a_policy_has_none_and_a_broken_one_is_the_strictest(self):
        self.assertIsNone(upkeep.load(self.store, common))
        saved(self.store)
        (self.store.persistent / upkeep.POLICY).write_text('{')
        policy = upkeep.load(self.store, common)
        self.assertEqual((policy.bots, policy.workflows), (frozenset(), frozenset()))
        (self.store.persistent / upkeep.POLICY).unlink()
        (self.store.runtime / upkeep.POLICY).chmod(0o644)
        self.assertEqual(upkeep.load(self.store, common).bots, frozenset())

    def test_the_policy_refuses_changes_that_an_upkeep_never_needs(self):
        policy = upkeep.Policy()
        refused = [('POST', 'releases'), ('PATCH', 'releases/1'), ('POST', 'releases/assets'),
                   ('POST', 'check-runs'), ('PATCH', 'check-runs/1'), ('POST', 'check-suites'),
                   ('PATCH', 'check-suites/preferences'), ('POST', 'check-runs/01/rerequest'),
                   ('POST', 'dispatches'), ('POST', 'deployments'), ('POST', 'statuses/' + HEAD), ('PUT', 'contents/.github/workflows/ci.yml'),
                   ('DELETE', 'contents/.github/workflows/ci.yml'), ('POST', 'actions/runs/1/approve'),
                   ('POST', 'actions/runs/1/pending_deployments'), ('DELETE', 'actions/caches'),
                   ('DELETE', 'actions/runs/1'), ('POST', 'actions/workflows/1/enable'),
                   ('PUT', 'pulls/3/reviews/8/dismissals'), ('POST', 'pulls/3/reviews/'),
                   ('POST', 'pulls/03/reviews'), ('POST', 'pulls/3/reviews/8/events/'),
                   ('PUT', 'pulls/3/reviews/8/dismissals/'), ('DELETE', 'pulls/3/reviews/08'),
                   ('POST', 'merges'), ('POST', 'merge-upstream'), ('POST', 'git/refs'),
                   ('PATCH', 'git/refs/tags/v1'), ('DELETE', 'git/refs/tags/v1'), ('PATCH', 'git/refs/notes/x'),
                   ('PATCH', 'git/refs/heads'), ('PATCH', 'git/refs/heads/x/'), ('PUT', 'pulls/04/update-branch'),
                   ('PUT', 'pulls/4/update-branch/'), ('PUT', 'pulls/3/reviews/8')]
        for method, path in refused:
            self.assertIsNotNone(upkeep.rest_refusal(policy, method, path.split('/')), (method, path))
        allowed = [('GET', 'git/trees/x'), ('POST', 'issues/1/comments'), ('POST', 'labels'),
                   ('POST', 'issues/1/labels'), ('POST', 'pulls'), ('PATCH', 'pulls/1'),
                   ('PUT', 'contents/src/main.rs'), ('PUT', 'contents/.github/dependabot.yml'),
                   ('POST', 'actions/runs/1/rerun'), ('POST', 'actions/runs/1/rerun-failed-jobs'),
                   ('POST', 'actions/runs/1/cancel'), ('POST', 'actions/jobs/1/rerun'),
                   ('POST', 'check-runs/1/rerequest'), ('POST', 'check-suites/1/rerequest'),
                   ('PUT', 'pulls/1/merge'), ('DELETE', 'git/refs/heads/dependabot/cargo/x'),
                   ('POST', 'pulls/3/reviews'), ('DELETE', 'pulls/3/reviews/8'),
                   ('POST', 'actions/workflows/release.yml/dispatches')]
        for method, path in allowed:
            self.assertIsNone(upkeep.rest_refusal(policy, method, path.split('/')), (method, path))
        self.assertIsNone(upkeep.rest_refusal(None, 'POST', ['releases']))
        self.assertEqual(upkeep.dispatched(['actions', 'workflows', '7', 'dispatches'], 'POST'), '7')
        for method, path, read in [('POST', 'pulls/3/reviews', True), ('POST', 'pulls/3/reviews/8/events', True),
                                   ('PUT', 'pulls/3/reviews/8', False), ('POST', 'pulls/3/reviews/8/comments', False),
                                   ('GET', 'pulls/3/reviews', False)]:
            self.assertEqual(upkeep.reviews(method, path.split('/')), read, path)
        for method, path, branch in [('PUT', 'contents/src/a.rs', upkeep.CONTENTS),
                                     ('DELETE', 'contents/a', upkeep.CONTENTS),
                                     ('PATCH', 'git/refs/heads/release/1', 'release/1'),
                                     ('DELETE', 'git/refs/heads/main', 'main'), ('GET', 'contents/a', None),
                                     ('POST', 'git/trees', None), ('PUT', 'pulls/1/merge', None),
                                     ('PUT', 'pulls/12/update-branch', 12)]:
            self.assertIs(upkeep.branch_written(method, path.split('/')), branch, path) if branch is None \
                or branch is upkeep.CONTENTS else self.assertEqual(upkeep.branch_written(method, path.split('/')),
                                                                   branch)
        self.assertTrue(upkeep.approves(' approve'))
        self.assertFalse(upkeep.approves('COMMENT') or upkeep.approves(None))

    def test_only_listed_workflows_may_be_started(self):
        policy = upkeep.load(self.store, common) if saved(self.store) else None
        upkeep.check_dispatch(policy, 'Example/Project', '.github/workflows/release.yml')
        for repository, path in [('example/project', '.github/workflows/deploy.yml'),
                                 ('example/other', '.github/workflows/release.yml'),
                                 ('example/project', 'release.yml'), ('example/project', None)]:
            with self.assertRaises(upkeep.Refused):
                upkeep.check_dispatch(policy, repository, path)

    def test_refusals_go_to_a_root_log_that_the_exchange_can_read_and_no_pipe_stops(self):
        folder = upkeep.log_directory(self.store)
        self.assertEqual(folder, self.root / 'workspace/.horizon-gate')
        log = upkeep.Log(folder, now=lambda: 120)
        os.umask(old := os.umask(0o077))
        try:
            log.refused('merge', 'example/project', 'x' * 400)
        finally:
            os.umask(old)
        self.assertEqual(stat.S_IMODE(folder.stat().st_mode), 0o755)
        self.assertEqual(stat.S_IMODE((folder / upkeep.LOG).stat().st_mode), 0o644)
        line = json.loads((folder / upkeep.LOG).read_text())
        self.assertEqual((line['kind'], line['repository'], len(line['reason'])), ('merge', 'example/project', 300))
        for _ in range(40):
            log.refused('api', None, 'again')
        self.assertEqual(len((folder / upkeep.LOG).read_text().splitlines()), 30)
        (folder / upkeep.LOG).unlink()
        os.mkfifo(folder / upkeep.LOG)
        upkeep.Log(folder, now=lambda: 0).refused('api', None, 'no reader')
        (folder / upkeep.LOG).unlink()
        os.symlink('/dev/null', folder / upkeep.LOG)
        upkeep.Log(folder, now=lambda: 0).refused('api', None, 'no link')
        self.assertTrue((folder / upkeep.LOG).is_symlink())


class UpkeepBrokerTests(BrokerTestCase):
    """The broker of an Upkeep cloud, in front of a fake GitHub."""

    def setUp(self):
        super().setUp()
        saved(self.store)
        self.log = upkeep.log_directory(self.store) / upkeep.LOG

    def test_refused_changes_never_reach_github_and_go_to_the_log(self):
        before = len(self.api.seen)
        for method, target in [('POST', '/repos/example/project/releases'),
                               ('PUT', '/repos/example/project/pulls/3/reviews/8/dismissals'),
                               ('PUT', '/repos/example/project/contents/.github/workflows/x.yml'),
                               ('POST', '/repos/example/project/statuses/' + HEAD),
                               ('POST', '/repos/example/project/actions/runs/1/pending_deployments')]:
            status, _, payload = self.send(method, target, body={})
            self.assertEqual(status, 403, target)
            self.assertIn('this Upkeep', json.loads(payload)['message'])
        status, _, _ = self.send('POST', '/repos/example/project/releases/1/assets?name=a', host='uploads.github.com',
                                 body=b'x')
        self.assertEqual(status, 403)
        self.assertEqual(len(self.api.seen), before)
        self.assertEqual(len(self.log.read_text().splitlines()), 6)
        # Ordinary work still reaches GitHub.
        self.assertEqual(self.send('POST', '/repos/example/project/issues/1/comments', body={'body': 'x'})[0], 200)
        self.assertEqual(self.send('POST', '/repos/example/project/actions/runs/1/rerun', body={})[0], 200)

    def test_a_dispatch_passes_only_for_a_listed_workflow(self):
        # The fake names each workflow by its request path, which is never a listed workflow file.
        status, _, payload = self.send('POST', '/repos/example/project/actions/workflows/9/dispatches',
                                       body={'ref': 'main'})
        self.assertEqual(status, 403)
        self.assertIn('listed', json.loads(payload)['message'])
        self.assertIn('"kind": "dispatch"', self.log.read_text())
        self.api.answers['/repos/example/project'] = {'default_branch': 'main'}
        target = '/repos/example/project/actions/workflows/release.yml/dispatches'
        with mock.patch.object(upkeep, 'check_dispatch') as check:
            for ref in ('main', 'refs/heads/main'):
                status, _, _ = self.send('POST', target, body={'ref': ref, 'inputs': {'a': 'b'}})
                self.assertEqual(status, 200)
                self.assertEqual(json.loads(self.api.seen[-1][-1]), {'ref': ref, 'inputs': {'a': 'b'}})
            self.assertEqual(check.call_args.args[1:],
                             ('example/project', '/repos/example/project/actions/workflows/release.yml'))
            # On another ref, the workflow file could be one that an agent pushed.
            before = len(self.api.seen)
            for body in ({'ref': 'upkeep/x'}, {'ref': 'refs/tags/main'}, {}, b'{"ref": "x", "ref": "main"}'):
                status, _, payload = self.send('POST', target, body=body)
                self.assertEqual(status, 403, body)
                self.assertIn('this Upkeep', json.loads(payload)['message'])
            self.assertFalse(any(path == target for _, _, path, *_ in self.api.seen[before:]))

    def test_the_default_branch_changes_only_through_a_pull_request(self):
        self.api.answers['/repos/example/project'] = {'default_branch': 'main'}
        # A pull request from the default branch, and one that GitHub does not show.
        self.api.answers['/repos/example/project/pulls/4'] = {
            'head': {'ref': 'main', 'repo': {'full_name': 'Example/Project'}}}
        self.api.answers['/repos/example/project/pulls/5'] = {}
        self.api.answers['/repos/example/project/pulls/6'] = {
            'head': {'ref': 'main', 'repo': {'full_name': 'fork/project'}}}
        before = len(self.api.seen)
        for method, target, body in [('PUT', '/repos/example/project/contents/src/a.rs', {'content': 'eA=='}),
                                     ('PUT', '/repos/example/project/contents/src/a.rs',
                                      {'content': 'eA==', 'branch': 'refs/heads/main'}),
                                     ('DELETE', '/repos/example/project/contents/a', {'sha': 'x', 'branch': 'main'}),
                                     ('PUT', '/repos/example/project/contents/a', {'branch': 7}),
                                     ('PATCH', '/repos/example/project/git/refs/heads/main', {'sha': HEAD}),
                                     ('PUT', '/repos/example/project/pulls/4/update-branch', {}),
                                     ('PUT', '/repos/example/project/pulls/5/update-branch', {}),
                                     ('POST', '/repos/example/project/merges', {'base': 'main', 'head': 'x'})]:
            status, _, payload = self.send(method, target, body=body)
            self.assertEqual(status, 403, (method, target, body))
            self.assertIn('this Upkeep', json.loads(payload)['message'])
        # Only the broker's own reads reach GitHub.
        self.assertEqual({method for method, *_ in self.api.seen[before:]}, {'GET'})
        # Another branch passes, with the body as sent.
        for method, target, body in [('PUT', '/repos/example/project/contents/src/a.rs',
                                      {'content': 'eA==', 'branch': 'upkeep/x'}),
                                     ('DELETE', '/repos/example/project/git/refs/heads/dependabot/cargo/x', None),
                                     ('PUT', '/repos/example/project/pulls/6/update-branch', {})]:
            status, _, _ = self.send(method, target, body=body)
            self.assertEqual(status, 200, target)
            self.assertEqual(self.api.seen[-1][2], target)
            self.assertEqual(json.loads(self.api.seen[-1][-1] or b'null'), body)
        # A repository whose default branch GitHub does not say is refused.
        self.api.answers['/repos/example/project'] = {}
        status, _, _ = self.send('PUT', '/repos/example/project/contents/a', body={'branch': 'upkeep/x'})
        self.assertEqual(status, 403)

    def test_auto_merge_is_refused_for_an_upkeep(self):
        reply = self.graphql('mutation($id: ID!) { enablePullRequestAutoMerge(input: {pullRequestId: $id}) '
                             '{ clientMutationId } }', {'id': 'PR_1'})
        self.assertIn('auto-merge', reply['errors'][0]['message'])
        self.assertFalse(any(b'enablePullRequestAutoMerge' in (data or b'') for *_, data in self.api.seen))

    def test_an_agent_cannot_approve_a_pull_request(self):
        before = len(self.api.seen)
        for target, body in [('/repos/example/project/pulls/3/reviews', {'event': 'APPROVE'}),
                             ('/repos/example/project/pulls/3/reviews/8/events', {'event': 'approve'}),
                             ('/repos/example/project/pulls/3/reviews', b'{"event": "COMMENT", "event": "APPROVE"}'),
                             ('/repos/example/project/pulls/3/reviews?event=APPROVE', {'body': 'x'})]:
            status, _, payload = self.send('POST', target, body=body)
            self.assertEqual(status, 403, target)
            self.assertIn('this Upkeep', json.loads(payload)['message'])
        self.assertEqual(len(self.api.seen), before)
        self.assertEqual(self.log.read_text().count('"kind": "review"'), 4)
        reply = self.graphql('mutation { addPullRequestReview(input: {pullRequestId: "PR_1", event: APPROVE}) '
                             '{ clientMutationId } }')
        self.assertIn('approve', reply['errors'][0]['message'])
        self.assertFalse(any(b'addPullRequestReview' in (data or b'') for *_, data in self.api.seen))
        # A review that does not approve reaches GitHub with its body as sent.
        status, _, _ = self.send('POST', '/repos/example/project/pulls/3/reviews',
                                 body={'event': 'REQUEST_CHANGES', 'body': 'x'})
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(self.api.seen[-1][-1]), {'event': 'REQUEST_CHANGES', 'body': 'x'})


if __name__ == '__main__':
    unittest.main()
