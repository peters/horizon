"""Session relaunch after a container reset, against real Git and a recording tmux stub."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).parent
SESSION = 'panel-1'
OPERATION = '6f1c2d3e-4a5b-4c6d-8e7f-001122334455'

# Records each launch and treats a recorded session as running until the test "resets" it.
TMUX = '''import json, os, sys
from pathlib import Path
state = Path(os.environ['TMUX_STATE'])
args = sys.argv[1:]
if args[:2] != ['-L', 'horizon-cloud']:
    sys.exit(90)
command, rest = args[2], args[3:]
if command == 'has-session':
    sys.exit(0 if (state / 'running' / rest[1].removeprefix('=')).exists() else 1)
if command == 'new-session':
    if len(rest) != 6 or rest[0:2] != ['-d', '-s'] or rest[3] != '-c' or (state / 'running' / rest[2]).exists():
        sys.exit(91)
    with open(state / 'launches', 'a') as log:
        log.write(json.dumps({'session': rest[2], 'cwd': rest[4], 'command': rest[5]}) + '\\n')
    (state / 'running').mkdir(exist_ok=True)
    (state / 'running' / rest[2]).touch()
    sys.exit(0)
if command == 'attach-session':
    sys.exit(0)
sys.exit(92)
'''
CHECK = '''import os, sys
with open(os.environ['CHECK_LOG'], 'a') as log:
    log.write(' '.join(sys.argv[1:]) + '\\n')
if sys.argv[1:] == ['--ready']:
    sys.exit(1 if os.environ.get('CHECK_READY_FAIL') == '1' else 0)
if sys.argv[1] == '--agent':
    sys.exit(1 if sys.argv[2] == 'grok' else 0)
sys.exit(93)
'''
SOURCE = '''import os, sys
with open(os.environ['SOURCE_LOG'], 'a') as log:
    log.write(' '.join(sys.argv[1:]) + '\\n')
'''


@unittest.skipUnless(shutil.which('flock') and shutil.which('git'), 'worker scripts need util-linux flock and Git')
class SessionRelaunchTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.workspace = self.root / 'workspace'
        self.state = self.root / 'tmux'
        self.state.mkdir()
        tools = self.root / 'bin'
        tools.mkdir()
        for name, body in [('tmux', TMUX), ('horizon-worker-check', CHECK), ('horizon-worker-source', SOURCE)]:
            (tools / name).write_text('#!' + sys.executable + '\n' + body)
            (tools / name).chmod(0o700)
        self.script = self.root / 'horizon-worker-session'
        self.script.write_text((SCRIPTS / 'horizon-worker-session').read_text().replace('/workspace', str(self.workspace)))
        self.env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ['PATH'],
                        TMUX_STATE=str(self.state), CHECK_LOG=str(self.root / 'check.log'),
                        SOURCE_LOG=str(self.root / 'source.log'), GIT_CONFIG_NOSYSTEM='1')
        self.env.pop('CHECK_READY_FAIL', None)
        local = self.root / 'local'
        self.git('init', '--quiet', local)
        (local / 'file.txt').write_text('committed\n')
        self.git('-C', local, 'add', 'file.txt')
        self.commit(local, 'Base')
        self.revision = self.git('-C', local, 'rev-parse', 'HEAD')
        self.workspace.mkdir()
        self.git('clone', '--quiet', '--bare', local, self.workspace / 'repository.git')
        self.worktree = self.workspace / 'agents' / SESSION
        self.session = self.workspace / 'sessions' / SESSION

    def git(self, *args):
        return subprocess.run(['git', *map(str, args)], check=True, capture_output=True, text=True,
                              env=dict(os.environ, GIT_CONFIG_NOSYSTEM='1')).stdout.strip()

    def commit(self, repository, message):
        self.git('-C', repository, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
                 'commit', '--quiet', '-m', message)

    def run_session(self, *args, agent='codex'):
        return subprocess.run(['bash', str(self.script), *args, SESSION, agent, self.revision],
                              env=self.env, capture_output=True, text=True)

    def relaunch(self, operation=OPERATION, **kwargs):
        return self.run_session('--relaunch', operation, **kwargs)

    def launches(self):
        log = self.state / 'launches'
        return [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []

    def reset_container(self):
        shutil.rmtree(self.state / 'running', ignore_errors=True)

    def launch_with_agent_work(self):
        attached = self.run_session()
        self.assertEqual(attached.returncode, 0, attached.stderr)
        (self.worktree / 'agent.txt').write_text('agent commit\n')
        self.git('-C', self.worktree, 'add', 'agent.txt')
        self.commit(self.worktree, 'Agent work')
        (self.worktree / 'file.txt').write_text('uncommitted agent edit\n')
        (self.worktree / 'scratch.txt').write_text('untracked agent file\n')
        return self.git('-C', self.worktree, 'rev-parse', 'HEAD')

    def assert_worktree_intact(self, head):
        self.assertEqual(self.git('-C', self.worktree, 'rev-parse', 'HEAD'), head)
        self.assertEqual((self.worktree / 'file.txt').read_text(), 'uncommitted agent edit\n')
        self.assertEqual((self.worktree / 'scratch.txt').read_text(), 'untracked agent file\n')

    def test_relaunch_after_reset_starts_one_process_in_the_existing_worktree(self):
        head = self.launch_with_agent_work()
        original = self.launches()
        self.assertEqual(len(original), 1)
        (self.session / 'exit-status').write_text('0\n')
        self.reset_container()
        lost = self.run_session()
        self.assertEqual(lost.returncode, 4)
        self.assertEqual(len(self.launches()), 1)

        relaunched = self.relaunch()
        self.assertEqual(relaunched.returncode, 0, relaunched.stderr)
        launches = self.launches()
        self.assertEqual(launches, original * 2)
        self.assertEqual(launches[1]['cwd'], str(self.worktree))
        self.assertEqual(launches[1]['command'], 'horizon-worker-run ' + SESSION + ' codex')
        self.assert_worktree_intact(head)
        self.assertEqual(self.git('-C', self.worktree, 'branch', '--show-current'), 'agent/' + SESSION)
        self.assertEqual((self.root / 'source.log').read_text().splitlines(), ['checkout ' + str(self.worktree)])
        self.assertTrue((self.session / ('relaunch-requested-' + OPERATION)).exists())
        self.assertFalse((self.session / 'exit-status').exists())
        self.assertIn('--ready', (self.root / 'check.log').read_text().splitlines())

        attached = self.run_session()
        self.assertEqual(attached.returncode, 0, attached.stderr)
        self.assertEqual(len(self.launches()), 2)
        self.assert_worktree_intact(head)

    def test_relaunch_is_idempotent_per_operation_and_never_replays_a_lost_relaunch(self):
        head = self.launch_with_agent_work()
        self.reset_container()
        self.assertEqual(self.relaunch().returncode, 0)
        retried = self.relaunch()
        self.assertEqual(retried.returncode, 0, retried.stderr)
        self.assertEqual(len(self.launches()), 2)

        self.reset_container()
        replay = self.relaunch()
        self.assertEqual(replay.returncode, 4)
        self.assertIn('No replacement process was started', replay.stderr)
        self.assertEqual(len(self.launches()), 2)

        following = self.relaunch('next-operation')
        self.assertEqual(following.returncode, 0, following.stderr)
        self.assertEqual(len(self.launches()), 3)
        self.assert_worktree_intact(head)

        # A delayed retry of the first operation sees a running process it did not start.
        late = self.relaunch()
        self.assertEqual(late.returncode, 4, late.stderr)
        self.assertEqual(len(self.launches()), 3)
        self.assertEqual(self.relaunch('next-operation').returncode, 0)

    def test_running_session_is_refused_without_a_fence(self):
        self.launch_with_agent_work()
        refused = self.relaunch()
        self.assertEqual(refused.returncode, 5)
        self.assertIn('Session process is running', refused.stderr)
        self.assertEqual(len(self.launches()), 1)
        self.assertFalse((self.session / ('relaunch-requested-' + OPERATION)).exists())

    def test_missing_worktree_is_refused_without_recreating_it(self):
        self.launch_with_agent_work()
        self.reset_container()
        shutil.rmtree(self.worktree)
        refused = self.relaunch()
        self.assertEqual(refused.returncode, 3)
        self.assertIn('worktree is missing', refused.stderr)
        self.assertFalse(self.worktree.exists())
        self.assertEqual(len(self.launches()), 1)
        self.assertFalse((self.session / ('relaunch-requested-' + OPERATION)).exists())

    def test_unknown_session_is_refused_without_writing_state(self):
        refused = self.relaunch()
        self.assertEqual(refused.returncode, 3)
        self.assertIn('Unknown session', refused.stderr)
        self.assertFalse((self.workspace / 'sessions').exists())
        self.assertFalse((self.workspace / 'agents').exists())
        self.assertEqual(self.launches(), [])

    def test_binding_mismatch_and_disabled_agent_are_refused(self):
        self.launch_with_agent_work()
        self.reset_container()
        mismatch = self.relaunch(agent='claude')
        self.assertEqual(mismatch.returncode, 3)
        self.assertIn('Session binding mismatch', mismatch.stderr)
        self.assertNotEqual(self.relaunch(agent='grok').returncode, 0)
        self.assertEqual(len(self.launches()), 1)

    def test_unready_services_block_relaunch_before_any_state_changes(self):
        self.launch_with_agent_work()
        self.reset_container()
        self.env['CHECK_READY_FAIL'] = '1'
        blocked = self.relaunch()
        self.assertNotEqual(blocked.returncode, 0)
        self.assertEqual(len(self.launches()), 1)
        self.assertFalse((self.session / ('relaunch-requested-' + OPERATION)).exists())
        del self.env['CHECK_READY_FAIL']
        self.assertEqual(self.relaunch().returncode, 0)

    def test_never_launched_session_is_left_for_attach(self):
        (self.session).mkdir(parents=True)
        (self.session / 'agent').write_text('codex\n')
        (self.session / 'revision').write_text(self.revision + '\n')
        (self.session / 'preparing').touch()
        skipped = self.relaunch()
        self.assertEqual(skipped.returncode, 5)
        self.assertIn('never launched', skipped.stderr)
        self.assertEqual(self.launches(), [])
        self.assertEqual(self.run_session().returncode, 0)
        self.assertEqual(len(self.launches()), 1)

    def test_invalid_operation_is_rejected_before_any_check(self):
        self.launch_with_agent_work()
        self.reset_container()
        checks = (self.root / 'check.log').read_text()
        for operation in ['', 'bad/operation', '../escape', 'x' * 101]:
            self.assertNotEqual(self.relaunch(operation).returncode, 0, operation)
        self.assertEqual((self.root / 'check.log').read_text(), checks)
        self.assertEqual(len(self.launches()), 1)
        self.assertEqual([path.name for path in self.session.iterdir() if path.name.startswith('relaunch')], [])


if __name__ == '__main__':
    unittest.main()
