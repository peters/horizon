"""Codex source derivation, fallback and integrity against a synthetic release and upstream."""
import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest import mock
import urllib.error

HERE = Path(__file__).parent
SPEC = importlib.util.spec_from_file_location('retain_component_sources', HERE / 'retain-component-sources.py')
retain = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(retain)
REVIEWED = json.loads((HERE / 'component-sources.json').read_text())
# Optional: a Codex checkout at rust-v0.155.1, to check the derivation against the real layout.
REAL_TREE = os.environ.get('HORIZON_TEST_CODEX_TREE')
VERSION = '0.156.1'
COMMIT = 'b412ff32c417f855c2b2d1581b77058eed87c84b'


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def by_name(components, *names):
    return [component for component in components if component['name'] in names]


def release_tree(voice, libcap, ripgrep=retain.PINNED_RIPGREP, zsh=retain.PINNED_ZSH, notice='release'):
    """Release files in the upstream formats, listing the given source entries."""
    sources = [{'name': c['name'], 'version': c['version'], 'role': c['role'], 'archive': c['filename'],
                'root': f"{c['name']}-{c['version']}", 'url': c['url'], 'sha256': c['sha256'],
                'provenance': 'Upstream checksum'} for c in voice]
    platforms = {platform: {'format': 'tar.gz', 'providers': [{'url': (
        f'https://github.com/BurntSushi/ripgrep/releases/download/{ripgrep}/ripgrep-{ripgrep}-{platform}.tar.gz')}]}
        for platform in ('linux-aarch64', 'linux-x86_64')}
    return {
        retain.VOICE_SOURCES: json.dumps({'schema_version': 1, 'sources': sources, 'bundled_sources': []}, indent=2),
        retain.LIBCAP_SCRIPT: (f'#!/usr/bin/env bash\nlibcap_version="{libcap["version"]}"\n'
                               f'libcap_sha256="{libcap["sha256"]}"\n'
                               'libcap_tarball_name="libcap-${libcap_version}.tar.xz"\n'),
        retain.RIPGREP_MANIFEST: '#!/usr/bin/env dotslash\n\n' + json.dumps({'name': 'rg', 'platforms': platforms}),
        retain.ZSH_WORKFLOW: f'name: rust-release-zsh\nenv:\n  ZSH_COMMIT: {zsh}\njobs: {{}}\n',
        **{path: f'{notice} {path}\n' for path in retain.TREE_NOTICES},
    }


def source_archive(files, commit):
    """A codeload-style archive with every file below codex-<commit>/."""
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode='w:gz') as tar:
        for path, content in files.items():
            data = content if isinstance(content, bytes) else content.encode()
            member = tarfile.TarInfo(f'codex-{commit}/{path}')
            member.size = len(data)
            tar.addfile(member, io.BytesIO(data))
    return buffer.getvalue()


class DerivationTests(unittest.TestCase):
    def assert_reproduces_reviewed_manifest(self, files):
        reviewed_source, = by_name(REVIEWED, retain.CORRESPONDING_SOURCE)
        with tempfile.TemporaryDirectory() as scratch:
            archive = Path(scratch) / 'source.tar.gz'
            archive.write_bytes(source_archive(files, retain.PINNED_COMMIT))
            # Only the corresponding-source checksum is computed from the downloaded archive.
            release = retain.derive(retain.PINNED_VERSION, retain.PINNED_COMMIT, archive, reviewed_source['sha256'])
        self.assertEqual(release.components, REVIEWED)
        self.assertEqual(release.manifest, (HERE / 'component-sources.json').read_bytes())
        self.assertEqual((release.ripgrep, release.zsh), (retain.PINNED_RIPGREP, retain.PINNED_ZSH))

    def test_reviewed_layout_reproduces_the_reviewed_manifest(self):
        voice = [c for c in REVIEWED if c['name'] not in {retain.CORRESPONDING_SOURCE, 'libcap'}]
        libcap, = by_name(REVIEWED, 'libcap')
        self.assert_reproduces_reviewed_manifest(release_tree(voice, libcap))

    @unittest.skipUnless(REAL_TREE, 'HORIZON_TEST_CODEX_TREE is not set')
    def test_real_pinned_tree_reproduces_the_reviewed_manifest(self):
        self.assert_reproduces_reviewed_manifest({path: (Path(REAL_TREE) / path).read_bytes() for path in retain.LAYOUT})


