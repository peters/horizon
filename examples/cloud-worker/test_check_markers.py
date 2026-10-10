"""The published helper artifact and the marker gate that guards it."""
import importlib.machinery
import importlib.util
import json
import os
from pathlib import Path
import re
import stat
import tempfile
import unittest

loader = importlib.machinery.SourceFileLoader('worker_markers', str(Path(__file__).with_name('check-markers.py')))
spec = importlib.util.spec_from_loader(loader.name, loader)
markers = importlib.util.module_from_spec(spec)
loader.exec_module(markers)

CURRENT = ['horizon-siblings-contract=1', 'horizon-session-env-contract=1', 'horizon-gpu-lock-contract=1',
           'horizon-shared-checkout-contract=1', 'horizon-prepare-checkout-contract=1', 'horizon-session-restart-contract=1',
           'horizon-idle-report-contract=1', 'horizon-git-auth-contract=2', 'horizon-tailnet-contract=2', 'horizon-tailnet-contract=3']


def fake_docker(root, output, status=0):
    """A docker stand-in that records its arguments and prints a canned checker reply."""
    script = Path(root) / 'docker'
    record = Path(root) / 'argv.json'
    script.write_text('#!/usr/bin/env python3\nimport json, sys\n'
                      f'open({str(record)!r}, "w").write(json.dumps(sys.argv[1:]))\n'
                      f'sys.stdout.write({output!r})\nsys.stderr.write("checker refused")\nsys.exit({status})\n')
    script.chmod(script.stat().st_mode | stat.S_IXUSR)
    return script, record


class MarkerTests(unittest.TestCase):
    def test_expected_markers_come_from_this_revisions_checker(self):
        expected = markers.expected_markers()
        for marker in CURRENT:
            self.assertIn(marker, expected)
        self.assertTrue(markers.CONDITIONAL.isdisjoint(expected))
        source = markers.CHECKER.read_text()
        for marker in markers.CONDITIONAL:
            self.assertIn(marker, source, 'a conditional marker the checker no longer prints')

    def test_only_whole_reported_lines_count(self):
        expected = ['horizon-gpu-lock-contract=1', 'horizon-siblings-contract=1']
        output = 'prefix horizon-siblings-contract=1\nhorizon-gpu-lock-contract=1\n'
        self.assertEqual(markers.missing_markers(output, expected), ['horizon-siblings-contract=1'])
        self.assertEqual(markers.missing_markers('\n'.join(expected) + '\n', expected), [])

    def test_image_check_runs_the_images_checker_offline_with_an_empty_selection(self):
        with tempfile.TemporaryDirectory() as root:
            docker, record = fake_docker(root, '\n'.join(markers.expected_markers()) + '\n')
            image = 'registry.example.com/team/worker@sha256:' + 'a' * 64
            self.assertEqual(markers.check_image(image, str(docker)), [])
            argv = json.loads(record.read_text())
        self.assertEqual(argv[:2], ['run', '--rm'])
        self.assertIn('--network=none', argv)
        self.assertEqual(argv[argv.index('--entrypoint') + 1], '/usr/local/bin/horizon-worker-check')
        selection = argv[argv.index('--env') + 1].removeprefix('HORIZON_WORKER_CAPABILITIES=')
        self.assertEqual(json.loads(selection), {'agents': [], 'browsers': [], 'desktop': False})
        self.assertEqual(argv[-2:], [image, '--git-auth'])

    def test_an_older_image_reports_what_it_lacks(self):
        older = [marker for marker in markers.expected_markers() if marker not in CURRENT]
        with tempfile.TemporaryDirectory() as root:
            docker, _ = fake_docker(root, '\n'.join(older) + '\n')
            self.assertEqual(markers.check_image('worker', str(docker)), sorted(CURRENT))

    def test_a_failing_checker_or_unsafe_reference_is_an_error(self):
        with tempfile.TemporaryDirectory() as root:
            docker, record = fake_docker(root, '', status=1)
            with self.assertRaisesRegex(ValueError, 'checker refused'):
                markers.check_image('worker', str(docker))
            os.remove(record)
            for image in ['', '--privileged', 'worker --privileged', 'worker\n']:
                with self.assertRaises(ValueError):
                    markers.check_image(image, str(docker))
            self.assertFalse(record.exists())


class HelperRecipeTests(unittest.TestCase):
    def test_every_base_is_pinned_and_the_artifact_holds_every_worker_script(self):
        recipe = Path(__file__).with_name('Dockerfile.helpers').read_text()
        bases = re.findall(r'^FROM (\S+)', recipe, re.MULTILINE)
        self.assertIn('scratch', bases)
        for base in bases:
            if base != 'scratch':
                self.assertRegex(base, r'@sha256:[0-9a-f]{64}$')
        self.assertIn('install -m 755 examples/cloud-worker/horizon-worker-* /output/usr/local/bin/', recipe)
        self.assertIn('ln -s /usr/local/bin/horizon-worker-git-auth /output/usr/local/bin/gh', recipe)
        # Dependency notices come from the collector the worker images use, over this build's cache.
        self.assertIn('CARGO_HOME=/tmp/helper-cargo cargo build', recipe)
        self.assertIn('python3 .horizon/collect-helper-notices.py', recipe)
        self.assertIn('mv /output/share/licenses/horizon-dependencies /output/usr/local/share/licenses/', recipe)
        self.assertRegex(recipe, r'FROM scratch AS helpers\n(?:.*\n)*?COPY --from=build /output/ /')
        self.assertRegex(recipe, r'AS probe\n(?:.*\n)*?COPY --from=helpers / /')


if __name__ == '__main__':
    unittest.main()
