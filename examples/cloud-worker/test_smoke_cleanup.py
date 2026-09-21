"""A failed cleanup must not leave another exact task container unattempted."""
import importlib.util
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('worker_image_smoke', Path(__file__).with_name('smoke-image.py'))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class CleanupTests(unittest.TestCase):
    def test_timeout_still_attempts_the_other_task_container(self):
        calls = []

        def command(args, **_kwargs):
            calls.append(args)
            if args[:2] == ['docker', 'rm'] and args[-1] == 'owned-contract':
                raise subprocess.TimeoutExpired(args, 45)
            return subprocess.CompletedProcess(args, 0, stdout='')

        with patch.object(smoke.subprocess, 'run', command):
            with self.assertRaisesRegex(RuntimeError, 'owned-contract'):
                smoke.remove(['owned-contract', 'owned-service'])
        self.assertIn(['docker', 'rm', '--force', '--volumes', 'owned-service'], calls)

    def test_unconfirmed_removal_is_reported_after_both_attempts(self):
        calls = []

        def command(args, **_kwargs):
            calls.append(args)
            return subprocess.CompletedProcess(args, 0, stdout='still-present' if 'ls' in args else '')

        with patch.object(smoke.subprocess, 'run', command):
            with self.assertRaisesRegex(RuntimeError, 'owned-contract, owned-service'):
                smoke.remove(['owned-contract', 'owned-service'])
        self.assertEqual(sum(args[:2] == ['docker', 'rm'] for args in calls), 2)


if __name__ == '__main__':
    unittest.main()
