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
    def execute(self, root, workspace, *arguments):
        script = root / 'horizon-worker-source'
        script.write_text(SOURCE.read_text().replace('/workspace', str(workspace)))
        runner = root / 'runner.py'
        # Disable site hooks, then install a spy instead of the real crash reporter.
        # A regression must fail the test without opening another desktop dialog.
        runner.write_text("import runpy, sys\n"
                          "def crash_hook(kind, error, trace):\n"
                          "    print('CRASH_HOOK_CALLED: ' + str(error), file=sys.stderr)\n"
                          "sys.excepthook = crash_hook\n"
                          "sys.argv.pop(0)\n"
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
        cases = ['usage', 'worktree', 'dependencies', 'missing-upload', 'archive', 'json', 'git', 'lock']
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
                elif case == 'git':
                    manifest = {'modules': [{'path': 'module', 'revision': 'a' * 40}], 'assets': []}
                    self.archive(workspace, json.dumps(manifest).encode(), b'invalid pack')
                    reason = b'index-pack'
                result = self.execute(root, workspace, *arguments)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertEqual(result.stdout, b'')
                self.assertIn(b'horizon-worker-source:', result.stderr)
                self.assertIn(reason, result.stderr)
                self.assertNotIn(b'CRASH_HOOK_CALLED', result.stderr)
                self.assertNotIn(b'Traceback', result.stderr)
                self.assertFalse((workspace / 'source' / 'manifest.json').exists())

    def test_contract_probes_succeed_without_a_workspace(self):
        for flag, contract in [('shallow', b'horizon-source-shallow-contract=1\n'),
                               ('lfs-selection', b'horizon-source-lfs-selection-contract=1\n')]:
            with self.subTest(flag=flag), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                workspace = root / 'workspace'
                result = self.execute(root, workspace, '--' + flag + '-contract')
                self.assertEqual((result.returncode, result.stdout, result.stderr), (0, contract, b''))
                self.assertFalse(workspace.exists())

    def test_expected_errors_keep_the_diagnostic_and_unexpected_errors_propagate(self):
        helper = runpy.run_path(str(SOURCE), run_name='horizon_worker_source')
        entry = helper['entry']
        errors = [ValueError('invalid request'), OSError('storage unavailable'),
                  subprocess.CalledProcessError(128, ['git', 'index-pack']),
                  tarfile.ReadError('invalid archive')]
        for error in errors:
            with self.subTest(error=type(error)), contextlib.redirect_stderr(io.StringIO()) as stderr:
                with mock.patch.dict(entry.__globals__, main=mock.Mock(side_effect=error)):
                    self.assertEqual(entry(['import']), 1)
                self.assertEqual(stderr.getvalue(), f'horizon-worker-source: {error}\n')
        with mock.patch.dict(entry.__globals__, main=mock.Mock(side_effect=RuntimeError('unexpected defect'))):
            with self.assertRaisesRegex(RuntimeError, 'unexpected defect'):
                entry(['import'])


if __name__ == '__main__':
    unittest.main()
