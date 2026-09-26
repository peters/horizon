"""Sibling manifest validation against real imported sibling repositories."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

SCRIPTS = Path(__file__).parent


@unittest.skipUnless(shutil.which('git'), 'sibling manifests are verified against Git repositories')
class SiblingManifestTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.workspace = self.root / 'workspace'
        self.workspace.mkdir()
        self.env = dict(os.environ, GIT_CONFIG_NOSYSTEM='1')
        for name in ['horizon-worker-siblings', 'horizon-worker-import']:
            (self.root / name).write_text((SCRIPTS / name).read_text().replace('/workspace', str(self.workspace)))
        self.revisions = {alias: self.import_sibling(alias, format) for alias, format in [('lib', 'sha1'), ('tools', 'sha256')]}

    def git(self, *args, **kwargs):
        return subprocess.run(['git', *map(str, args)], check=True, capture_output=True, env=self.env, **kwargs).stdout

    def import_sibling(self, alias, object_format, material=True):
        local = self.root / ('local-' + alias)
        self.git('init', '--quiet', '--object-format=' + object_format, local)
        (local / 'file.txt').write_text(alias + '\n')
        self.git('-C', local, 'add', 'file.txt')
        self.git('-C', local, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '--quiet', '-m', 'Base')
        revision = self.git('-C', local, 'rev-parse', 'HEAD').decode().strip()
        staged = subprocess.run(['python3', str(self.root / 'horizon-worker-siblings'), 'stage', alias],
                                capture_output=True, text=True, env=self.env, check=True).stdout.strip()
        (Path(staged) / 'horizon-transfer.pack').write_bytes(
            self.git('-C', local, 'pack-objects', '--stdout', '--revs', input=(revision + '\n').encode()))
        subprocess.run(['bash', str(self.root / 'horizon-worker-import'), revision, '--sibling', alias],
                       check=True, capture_output=True, env=self.env)
        if material:
            source = self.workspace / 'siblings' / alias / 'source'
            source.mkdir()
            (source / 'manifest.json').write_text('{"modules":[],"assets":[]}')
        return revision

    def siblings(self, *args, stdin=b''):
        return subprocess.run(['python3', str(self.root / 'horizon-worker-siblings'), *args], input=stdin,
                              capture_output=True, env=self.env)

    def manifest(self, **overrides):
        value = {'version': 1, 'primary': 'App',
                 'siblings': [{'alias': alias, 'directory': alias.capitalize(), 'revision': revision}
                              for alias, revision in self.revisions.items()]}
        value.update(overrides)
        return value

    def set(self, value):
        return self.siblings('set', stdin=value if isinstance(value, bytes) else json.dumps(value).encode())

    def shown(self):
        result = self.siblings('show')
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_show_reports_an_empty_manifest_before_any_set(self):
        self.assertEqual(self.shown(), {'version': 1, 'primary': None, 'siblings': []})
        self.assertFalse((self.workspace / 'siblings.json').exists())

    def test_set_records_imported_siblings_atomically_and_privately(self):
        recorded = self.set(self.manifest())
        self.assertEqual(recorded.returncode, 0, recorded.stderr)
        self.assertEqual(self.shown(), self.manifest())
        self.assertEqual((self.workspace / 'siblings.json').stat().st_mode & 0o777, 0o600)
        self.assertEqual([path.name for path in self.workspace.iterdir() if path.name.startswith('siblings.')], ['siblings.json'])
        cleared = self.set(self.manifest(siblings=[]))
        self.assertEqual(cleared.returncode, 0, cleared.stderr)
        self.assertEqual(self.shown(), self.manifest(siblings=[]))

    def assert_refused(self, value, message):
        before = (self.workspace / 'siblings.json').read_bytes() if (self.workspace / 'siblings.json').exists() else None
        result = self.set(value)
        self.assertNotEqual(result.returncode, 0, value)
        self.assertIn(message, result.stderr.decode(), value)
        after = (self.workspace / 'siblings.json').read_bytes() if (self.workspace / 'siblings.json').exists() else None
        self.assertEqual(after, before)

    def test_invalid_names_are_refused(self):
        sibling = self.manifest()['siblings'][0]
        for alias in ['Lib', '1lib', '-lib', 'lib/x', 'lib.x', 'a' * 65, '', 7]:
            self.assert_refused(self.manifest(siblings=[dict(sibling, alias=alias)]), 'alias')
        for directory in ['.', '..', '.git', '.GIT', '.hidden', '-option', 'a/b', '', 'a b', '/abs', 'x' * 256, 'café', None]:
            self.assert_refused(self.manifest(siblings=[dict(sibling, directory=directory)]), 'directory')
            self.assert_refused(self.manifest(primary=directory), 'directory')
        for revision in ['abc', self.revisions['lib'].upper(), self.revisions['lib'] + '0', 1]:
            self.assert_refused(self.manifest(siblings=[dict(sibling, revision=revision)]), 'revision')

    def test_duplicates_are_refused_case_insensitively_including_the_primary(self):
        lib, tools = self.manifest()['siblings']
        self.assert_refused(self.manifest(siblings=[lib, dict(tools, alias='lib')]), 'Duplicate')
        self.assert_refused(self.manifest(siblings=[lib, dict(tools, directory='LIB')]), 'Duplicate')
        self.assert_refused(self.manifest(primary='lib'), 'Duplicate')
        self.assert_refused(b'{"version":1,"version":1,"primary":"App","siblings":[]}', 'Duplicate')

    def test_revision_must_equal_the_imported_base(self):
        self.assertEqual(self.set(self.manifest()).returncode, 0)
        lib, tools = self.manifest()['siblings']
        self.assert_refused(self.manifest(siblings=[dict(lib, revision='0' * 40), tools]), 'differs from its imported base')
        # A revision of the other object format is well formed but still not the imported base.
        self.assert_refused(self.manifest(siblings=[dict(lib, revision=tools['revision'])]), 'differs from its imported base')

    def test_unimported_repository_or_material_is_refused(self):
        self.assert_refused(self.manifest(siblings=[{'alias': 'missing', 'directory': 'Missing', 'revision': '0' * 40}]),
                            'has not been imported')
        revision = self.import_sibling('bare', 'sha1', material=False)
        self.assert_refused(self.manifest(siblings=[{'alias': 'bare', 'directory': 'Bare', 'revision': revision}]),
                            'has not been imported')

    def test_unknown_keys_versions_and_shapes_are_refused(self):
        lib = self.manifest()['siblings'][0]
        self.assert_refused(dict(self.manifest(), extra=True), 'Invalid sibling manifest')
        missing = self.manifest()
        del missing['primary']
        self.assert_refused(missing, 'Invalid sibling manifest')
        self.assert_refused(self.manifest(siblings=[dict(lib, path='/elsewhere')]), 'Invalid sibling entry')
        self.assert_refused(self.manifest(siblings=[{'alias': 'lib', 'directory': 'Lib'}]), 'Invalid sibling entry')
        for version in [2, True, '1', 1.0]:
            self.assert_refused(self.manifest(version=version), 'version')
        for siblings in [{}, 'lib', [lib] * 17]:
            self.assert_refused(self.manifest(siblings=siblings), 'Invalid sibling list')
        self.assert_refused([], 'Invalid sibling manifest')
        self.assert_refused(b'{not json', 'Expecting')

    def test_oversize_input_is_refused_before_parsing(self):
        value = json.dumps(self.manifest()).encode()
        self.assert_refused(value[:-1] + b' ' * (64 * 1024) + b'}', 'too large')

    def test_the_empty_manifest_from_show_clears_the_siblings(self):
        self.assertEqual(self.set(self.manifest()).returncode, 0)
        empty = {'version': 1, 'primary': None, 'siblings': []}
        cleared = self.set(empty)
        self.assertEqual(cleared.returncode, 0, cleared.stderr)
        self.assertEqual(self.shown(), empty)
        self.assert_refused(dict(empty, siblings=self.manifest()['siblings']), 'directory')

    def test_stage_creates_a_private_upload_directory_per_valid_alias(self):
        staged = self.siblings('stage', 'fresh')
        self.assertEqual(staged.returncode, 0, staged.stderr)
        directory = self.workspace / 'siblings' / 'fresh'
        self.assertEqual(staged.stdout.decode().strip(), str(directory))
        self.assertEqual(directory.stat().st_mode & 0o777, 0o700)
        self.assertEqual(self.siblings('stage', 'fresh').returncode, 0)
        for alias in ['Fresh', '../fresh', 'fresh/x', '']:
            self.assertNotEqual(self.siblings('stage', alias).returncode, 0, alias)
        (self.workspace / 'siblings' / 'linked').symlink_to(self.root)
        self.assertNotEqual(self.siblings('stage', 'linked').returncode, 0)

    def test_usage_errors_write_nothing(self):
        for args in [(), ('list',), ('set', 'extra'), ('show', 'extra')]:
            self.assertNotEqual(self.siblings(*args, stdin=json.dumps(self.manifest()).encode()).returncode, 0, args)
        self.assertFalse((self.workspace / 'siblings.json').exists())


if __name__ == '__main__':
    unittest.main()
