"""The Upkeep checks of the worker's Git proxy: a push names branches only, and never the default
branch. A loopback Git server and synthetic tokens only."""
import os
import shutil
import unittest
from unittest import mock

from test_github_git_proxy import ProxyTestCase, git
from test_github_upkeep_policy import saved
from test_horizon_worker_github import service

upkeep = service.upkeep
push = service.gitproxy.checks
OLD, NEW = '0' * 40, 'b' * 40


def line(text):
    data = text.encode() if isinstance(text, str) else text
    return b'%04x' % (len(data) + 4) + data


class CommandTests(unittest.TestCase):
    def test_the_commands_name_their_refs_and_wait_for_their_end(self):
        request = line('%s %s refs/heads/topic\0report-status push-options' % (OLD, NEW)) + b'0000' \
            + line('ci.skip') + b'0000PACK'
        self.assertEqual(push.refs(request), ['refs/heads/topic'])
        for part in (request[:2], request[:30], line('%s %s refs/heads/x\0report-status' % (OLD, NEW))):
            with self.assertRaises(push.Incomplete):
                push.refs(part)
        for broken in (b'zzzz', b'+0x5', b'00 4', line('%s refs/heads/x' % NEW) + b'0000', b'0002'):
            with self.assertRaises(upkeep.Refused) as raised:
                push.refs(broken)
            self.assertNotIsInstance(raised.exception, push.Incomplete)
        # A shallow clone names its shallow commits first; the report names only the refs.
        shallow = line('shallow %s' % ('c' * 40)) + line('%s %s refs/heads/x\0report-status' % (OLD, NEW)) + b'0000'
        self.assertEqual(push.refs(shallow), ['refs/heads/x'])
        report = push.push_report(shallow, 'no')
        self.assertIn(b'ng refs/heads/x no', report)
        self.assertNotIn(b'ng ' + b'c' * 40, report)
        self.assertIsNone(push.push_report(line('%s %s refs/heads/x' % (OLD, NEW)) + b'0000', 'no'))

    def test_only_branches_other_than_the_default_pass(self):
        push.check_refs(['refs/heads/upkeep/x', 'refs/heads/main-2', 'refs/heads/dependabot/cargo/x'], 'main')
        for ref in ('refs/heads/main', 'refs/tags/v1', 'refs/notes/x', 'HEAD', 'refs/heads/'):
            with self.assertRaises(upkeep.Refused, msg=ref):
                push.check_refs(['refs/heads/ok', ref], 'main')


@unittest.skipUnless(shutil.which('git'), 'needs Git')
class UpkeepPushTests(ProxyTestCase):
    """A real Git push through the proxy of an Upkeep cloud."""

    def setUp(self):
        super().setUp()
        saved(self.store)
        branch = mock.patch.object(push, 'default_branch', return_value='main')
        self.default_branch = branch.start()
        self.addCleanup(branch.stop)

    def test_a_branch_passes_and_the_default_branch_and_tags_are_refused(self):
        env = self.routed()
        result, project = self.clone('example/project', env)
        self.assertEqual(result.returncode, 0, result.stderr)
        bare = '--git-dir=' + str(self.repositories / 'example/project.git')
        main = git(bare, 'rev-parse', 'main', env=env).stdout.strip()
        # A push larger than one piece of the body, to a new branch.
        (project / 'large').write_bytes(os.urandom(3 << 20))
        git('add', 'large', cwd=project, env=env)
        git('commit', '-q', '-m', 'change', cwd=project, env=env)
        result = git('push', '-q', 'origin', 'HEAD:refs/heads/upkeep/x', cwd=project, env=env, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.default_branch.call_args.args[2], 'example/project')
        result = git('push', '-q', 'origin', 'HEAD:main', cwd=project, env=env, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Horizon: this Upkeep changes the default branch main only through a merged pull request',
                      result.stderr)
        git('tag', 'v1', cwd=project, env=env)
        result = git('push', '-q', 'origin', 'v1', cwd=project, env=env, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(git(bare, 'rev-parse', 'main', env=env).stdout.strip(), main)
        self.assertEqual(git(bare, 'tag', env=env).stdout.strip(), '')
        log = (upkeep.log_directory(self.store) / upkeep.LOG).read_text()
        self.assertIn('refs/tags/v1', log)
        self.assertEqual(log.count('"kind": "push"'), 2)


if __name__ == '__main__':
    unittest.main()
