"""Generated GPU contexts use the same capability recipe as CPU contexts."""
import importlib.machinery
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest import mock

loader = importlib.machinery.SourceFileLoader('worker_context', str(Path(__file__).with_name('prepare-context.py')))
spec = importlib.util.spec_from_loader(loader.name, loader)
context = importlib.util.module_from_spec(spec)
loader.exec_module(context)


class WorkerContextTests(unittest.TestCase):
    def test_standalone_recipe_and_ignore_rules_admit_only_declared_inputs(self):
        import fnmatch
        import shlex
        source = Path(context.__file__).parent
        recipe = (source / 'Dockerfile').read_text().replace('\\\n', ' ')
        copies = [shlex.split(line)[1:-1] for line in recipe.splitlines()
                  if line.startswith('COPY horizon-worker-')]
        self.assertEqual(copies, [list(context.WORKER_SCRIPTS)])
        rules = (source / '.dockerignore').read_text().splitlines()
        admitted = {line[1:] for line in rules if line.startswith('!')}
        self.assertEqual(admitted, {'Dockerfile', 'Dockerfile.gpu', 'bin/'}
                         | set(context.WORKER_SCRIPTS)
                         | {'bin/' + name for name in context.HELPERS})
        helper_copies = [shlex.split(line)[1:-1] for line in recipe.splitlines()
                         if line.startswith('COPY bin/')]
        self.assertEqual(helper_copies, [['bin/' + name for name in context.HELPERS]])
        # Docker matches both the path and its parent directories, last rule wins.
        def included(name):
            paths = [name] + [str(p) for p in Path(name).parents if str(p) != '.']
            result = True
            for rule in rules:
                pattern = rule.lstrip('!').rstrip('/')
                if any(fnmatch.fnmatchcase(path, pattern) for path in paths):
                    result = rule.startswith('!')
            return result
        for name in ['horizon-worker-credentials', 'horizon-worker-start.backup',
                     'bin/private-setting', 'bin/subdir/private-setting',
                     'bin/horizon-cloud-worker.old']:
            self.assertFalse(included(name), name)
        for name in context.WORKER_SCRIPTS:
            self.assertTrue(included(name), name)
        for name in context.HELPERS:
            self.assertTrue(included('bin/' + name), name)

    def test_gpu_context_contains_a_standalone_pinned_recipe_and_only_allowed_inputs(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            binaries = root / 'binaries'
            binaries.mkdir()
            for name in ['horizon-cloud-worker', 'horizon-browser', 'horizon-device']:
                (binaries / name).write_bytes(b'synthetic-helper')
            (binaries / 'private-setting').write_text('must-not-be-copied')
            output = root / 'context'
            base = 'registry.example.com/cuda-runtime@sha256:' + 'a' * 64
            with mock.patch.object(context.subprocess, 'run') as strip:
                context.prepare_context(binaries, output, base)
            gpu = (output / 'Dockerfile.gpu').read_text()
            self.assertIn('ARG BASE_IMAGE=' + base, gpu)
            self.assertNotIn('WORKER_IMAGE', gpu)
            for option in ['HORIZON_AGENTS', 'HORIZON_BROWSERS', 'HORIZON_DESKTOP']:
                self.assertIn('ARG ' + option + '=', gpu)
            self.assertIn('FROM ${BASE_IMAGE}', gpu)
            self.assertTrue((output / '.dockerignore').is_file())
            self.assertFalse((output / 'bin/private-setting').exists())
            self.assertEqual(strip.call_count, 3)
            with self.assertRaises(ValueError):
                context.prepare_context(binaries, output, base)

    def test_unpinned_or_credential_bearing_gpu_base_fails_without_creating_a_context(self):
        with tempfile.TemporaryDirectory() as root:
            output = Path(root) / 'context'
            for base in ['', 'ubuntu:24.04', 'https://user:secret@example.invalid/image',
                         'runtime@sha256:' + 'a' * 64 + '\nRUN unwanted']:
                with self.assertRaises(ValueError):
                    context.prepare_context(Path(root), output, base)
                self.assertFalse(output.exists())

    def test_unknown_matching_files_are_excluded_and_symlink_inputs_are_rejected(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            source, binaries = root / 'source', root / 'bin'
            source.mkdir()
            binaries.mkdir()
            original = Path(context.__file__).parent
            for name in context.CONTEXT_FILES:
                (source / name).write_bytes((original / name).read_bytes())
            for name in context.HELPERS:
                (binaries / name).write_bytes(b'synthetic-helper')
            secret = root / 'private-runtime-setting'
            secret.write_text('synthetic-secret-must-not-enter-context')
            (source / 'horizon-worker-history').write_bytes(secret.read_bytes())
            (source / 'horizon-worker-local-link').symlink_to(secret)
            with mock.patch.object(context, '__file__', str(source / 'prepare-context.py')), \
                    mock.patch.object(context.subprocess, 'run'):
                output = root / 'allowed'
                context.prepare_context(binaries, output)
                self.assertEqual({p.name for p in output.iterdir()},
                                 set(context.CONTEXT_FILES) | {'bin', 'manifest.json'})
                for path in output.rglob('*'):
                    if path.is_file():
                        self.assertNotIn(secret.read_bytes(), path.read_bytes())
                for selected in [source / context.WORKER_SCRIPTS[0], binaries / context.HELPERS[0]]:
                    original_bytes = selected.read_bytes()
                    selected.unlink()
                    selected.symlink_to(secret)
                    rejected = root / 'rejected'
                    with self.assertRaises(ValueError):
                        context.prepare_context(binaries, rejected)
                    self.assertFalse(rejected.exists())
                    selected.unlink()
                    selected.write_bytes(original_bytes)


if __name__ == '__main__':
    unittest.main()
