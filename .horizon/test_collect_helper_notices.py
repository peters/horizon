"""Helper notices follow each published crate version, including the pinned worker lockfile."""
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import tomllib
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location('collect_helper_notices', Path(__file__).with_name('collect-helper-notices.py'))
collector = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(collector)
RMCP_341 = '9427a929959e665e0d12e9395f674026baf4bd48'
RMCP_350 = '0cde3c5cf3e6aff0cc852ce6045f107e95991f48'
RMCP_351 = '79437f291b2c44053d00dcd5db969fd0cca7c887'


class HelperNoticeTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name) / 'cargo'
        self.output = Path(temp.name) / 'notices'
        self.download = mock.patch.object(collector.urllib.request, 'urlopen')
        self.urlopen = self.download.start()
        self.addCleanup(self.download.stop)
        self.urlopen.side_effect = lambda url, timeout: io.BytesIO(url.encode())

    def crate(self, name, version, revision, location='registry/src/test-registry'):
        source = self.root / location / f'{name}-{version}'
        source.mkdir(parents=True)
        (source / 'Cargo.toml').write_text(
            f'[package]\nname = "{name}"\nversion = "{version}"\nlicense = "MIT"\n')
        if revision is not None:
            (source / '.cargo_vcs_info.json').write_text(json.dumps({'git': {'sha1': revision}}))
        return source

    def test_pinned_worker_uses_distinct_rmcp_and_macro_revisions(self):
        self.crate('rmcp', '3.4.1', RMCP_341)
        self.crate('rmcp-macros', '3.5.0', RMCP_350)
        collector.collect_notices(self.root, self.output)
        self.assertEqual(self.urlopen.call_args_list, [
            mock.call(f'https://raw.githubusercontent.com/modelcontextprotocol/rust-sdk/{revision}/LICENSE', timeout=60)
            for revision in (RMCP_341, RMCP_350)])
        packages = json.loads((self.output / 'packages.json').read_text())
        self.assertEqual([(p['name'], p['version'], p['notices']) for p in packages], [
            ('rmcp', '3.4.1', ['LICENSE']), ('rmcp-macros', '3.5.0', ['LICENSE'])])
        for package, revision in zip(packages, (RMCP_341, RMCP_350)):
            destination = self.output / f"{package['name']}-{package['version']}"
            self.assertIn(revision, (destination / 'LICENSE').read_text())
            provenance = json.loads((destination / 'provenance.json').read_text())
            self.assertEqual(provenance['source_revision']['git']['sha1'], revision)

    def test_workspace_lockfile_has_reviewed_supplements(self):
        lockfile = Path(__file__).resolve().parents[1] / 'Cargo.lock'
        packages = tomllib.loads(lockfile.read_text())['package']
        for package in packages:
            if package['name'] in collector.SUPPLEMENTED_NAMES:
                identity = package['name'], package['version']
                with self.subTest(package=identity):
                    self.assertIn(identity, collector.SUPPLEMENTS)

    def test_current_rmcp_and_macros_use_their_reviewed_revision(self):
        for name in ('rmcp', 'rmcp-macros'):
            self.crate(name, '3.5.1', RMCP_351)
        collector.collect_notices(self.root, self.output)
        for name in ('rmcp', 'rmcp-macros'):
            destination = self.output / f'{name}-3.5.1'
            self.assertIn(RMCP_351, (destination / 'LICENSE').read_text())
            provenance = json.loads((destination / 'provenance.json').read_text())
            self.assertEqual(provenance['source_revision']['git']['sha1'], RMCP_351)
        self.assertEqual(self.urlopen.call_args_list, [
            mock.call(f'https://raw.githubusercontent.com/modelcontextprotocol/rust-sdk/{RMCP_351}/LICENSE', timeout=60)
        ] * 2)

    def test_previous_rmcp_uses_its_own_reviewed_revision(self):
        self.crate('rmcp', '3.5.0', RMCP_350)
        collector.collect_notices(self.root, self.output)
        self.assertIn(RMCP_350, (self.output / 'rmcp-3.5.0/LICENSE').read_text())

    def test_wrong_revision_stops_before_download(self):
        self.crate('rmcp', '3.4.1', RMCP_350)
        with self.assertRaisesRegex(RuntimeError, f'rmcp 3.4.1: expected {RMCP_341}, got {RMCP_350}'):
            collector.collect_notices(self.root, self.output)
        self.urlopen.assert_not_called()

    def test_missing_provenance_stops_before_download(self):
        self.crate('rmcp', '3.4.1', None)
        with self.assertRaisesRegex(RuntimeError, 'License provenance mismatch for rmcp 3.4.1:.*got None'):
            collector.collect_notices(self.root, self.output)
        self.urlopen.assert_not_called()

    def test_unknown_version_requires_review(self):
        self.crate('rmcp', '9.0.0', RMCP_350)
        with self.assertRaisesRegex(RuntimeError, 'No reviewed license supplement for rmcp 9.0.0'):
            collector.collect_notices(self.root, self.output)
        self.urlopen.assert_not_called()

    def test_existing_nested_and_declared_notices_are_preserved(self):
        source = self.crate('ordinary', '1.0.0', None, 'git/checkouts/test-repository')
        (source / 'Cargo.toml').write_text(
            '[package]\nname = "ordinary"\nversion = "1.0.0"\nlicense-file = "terms.txt"\n')
        (source / 'terms.txt').write_text('declared license')
        (source / 'vendor').mkdir()
        (source / 'vendor/NOTICE').write_text('vendored notice')
        collector.collect_notices(self.root, self.output)
        destination = self.output / source.name
        self.assertEqual((destination / 'terms.txt').read_text(), 'declared license')
        self.assertEqual((destination / 'vendor/NOTICE').read_text(), 'vendored notice')
        package, = json.loads((self.output / 'packages.json').read_text())
        self.assertEqual(package['notices'], ['terms.txt', 'vendor/NOTICE'])
        self.urlopen.assert_not_called()


if __name__ == '__main__':
    unittest.main()
