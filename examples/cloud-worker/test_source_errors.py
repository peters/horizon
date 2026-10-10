"""Expected source errors exit without calling the desktop crash hook."""
import contextlib
import io
import json
from pathlib import Path
import runpy
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock

SOURCE = Path(__file__).with_name('horizon-worker-source')


class SourceErrorTests(unittest.TestCase):
    def execute(self, root, workspace, *arguments, legacy_symlink_loop=False):
        script = root / 'horizon-worker-source'
        script.write_text(SOURCE.read_text().replace('/workspace', str(workspace)))
        runner = root / 'runner.py'
        # Disable site hooks, then install a spy instead of the real crash reporter.
        # A regression must fail the test without opening another desktop dialog.
        legacy = ("from pathlib import Path\n"
                  "def legacy_resolve(path):\n"
                  "    raise RuntimeError('Symlink loop from ' + str(path))\n"
                  "Path.resolve = legacy_resolve\n") if legacy_symlink_loop else ''
        runner.write_text("import runpy, sys\n"
                          "def crash_hook(kind, error, trace):\n"
                          "    print('CRASH_HOOK_CALLED: ' + str(error), file=sys.stderr)\n"
                          "sys.excepthook = crash_hook\n"
                          "sys.argv.pop(0)\n" + legacy +
                          "runpy.run_path(sys.argv[0], run_name='__main__')\n")
        return subprocess.run([sys.executable, '-S', str(runner), str(script), *arguments],
                              capture_output=True, timeout=30)

    def archive(self, workspace, manifest, pack=None):
        with tarfile.open(workspace / 'horizon-source.tar', 'w') as archive:
            files = {'manifest.json': manifest}
            if pack is not None:
                files['module-0.pack'] = pack
            for name, content in files.items():
                member = tarfile.TarInfo(name)
                member.size = len(content)
                archive.addfile(member, io.BytesIO(content))

    def test_request_and_transfer_failures_do_not_call_the_crash_hook(self):
        cases = ['usage', 'worktree', 'dependencies', 'missing-upload', 'archive', 'json', 'json-encoding', 'json-depth', 'git', 'lock']
        for case in cases:
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                workspace = root / 'workspace'
                if case != 'lock':
                    workspace.mkdir()
                arguments, reason = ['import'], b'No such file or directory'
                if case == 'usage':
                    arguments, reason = [], b'Usage:'
                elif case == 'worktree':
                    arguments, reason = ['checkout', str(root / 'outside')], b'Invalid agent worktree'
                elif case == 'dependencies':
                    arguments = ['checkout', str(workspace / 'agents' / 'session')]
                    reason = b'Source dependencies have not been imported'
                elif case == 'archive':
                    (workspace / 'horizon-source.tar').write_bytes(b'not an archive')
                    reason = b'could not be opened'
                elif case == 'json':
                    self.archive(workspace, b'{')
                    reason = b'Expecting property name'
                elif case == 'json-encoding':
                    self.archive(workspace, b'\xff')
                    reason = b'decode'
                elif case == 'json-depth':
                    self.archive(workspace, b'[' * 1100 + b']' * 1100)
                    reason = b'recursion'
                elif case == 'git':
                    manifest = {'modules': [{'path': 'module', 'revision': 'a' * 40}], 'assets': []}
                    self.archive(workspace, json.dumps(manifest).encode(), b'invalid pack')
                    reason = b'index-pack'
                result = self.execute(root, workspace, *arguments)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertEqual(result.stdout, b'')
                self.assertIn(b'horizon-worker-source:', result.stderr)
                if case == 'json-depth':
                    self.assertTrue(b'recursion' in result.stderr or b'Invalid source manifest' in result.stderr,
                                    result.stderr)
                else:
                    self.assertIn(reason, result.stderr)
                self.assertNotIn(b'CRASH_HOOK_CALLED', result.stderr)
                self.assertNotIn(b'Traceback', result.stderr)
                self.assertFalse((workspace / 'source' / 'manifest.json').exists())

    def test_symlink_loop_requests_do_not_call_the_crash_hook(self):
        for legacy in (False, True):
            with self.subTest(legacy=legacy), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                workspace = root / 'workspace'
                workspace.mkdir()
                loop = root / 'loop'
                loop.symlink_to(loop.name)
                result = self.execute(root, workspace, 'checkout', str(loop), legacy_symlink_loop=legacy)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertEqual(result.stdout, b'')
                self.assertIn(b'horizon-worker-source:', result.stderr)
                if legacy:
                    self.assertIn(b'Symlink loop', result.stderr)
                self.assertNotIn(b'CRASH_HOOK_CALLED', result.stderr)
                self.assertNotIn(b'Traceback', result.stderr)
                self.assertFalse((workspace / 'source').exists())

    def test_malformed_manifests_do_not_call_the_crash_hook(self):
        cases = [None, [], {}, {'modules': []}, {'assets': []}]
        for key in ('modules', 'assets'):
            for invalid in (None, {}, 'items', 1):
                cases.append({'modules': [], 'assets': [], key: invalid})
            for item in (None, [], {}, {'path': None}, {'path': []}, {'path': 1}, {'path': 'bad\0path'}, {'path': 'bad\ud800path'}):
                cases.append({'modules': [], 'assets': [], key: [item]})
        for path in ('./module', 'module/./child', 'module//child', 'module/', '.', '../module'):
            cases.append({'modules': [{'path': path, 'revision': 'a' * 40}], 'assets': []})
            cases.append({'modules': [], 'assets': [{'path': path, 'oid': 'a' * 64, 'size': 1}]})
        for revision in (None, [], 1, '', 'invalid'):
            cases.append({'modules': [{'path': 'module', 'revision': revision}], 'assets': []})
        for oid in (None, [], 1, '', 'invalid'):
            cases.append({'modules': [], 'assets': [{'path': 'asset', 'oid': oid, 'size': 1}]})
        for size in (None, [], '1', True, -1, 1.5):
            cases.append({'modules': [], 'assets': [{'path': 'asset', 'oid': 'a' * 64, 'size': size}]})
        cases.extend([{'modules': [{'path': 'module'}], 'assets': []},
                      {'modules': [], 'assets': [{'path': 'asset', 'size': 1}]},
                      {'modules': [], 'assets': [{'path': 'asset', 'oid': 'a' * 64}]}])
        for value in cases:
            with self.subTest(manifest=value), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                workspace = root / 'workspace'
                workspace.mkdir()
                self.archive(workspace, json.dumps(value).encode())
                result = self.execute(root, workspace, 'import')
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertEqual(result.stdout, b'')
                self.assertIn(b'horizon-worker-source: Invalid ', result.stderr)
                self.assertNotIn(b'CRASH_HOOK_CALLED', result.stderr)
                self.assertNotIn(b'Traceback', result.stderr)
                self.assertFalse((workspace / 'source').exists())

    def test_git_source_decoding_failure_does_not_call_the_crash_hook(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workspace = root / 'workspace'
            local = root / 'local'
            agent = workspace / 'agents' / 'session'
            source = workspace / 'source'
            agent.mkdir(parents=True)
            source.mkdir()

            def git(*arguments):
                return subprocess.check_output(['git', *map(str, arguments)], stderr=subprocess.DEVNULL)

            git('init', '-q', local)
            (local / 'file.txt').write_text('committed source\n')
            git('-C', local, 'add', 'file.txt')
            git('-C', local, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                'commit', '-qm', 'Add fixture')
            revision = git('-C', local, 'rev-parse', 'HEAD').decode().strip()
            git('clone', '--bare', '-q', local, source / 'module-0.git')
            (source / 'manifest.json').write_text(json.dumps({
                'modules': [{'path': 'module', 'revision': revision}], 'assets': []}))
            git('init', '-q', agent)
            (agent / '.gitmodules').write_bytes(b'[submodule "module"]\npath = module\xff\n')
            result = self.execute(root, workspace, 'checkout', str(agent))
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn(b'horizon-worker-source:', result.stderr)
            self.assertIn(b'decode', result.stderr)
            self.assertNotIn(b'CRASH_HOOK_CALLED', result.stderr)
            self.assertNotIn(b'Traceback', result.stderr)

    def test_contract_probes_succeed_without_a_workspace(self):
        for flag, contract in [('shallow', b'horizon-source-shallow-contract=1\n'),
                               ('lfs-selection', b'horizon-source-lfs-selection-contract=1\n')]:
            with self.subTest(flag=flag), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                workspace = root / 'workspace'
                result = self.execute(root, workspace, '--' + flag + '-contract')
                self.assertEqual((result.returncode, result.stdout, result.stderr), (0, contract, b''))
                self.assertFalse(workspace.exists())

    def test_manifest_comparison_recursion_is_a_source_rejection(self):
        class RecursiveManifest(dict):
            def __eq__(self, other):
                raise RecursionError('manifest comparison depth exceeded')

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workspace = root / 'workspace'
            source = workspace / 'source'
            source.mkdir(parents=True)
            original = b'{"modules": [], "assets": []}'
            (source / 'manifest.json').write_bytes(original)
            self.archive(workspace, original)
            script = root / 'helper.py'
            script.write_text(SOURCE.read_text().replace('/workspace', str(workspace)))
            helper = runpy.run_path(str(script), run_name='horizon_worker_source')
            entry = helper['entry']
            with mock.patch.object(helper['os'], 'umask'), mock.patch.dict(entry.__globals__, manifest=mock.Mock(side_effect=[
                    {'modules': [], 'assets': []}, RecursiveManifest()])):
                with contextlib.redirect_stderr(io.StringIO()) as stderr:
                    self.assertEqual(entry(['import']), 1)
                self.assertEqual(stderr.getvalue(), 'horizon-worker-source: manifest comparison depth exceeded\n')
            self.assertEqual((source / 'manifest.json').read_bytes(), original)
            self.assertTrue((workspace / 'horizon-source.tar').exists())

    def test_parser_recursion_error_is_a_source_rejection(self):
        helper = runpy.run_path(str(SOURCE), run_name='horizon_worker_source')
        entry = helper['entry']
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'manifest.json').write_text('{"modules": [], "assets": []}')
            with mock.patch.object(helper['json'], 'loads', side_effect=RecursionError('manifest depth exceeded')):
                with mock.patch.dict(entry.__globals__, main=lambda arguments: helper['manifest'](root)):
                    with contextlib.redirect_stderr(io.StringIO()) as stderr:
                        self.assertEqual(entry(['import']), 1)
                    self.assertEqual(stderr.getvalue(), 'horizon-worker-source: manifest depth exceeded\n')

    def test_expected_errors_keep_the_diagnostic_and_unexpected_errors_propagate(self):
        helper = runpy.run_path(str(SOURCE), run_name='horizon_worker_source')
        entry = helper['entry']
        errors = [helper['SourceError']('invalid request'), OSError('storage unavailable'),
                  UnicodeDecodeError('utf-8', b'\xff', 0, 1, 'invalid source text'),
                  subprocess.CalledProcessError(128, ['git', 'index-pack']),
                  tarfile.ReadError('invalid archive')]
        for error in errors:
            with self.subTest(error=type(error)), contextlib.redirect_stderr(io.StringIO()) as stderr:
                with mock.patch.dict(entry.__globals__, main=mock.Mock(side_effect=error)):
                    self.assertEqual(entry(['import']), 1)
                self.assertEqual(stderr.getvalue(), f'horizon-worker-source: {error}\n')
        for error_type in (ValueError, RuntimeError, RecursionError):
            with self.subTest(unexpected=error_type), contextlib.redirect_stderr(io.StringIO()) as stderr:
                with mock.patch.dict(entry.__globals__, main=mock.Mock(side_effect=error_type('unexpected defect'))):
                    with self.assertRaisesRegex(error_type, 'unexpected defect'):
                        entry(['import'])
                self.assertEqual(stderr.getvalue(), '')


if __name__ == '__main__':
    unittest.main()
