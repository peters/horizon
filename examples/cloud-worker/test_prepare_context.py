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


if __name__ == '__main__':
    unittest.main()