class RetentionTests(unittest.TestCase):
    """Runs the build step against an in-memory upstream; nothing touches the network."""

    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.upstream = {}
        self.tags = {VERSION: ('81e8e29b2956dfe9b092c63953a9ed282781e77c', COMMIT)}
        self.git = []
        self.voice = [self.serve('opus', '1.6.1'), self.serve('glib', '2.88.3')]
        libcap_url = 'https://mirrors.edge.kernel.org/pub/linux/libs/security/linux-privs/libcap2/libcap-2.75.tar.xz'
        self.upstream[libcap_url] = b'libcap 2.75'
        self.libcap = {'name': 'libcap', 'version': '2.75', 'role': 'bubblewrap-statically-linked-native-library',
                       'url': libcap_url, 'filename': 'libcap-2.75.tar.xz', 'sha256': sha256(b'libcap 2.75')}
        for name in ('COPYING', 'LICENSE-MIT', 'UNLICENSE'):
            self.upstream[f'https://raw.githubusercontent.com/BurntSushi/ripgrep/15.2.0/{name}'] = f'rg {name}'.encode()
        self.upstream[f'https://raw.githubusercontent.com/zsh-users/zsh/{retain.PINNED_ZSH}/LICENCE'] = b'zsh'
        # The reviewed pin lists its own archives, so a fallback is distinguishable.
        pinned_archive = source_archive(release_tree(self.voice, self.libcap, notice='pinned'), retain.PINNED_COMMIT)
        pinned_url = f'https://codeload.github.com/openai/codex/tar.gz/{retain.PINNED_COMMIT}'
        self.upstream[pinned_url] = pinned_archive
        self.pinned = [self.serve('opus', '1.5.2'), {
            'name': retain.CORRESPONDING_SOURCE, 'version': retain.PINNED_COMMIT,
            'role': 'native-build-scripts-and-bubblewrap-source', 'url': pinned_url,
            'filename': f'agent-source-{retain.PINNED_COMMIT}.tar.gz', 'sha256': sha256(pinned_archive)}, self.libcap]
        self.pinned_manifest = self.root / 'component-sources.json'
        self.pinned_manifest.write_text(json.dumps(self.pinned, indent=2) + '\n')
        self.publish(release_tree(self.voice, self.libcap))
        self.reviewed_notices = {url: sha256(data) for url, data in self.upstream.items()
                                 if url.startswith('https://raw.githubusercontent.com/')}

    def serve(self, name, version):
        url = f'https://example.org/{name}-{version}.tar.gz'
        self.upstream[url] = f'{name} {version}'.encode()
        return {'name': name, 'version': version, 'role': 'native-library', 'url': url,
                'filename': f'{name}-{version}.tar', 'sha256': sha256(self.upstream[url])}

    def publish(self, files):
        self.release_archive = source_archive(files, COMMIT)
        self.upstream[f'https://codeload.github.com/openai/codex/tar.gz/{COMMIT}'] = self.release_archive

    def urlopen(self, url, timeout):
        if url not in self.upstream:
            raise urllib.error.URLError('offline')
        return io.BytesIO(self.upstream[url])

    def ls_remote(self, command, **kwargs):
        self.git.append(command)
        version = command[5].removeprefix('refs/tags/rust-v')
        if version not in self.tags:
            return subprocess.CompletedProcess(command, 2, stdout='')
        if self.tags[version] is None:
            return subprocess.CompletedProcess(command, 128, stdout='')
        tag, commit = self.tags[version]
        return subprocess.CompletedProcess(command, 0, stdout=f'{tag}\t{command[5]}\n{commit}\t{command[6]}\n')

    def retain_sources(self, *args):
        """Run the build step into fresh output directories; returns its stdout and stderr."""
        self.output = Path(tempfile.mkdtemp(dir=self.root))
        self.sources, self.notices = self.output / 'sources', self.output / 'notices'
        stdout, stderr = io.StringIO(), io.StringIO()
        with mock.patch.multiple(retain, SOURCES=self.sources, NOTICES=self.notices,
                                 PINNED_MANIFEST=self.pinned_manifest, NOTICE_SHA256=self.reviewed_notices), \
                mock.patch('urllib.request.urlopen', side_effect=self.urlopen), \
                mock.patch('subprocess.run', side_effect=self.ls_remote), \
                contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            try:
                retain.main(['retain-component-sources.py', *args])
            finally:
                self.stdout, self.stderr = stdout.getvalue(), stderr.getvalue()
        return self.stdout, self.stderr

    def assert_retained_pin(self):
        self.assertEqual(self.stdout, f'{retain.PINNED_VERSION}\n')
        self.assertEqual((self.sources / 'manifest.json').read_bytes(), self.pinned_manifest.read_bytes())
        self.assertEqual(sorted(p.name for p in self.sources.iterdir()),
                         sorted([c['filename'] for c in self.pinned] + ['manifest.json']))
        self.assertEqual((self.notices / 'LICENSE').read_text(), 'pinned LICENSE\n')
        self.assertEqual((self.notices / 'SOURCE').read_text(),
                         f'https://github.com/openai/codex/tree/{retain.PINNED_COMMIT}\n')

    def test_requested_release_retains_the_sources_derived_from_its_tag(self):
        stdout, stderr = self.retain_sources(VERSION)
        self.assertEqual(stdout, f'{VERSION}\n')
        self.assertNotIn('WARNING', stderr)
        tag = f'refs/tags/rust-v{VERSION}'
        self.assertEqual(self.git, [['git', 'ls-remote', '--exit-code', '--tags', retain.REPOSITORY, tag, tag + '^{}']])
        source = {'name': retain.CORRESPONDING_SOURCE, 'version': COMMIT,
                  'role': 'native-build-scripts-and-bubblewrap-source',
                  'url': f'https://codeload.github.com/openai/codex/tar.gz/{COMMIT}',
                  'filename': f'agent-source-{COMMIT}.tar.gz', 'sha256': sha256(self.release_archive)}
        components = [*self.voice, source, self.libcap]
        self.assertEqual(json.loads((self.sources / 'manifest.json').read_text()), components)
        for component in components:
            self.assertEqual(sha256((self.sources / component['filename']).read_bytes()), component['sha256'])
        for path, name in retain.TREE_NOTICES.items():
            self.assertEqual((self.notices / name).read_text(), f'release {path}\n')
        self.assertEqual((self.notices / 'SOURCE').read_text(), f'https://github.com/openai/codex/tree/{COMMIT}\n')
        for name in ('rg-COPYING', 'rg-LICENSE-MIT', 'rg-UNLICENSE', 'zsh-LICENCE'):
            self.assertTrue((self.notices / 'bundled' / name).is_file(), name)

    def test_builds_without_a_requested_release_retain_the_pin_without_a_lookup(self):
        self.retain_sources('')
        self.assert_retained_pin()
        self.assertEqual(self.git, [])
        self.assertNotIn('WARNING', self.stderr)

    def test_unreadable_release_layout_installs_the_pin_with_a_warning(self):
        files = release_tree(self.voice, self.libcap)
        changed = {f'without {path}': {k: v for k, v in files.items() if k != path} for path in retain.LAYOUT}
        voice = json.loads(files[retain.VOICE_SOURCES])
        changed['voice schema 2'] = {**files, retain.VOICE_SOURCES: json.dumps({**voice, 'schema_version': 2})}
        changed['voice without sources'] = {**files, retain.VOICE_SOURCES: json.dumps({'schema_version': 1})}
        for case, change in [('archive over the manifest', {'archive': 'manifest.json'}),
                             ('archive outside the sources', {'archive': '../opus.tar'}),
                             ('archive without HTTPS', {'url': 'http://example.org/opus.tar.gz'})]:
            sources = [{**voice['sources'][0], **change}, *voice['sources'][1:]]
            changed[case] = {**files, retain.VOICE_SOURCES: json.dumps({**voice, 'sources': sources})}
        changed['libcap checksum moved'] = {**files, retain.LIBCAP_SCRIPT: 'libcap_version="2.76"\n'}
        changed['ripgrep not from releases'] = {**files, retain.RIPGREP_MANIFEST: '#!/usr/bin/env dotslash\n{}'}
        changed['zsh commit moved'] = {**files, retain.ZSH_WORKFLOW: 'env:\n  ZSH_REF: main\n'}
        for case, tree in [('missing tag', files), *changed.items()]:
            with self.subTest(case):
                self.publish(tree)
                self.retain_sources('9.9.9' if case == 'missing tag' else VERSION)
                self.assertIn('WARNING: the Codex', self.stderr)
                self.assertIn('source layout changed', self.stderr)
                self.assertIn(f'last verified Codex {retain.PINNED_VERSION}', self.stderr)
                self.assert_retained_pin()

    def test_integrity_and_network_failures_stop_the_build_instead_of_falling_back(self):
        release_url = f'https://codeload.github.com/openai/codex/tar.gz/{COMMIT}'
        rg_url = 'https://raw.githubusercontent.com/BurntSushi/ripgrep/15.2.0/COPYING'
        cases = {
            'voice archive changed': (VERSION, lambda: self.upstream.update({self.voice[0]['url']: b'x'}), ValueError),
            'reviewed notice changed': (VERSION, lambda: self.upstream.update({rg_url: b'x'}), ValueError),
            'pinned source changed': ('', lambda: self.upstream.update({self.pinned[1]['url']: b'x'}), ValueError),
            'release archive offline': (VERSION, lambda: self.upstream.pop(release_url), urllib.error.URLError),
            'release archive corrupt': (VERSION, lambda: self.upstream.update({release_url: b'x'}), tarfile.TarError),
            'tag lookup failed': (VERSION, lambda: self.tags.update({VERSION: None}), subprocess.CalledProcessError),
        }
        for case, (version, break_upstream, error) in cases.items():
            with self.subTest(case):
                self.setUp()
                break_upstream()
                with self.assertRaises(error) as raised:
                    self.retain_sources(version)
                if error is ValueError:
                    self.assertIn('Checksum mismatch', str(raised.exception))
                self.assertEqual(self.stdout, '')
                self.assertNotIn('WARNING', self.stderr)
                retained = [path.read_bytes() for path in self.output.rglob('*') if path.is_file()]
                self.assertNotIn(b'x', retained)

    def test_requested_release_must_be_a_plain_version(self):
        for version in ['latest', '1.2', '0.156.1 --tag x', '../0.156.1']:
            with self.subTest(version), self.assertRaises(SystemExit):
                self.retain_sources(version)
            self.assertEqual(self.git, [])


if __name__ == '__main__':
    unittest.main()
