"""New cloud panels share one checkout without resetting existing work."""
import shutil
import subprocess
import unittest

import test_session_relaunch as fixtures


@unittest.skipUnless(shutil.which('flock') and shutil.which('git'), 'worker scripts need flock and Git')
class SharedCheckoutTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.SessionRelaunchTests(methodName='runTest')
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.checkout = self.fixture.workspace / 'checkout'

    def command(self, identity, agent='shell', revision=None, relaunch=None):
        f = self.fixture
        args = ['bash', str(f.script), '--shared']
        if relaunch:
            args += ['--relaunch', relaunch]
        return args + [identity, agent, revision or f.revision]

    def start(self, identity, **kwargs):
        return subprocess.run(self.command(identity, **kwargs), env=self.fixture.env,
                              capture_output=True, text=True, timeout=30)

    def assert_started(self, identity, **kwargs):
        result = self.start(identity, **kwargs)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_new_shell_and_agent_share_files_branch_and_one_checkout(self):
        f = self.fixture
        self.assert_started('shell-one')
        f.git('-C', self.checkout, 'switch', '-c', 'manual-work')
        (self.checkout / 'file.txt').write_text('uncommitted edit\n')
        (self.checkout / 'scratch.txt').write_text('untracked work\n')
        source_calls = (f.root / 'source.log').read_text()
        self.assert_started('agent-two', agent='claude')
        self.assert_started('shell-three')
        self.assertEqual([entry['cwd'] for entry in f.launches()], [str(self.checkout)] * 3)
        self.assertEqual((self.checkout / 'file.txt').read_text(), 'uncommitted edit\n')
        self.assertEqual((self.checkout / 'scratch.txt').read_text(), 'untracked work\n')
        self.assertEqual(f.git('-C', self.checkout, 'branch', '--show-current'), 'manual-work')
        self.assertEqual((f.root / 'source.log').read_text(), source_calls)
        self.assertEqual(f.git('--git-dir', f.workspace / 'repository.git', 'for-each-ref', 'refs/heads/agent'), '')
        self.assertEqual(len(list((f.workspace / 'repository.git/worktrees').iterdir())), 1)
        self.assertFalse((f.workspace / 'agents').exists())
        self.assertTrue((f.workspace / 'session-data/shell-one').is_dir())
        self.assertTrue((f.workspace / 'session-data/agent-two').is_dir())

    def test_simultaneous_first_panels_prepare_only_once(self):
        f = self.fixture
        children = [subprocess.Popen(self.command(identity), env=f.env, stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, text=True) for identity in ('one', 'two')]
        for child in children:
            _, error = child.communicate(timeout=30)
            self.assertEqual(child.returncode, 0, error)
        self.assertEqual([entry['cwd'] for entry in f.launches()], [str(self.checkout)] * 2)
        self.assertEqual(len((f.root / 'source.log').read_text().splitlines()), 1)
        self.assertEqual(len(list((f.workspace / 'repository.git/worktrees').iterdir())), 1)

    def test_relaunch_preserves_manual_commits_and_uncommitted_work(self):
        f = self.fixture
        self.assert_started('one')
        f.git('-C', self.checkout, 'switch', '-c', 'manual-work')
        (self.checkout / 'commit.txt').write_text('new commit\n')
        f.git('-C', self.checkout, 'add', 'commit.txt')
        f.commit(self.checkout, 'Manual work')
        head = f.git('-C', self.checkout, 'rev-parse', 'HEAD')
        (self.checkout / 'file.txt').write_text('dirty\n')
        f.reset_container()
        self.assert_started('one', relaunch=fixtures.OPERATION)
        self.assert_started('two')
        self.assertEqual(f.git('-C', self.checkout, 'rev-parse', 'HEAD'), head)
        self.assertEqual((self.checkout / 'file.txt').read_text(), 'dirty\n')
        self.assertEqual([entry['cwd'] for entry in f.launches()], [str(self.checkout)] * 3)

    def test_legacy_sessions_keep_their_existing_worktrees(self):
        f = self.fixture
        head = f.launch_with_agent_work()
        self.assert_started('new-panel')
        f.assert_worktree_intact(head)
        f.reset_container()
        self.assertEqual(f.relaunch().returncode, 0)
        f.assert_worktree_intact(head)
        self.assertEqual([entry['cwd'] for entry in f.launches()],
                         [str(f.worktree), str(self.checkout), str(f.worktree)])

    def test_failed_first_checkout_is_not_reset_or_exposed(self):
        f = self.fixture
        source = f.tools / 'horizon-worker-source'
        source.write_text('#!/bin/sh\nexit 9\n')
        self.assertNotEqual(self.start('one').returncode, 0)
        (self.checkout / 'file.txt').write_text('recovery work\n')
        result = self.start('two')
        self.assertEqual(result.returncode, 3)
        self.assertIn('preparation failed', result.stderr)
        self.assertEqual((self.checkout / 'file.txt').read_text(), 'recovery work\n')
        self.assertEqual(f.launches(), [])

    def test_missing_checkout_and_changed_revision_are_refused_without_recreation(self):
        f = self.fixture
        self.assert_started('one')
        self.assertEqual(self.start('wrong-revision', revision='b' * 40).returncode, 3)
        shutil.rmtree(self.checkout)
        self.assertEqual(self.start('two').returncode, 3)
        self.assertFalse(self.checkout.exists())
        self.assertEqual(len(f.launches()), 1)

    def test_siblings_are_hydrated_once_and_shared_beside_the_primary(self):
        f = self.fixture
        f.install('horizon-worker-source')
        (f.workspace / 'source').mkdir()
        (f.workspace / 'source/manifest.json').write_text('{"modules":[],"assets":[]}')
        revision = f.add_sibling('lib', {'library.txt': 'library\n'})
        f.set_siblings([('lib', 'native-lib', revision)])
        self.assert_started('one')
        library = self.checkout / 'native-lib/library.txt'
        library.write_text('edited library\n')
        self.assert_started('two', agent='claude')
        self.assertEqual(library.read_text(), 'edited library\n')
        self.assertEqual([entry['cwd'] for entry in f.launches()], [str(self.checkout / 'app')] * 2)
        f.set_siblings([('lib', 'renamed-library', revision)])
        self.assertEqual(self.start('changed-layout').returncode, 3)
        self.assertFalse((self.checkout / 'renamed-library').exists())


if __name__ == '__main__':
    unittest.main()
