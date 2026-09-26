"""Session relaunch after a container reset, against real Git and a recording tmux stub."""
import hashlib
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
    try:
        os.fstat(9)
        inherits_lock = True
    except OSError:
        inherits_lock = False
    with open(state / 'launches', 'a') as log:
        log.write(json.dumps({'session': rest[2], 'cwd': rest[4], 'command': rest[5],
                              'inherits_lock': inherits_lock}) + '\\n')
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
        self.tools = tools
        for name, body in [('tmux', TMUX), ('horizon-worker-check', CHECK), ('horizon-worker-source', SOURCE)]:
            (tools / name).write_text('#!' + sys.executable + '\n' + body)
            (tools / name).chmod(0o700)
        self.install('horizon-worker-siblings')
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

    def install(self, name):
        tool = self.tools / name
        tool.write_text('#!' + sys.executable + '\n' + (SCRIPTS / name).read_text().replace('/workspace', str(self.workspace)))
        tool.chmod(0o700)

    def git(self, *args, env=None):
        return subprocess.run(['git', *map(str, args)], check=True, capture_output=True, text=True,
                              env=env or dict(os.environ, GIT_CONFIG_NOSYSTEM='1')).stdout.strip()

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
        launches = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
        # The tmux server never holds the session lock, so a crash cannot leave it locked.
        self.assertFalse(any(launch.pop('inherits_lock') for launch in launches))
        return launches

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


    def add_sibling(self, alias, files):
        local = self.root / ('local-' + alias)
        self.git('init', '--quiet', local)
        for name, content in files.items():
            (local / name).write_text(content)
        self.git('-C', local, 'add', '.')
        self.commit(local, 'Sibling base')
        revision = self.git('-C', local, 'rev-parse', 'HEAD')
        root = self.workspace / 'siblings' / alias
        (root / 'source').mkdir(parents=True)
        (root / 'source' / 'manifest.json').write_text('{"modules":[],"assets":[]}')
        pack = self.root / 'transfer.pack'
        pack.write_bytes(subprocess.run(['git', '-C', local, 'pack-objects', '--stdout', '--revs'], input=(revision + '\n').encode(),
                                        check=True, capture_output=True).stdout)
        importer = self.root / 'horizon-worker-import'
        importer.write_text((SCRIPTS / 'horizon-worker-import').read_text().replace('/workspace', str(self.workspace)))
        shutil.copyfile(pack, root / 'horizon-transfer.pack')
        subprocess.run(['bash', importer, revision, '--sibling', alias], check=True, capture_output=True, env=self.env)
        return revision

    def set_siblings(self, siblings, primary='app'):
        manifest = {'version': 1, 'primary': primary,
                    'siblings': [{'alias': alias, 'directory': directory, 'revision': revision}
                                 for alias, directory, revision in siblings]}
        result = subprocess.run([self.tools / 'horizon-worker-siblings', 'set'], input=json.dumps(manifest),
                                env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def launch_beside_sibling(self):
        library = self.add_sibling('lib', {'library.txt': 'library base\n'})
        self.set_siblings([('lib', 'native-lib', library)])
        root = self.worktree
        self.worktree = root / 'app'
        head = self.launch_with_agent_work()
        sibling = root / 'native-lib'
        (sibling / 'library.txt').write_text('uncommitted library edit\n')
        return library, head, sibling

    def test_sibling_layout_checks_out_each_repository_beside_the_primary(self):
        library, _, sibling = self.launch_beside_sibling()
        [launch] = self.launches()
        self.assertEqual(launch['cwd'], str(self.worktree))
        self.assertEqual(self.git('-C', self.worktree, 'branch', '--show-current'), 'agent/' + SESSION)
        self.assertEqual(self.git('-C', sibling, 'branch', '--show-current'), 'agent/' + SESSION)
        self.assertEqual(self.git('-C', sibling, 'rev-parse', 'HEAD'), library)
        self.assertEqual(self.git('-C', sibling, 'rev-parse', '--git-common-dir'),
                         str(self.workspace / 'siblings/lib/repository.git'))
        self.assertEqual(sorted(path.name for path in self.worktree.parent.iterdir()), ['app', 'native-lib'])
        self.assertEqual((self.root / 'source.log').read_text().splitlines(),
                         ['checkout ' + str(self.worktree), 'checkout ' + str(sibling) + ' --sibling lib'])
        self.assertEqual(json.loads((self.session / 'siblings.json').read_text())['siblings'][0]['directory'], 'native-lib')
        data = self.workspace / 'session-data' / SESSION
        self.assertEqual(data.stat().st_mode & 0o777, 0o700)
        self.assertEqual(data.parent.stat().st_mode & 0o777, 0o700)

    def test_relaunch_keeps_the_sibling_layout_after_the_manifest_changes(self):
        _, head, sibling = self.launch_beside_sibling()
        self.set_siblings([], primary='elsewhere')
        self.reset_container()
        relaunched = self.relaunch()
        self.assertEqual(relaunched.returncode, 0, relaunched.stderr)
        self.assertEqual([launch['cwd'] for launch in self.launches()], [str(self.worktree)] * 2)
        self.assert_worktree_intact(head)
        self.assertEqual((sibling / 'library.txt').read_text(), 'uncommitted library edit\n')
        self.assertEqual(len((self.root / 'source.log').read_text().splitlines()), 2)
        self.assertEqual(self.run_session().returncode, 0)
        self.assertEqual(len(self.launches()), 2)

    def test_single_layout_session_never_gains_siblings(self):
        head = self.launch_with_agent_work()
        self.assertFalse((self.session / 'siblings.json').exists())
        self.set_siblings([('lib', 'native-lib', self.add_sibling('lib', {'library.txt': 'library\n'}))])
        self.reset_container()
        relaunched = self.relaunch()
        self.assertEqual(relaunched.returncode, 0, relaunched.stderr)
        self.assertEqual([launch['cwd'] for launch in self.launches()], [str(self.worktree)] * 2)
        self.assert_worktree_intact(head)
        self.assertFalse((self.worktree / 'native-lib').exists())

    def test_manifest_without_siblings_keeps_the_single_layout(self):
        self.set_siblings([])
        self.assertEqual(self.run_session().returncode, 0)
        self.assertEqual([launch['cwd'] for launch in self.launches()], [str(self.worktree)])
        self.assertEqual(self.git('-C', self.worktree, 'rev-parse', '--show-toplevel'), str(self.worktree))
        self.assertFalse((self.session / 'siblings.json').exists())

    def test_missing_sibling_worktree_is_refused_without_recreating_it(self):
        _, _, sibling = self.launch_beside_sibling()
        self.reset_container()
        shutil.rmtree(sibling)
        refused = self.relaunch()
        self.assertEqual(refused.returncode, 3)
        self.assertIn('worktree is missing', refused.stderr)
        self.assertFalse(sibling.exists())
        self.assertEqual(len(self.launches()), 1)
        self.assertFalse((self.session / ('relaunch-requested-' + OPERATION)).exists())

    def test_relaunch_refuses_another_repository_at_a_worktree_path(self):
        _, _, sibling = self.launch_beside_sibling()
        self.reset_container()
        for replaced in [sibling, self.worktree]:
            moved = replaced.with_name(replaced.name + '.moved')
            replaced.rename(moved)
            self.git('init', '--quiet', replaced)
            refused = self.relaunch()
            self.assertEqual(refused.returncode, 3, replaced)
            self.assertIn('worktree is missing', refused.stderr)
            shutil.rmtree(replaced)
            moved.rename(replaced)
        self.assertEqual(len(self.launches()), 1)
        self.assertFalse((self.session / ('relaunch-requested-' + OPERATION)).exists())
        self.assertEqual(self.relaunch().returncode, 0)

    def test_interrupted_preparation_completes_the_sibling_on_the_next_attach(self):
        library = self.add_sibling('lib', {'library.txt': 'library base\n'})
        self.set_siblings([('lib', 'native-lib', library)])
        # The first attach fails after the primary worktree, before the sibling's.
        (self.tools / 'horizon-worker-source').write_text('#!/bin/sh\nexit 1\n')
        self.assertNotEqual(self.run_session().returncode, 0)
        self.assertTrue((self.worktree / 'app' / '.git').exists())
        self.assertFalse((self.worktree / 'native-lib').exists())
        self.assertEqual(self.launches(), [])
        (self.tools / 'horizon-worker-source').write_text('#!' + sys.executable + '\n' + SOURCE)
        attached = self.run_session()
        self.assertEqual(attached.returncode, 0, attached.stderr)
        self.assertEqual(self.git('-C', self.worktree / 'native-lib', 'rev-parse', 'HEAD'), library)
        self.assertEqual([launch['cwd'] for launch in self.launches()], [str(self.worktree / 'app')])

    def test_corrupt_session_snapshot_refuses_attach_and_relaunch(self):
        self.launch_beside_sibling()
        self.reset_container()
        (self.session / 'siblings.json').write_text('{"version":1,"primary":"../escape","siblings":[]}')
        self.assertNotEqual(self.relaunch().returncode, 0)
        (self.session / 'siblings.json').write_text('{"version":1,')
        self.assertNotEqual(self.relaunch().returncode, 0)
        self.assertEqual(len(self.launches()), 1)
        self.assertFalse((self.session / ('relaunch-requested-' + OPERATION)).exists())

    def test_relaunch_creates_the_session_data_directory_of_an_older_session(self):
        self.launch_with_agent_work()
        data = self.workspace / 'session-data' / SESSION
        shutil.rmtree(self.workspace / 'session-data')
        self.reset_container()
        self.assertEqual(self.relaunch().returncode, 0)
        self.assertEqual(data.stat().st_mode & 0o777, 0o700)
        self.assertEqual(list(data.iterdir()), [])

    @unittest.skipUnless(shutil.which('git-lfs'), 'sibling LFS hydration needs git-lfs')
    def test_sibling_session_hydrates_submodule_and_lfs_from_the_sibling_material(self):
        content = b'native library binary\n'
        oid = hashlib.sha256(content).hexdigest()
        pointer = f'version https://git-lfs.github.com/spec/v1\noid sha256:{oid}\nsize {len(content)}\n'
        clean = self.root / 'clean.gitconfig'
        clean.touch()
        plain = dict(os.environ, GIT_CONFIG_GLOBAL=str(clean), GIT_CONFIG_NOSYSTEM='1')
        module = self.root / 'module'
        self.git('init', '--quiet', module, env=plain)
        (module / '.gitattributes').write_text('blob.bin filter=lfs -text\n')
        (module / 'blob.bin').write_text(pointer)
        self.git('-C', module, 'add', '.', env=plain)
        self.git('-C', module, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '--quiet', '-m', 'Module', env=plain)
        module_revision = self.git('-C', module, 'rev-parse', 'HEAD', env=plain)
        # The worker's global configuration names the primary's store, which holds nothing.
        config = self.root / 'worker.gitconfig'
        config.write_text('[filter "lfs"]\n\tsmudge = git-lfs smudge -- %f\n\tprocess = git-lfs filter-process\n'
                          f'\trequired = true\n[lfs]\n\tstorage = {self.workspace}/source/lfs\n')
        self.env.update(GIT_CONFIG_GLOBAL=str(config))
        self.install('horizon-worker-source')
        (self.workspace / 'source').mkdir()
        (self.workspace / 'source' / 'manifest.json').write_text('{"modules":[],"assets":[]}')
        library = self.add_sibling('lib', {'.gitattributes': 'blob.bin filter=lfs -text\n', 'blob.bin': pointer})
        archive = self.root / 'material'
        (archive / 'lfs').mkdir(parents=True)
        (archive / 'lfs' / oid).write_bytes(content)
        (archive / 'module-0.pack').write_bytes(subprocess.run(
            ['git', '-C', module, 'pack-objects', '--stdout', '--revs'], input=(module_revision + '\n').encode(),
            check=True, capture_output=True, env=plain).stdout)
        (archive / 'manifest.json').write_text(json.dumps({
            'modules': [{'path': 'vendor/module', 'revision': module_revision}],
            'assets': [{'path': 'blob.bin', 'oid': oid, 'size': len(content)}]}))
        shutil.rmtree(self.workspace / 'siblings/lib/source')
        subprocess.run(['tar', '-cf', self.workspace / 'siblings/lib/horizon-source.tar', '-C', archive, '.'], check=True)
        subprocess.run([self.tools / 'horizon-worker-source', 'import', '--sibling', 'lib'], check=True, capture_output=True, env=self.env)
        self.set_siblings([('lib', 'native-lib', library)])
        attached = self.run_session()
        self.assertEqual(attached.returncode, 0, attached.stderr)
        sibling = self.worktree / 'native-lib'
        self.assertEqual((sibling / 'blob.bin').read_bytes(), content)
        self.assertEqual((sibling / 'vendor/module/blob.bin').read_bytes(), content)
        self.assertEqual([launch['cwd'] for launch in self.launches()], [str(self.worktree / 'app')])
        self.assertFalse((self.workspace / 'source' / 'lfs').exists())


if __name__ == '__main__':
    unittest.main()
