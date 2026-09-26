"""Transfer committed objects with both Git object formats, including submodules and siblings."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest

SCRIPTS = Path(__file__).parent

class SourceFormatTests(unittest.TestCase):
    def git(self, *args, **kwargs):
        return subprocess.check_output(['git', *map(str, args)], stderr=subprocess.DEVNULL, **kwargs)

    def fixture(self, root, object_format):
        repository = root / 'local'
        self.git('init', '--object-format=' + object_format, repository)
        (repository / 'file.txt').write_text('selected committed content\n')
        self.git('-C', repository, 'add', 'file.txt')
        self.git('-C', repository, '-c', 'user.name=Smoke', '-c', 'user.email=smoke@example.invalid', 'commit', '-m', 'Add fixture')
        revision = self.git('-C', repository, 'rev-parse', 'HEAD').decode().strip()
        pack = self.git('-C', repository, 'pack-objects', '--stdout', '--revs', input=(revision+'\n').encode())
        workspace = root / 'workspace'
        workspace.mkdir()
        return revision, pack, workspace

    def execute(self, name, workspace, *args, stdin=b'', env=None):
        script = workspace.parent / name
        script.write_text((SCRIPTS / name).read_text().replace('/workspace', str(workspace)))
        command = ['bash' if name == 'horizon-worker-import' else 'python3', str(script), *args]
        return subprocess.run(command, input=stdin, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)

    def archive(self, root, workspace, modules, packs, assets=(), alias=None):
        source = root / 'transfer'
        shutil.rmtree(source, ignore_errors=True)
        (source / 'lfs').mkdir(parents=True)
        (source / 'manifest.json').write_text(json.dumps({'modules': modules, 'assets': [
            {'path': path, 'oid': hashlib.sha256(content).hexdigest(), 'size': len(content)} for path, content in assets]}))
        for index, pack in enumerate(packs):
            (source / f'module-{index}.pack').write_bytes(pack)
        for _, content in assets:
            (source / 'lfs' / hashlib.sha256(content).hexdigest()).write_bytes(content)
        target = workspace / 'siblings' / alias / 'horizon-source.tar' if alias else workspace / 'horizon-source.tar'
        target.parent.mkdir(parents=True, exist_ok=True)
        with tarfile.open(target, 'w') as archive:
            for path in source.iterdir(): archive.add(path, arcname=path.name)

    def test_main_import_and_replay_preserve_selected_object_format(self):
        for object_format in ['sha1', 'sha256']:
            with self.subTest(object_format=object_format), tempfile.TemporaryDirectory() as directory:
                revision, pack, workspace = self.fixture(Path(directory), object_format)
                for _ in range(2):
                    (workspace / 'horizon-transfer.pack').write_bytes(pack)
                    result = self.execute('horizon-worker-import', workspace, revision)
                    self.assertEqual(result.returncode, 0, result.stderr.decode())
                self.assertEqual(self.git('--git-dir', workspace / 'repository.git', 'rev-parse', '--show-object-format').decode().strip(), object_format)
                self.assertEqual(self.git('--git-dir', workspace / 'repository.git', 'show', revision+':file.txt'), b'selected committed content\n')

    def test_submodule_archive_import_and_checkout_preserve_objects(self):
        for object_format in ['sha1', 'sha256']:
            with self.subTest(object_format=object_format), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                revision, pack, workspace = self.fixture(root, object_format)
                self.archive(root, workspace, [{'path': 'nested/module', 'revision': revision}], [pack])
                imported = self.execute('horizon-worker-source', workspace, 'import')
                self.assertEqual(imported.returncode, 0, imported.stderr.decode())
                agent = workspace / 'agents' / 'isolated'
                agent.mkdir(parents=True)
                checked = self.execute('horizon-worker-source', workspace, 'checkout', str(agent))
                self.assertEqual(checked.returncode, 0, checked.stderr.decode())
                self.assertEqual((agent / 'nested/module/file.txt').read_text(), 'selected committed content\n')
                self.assertEqual(self.git('-C', agent / 'nested/module', 'rev-parse', 'HEAD').decode().strip(), revision)

    def import_sibling(self, workspace, alias, revision, pack, env=None):
        (workspace / 'siblings' / alias).mkdir(parents=True, exist_ok=True)
        (workspace / 'siblings' / alias / 'horizon-transfer.pack').write_bytes(pack)
        return self.execute('horizon-worker-import', workspace, revision, '--sibling', alias, env=env)

    def test_sibling_import_uses_its_own_repository_and_refuses_another_base(self):
        for object_format in ['sha1', 'sha256']:
            with self.subTest(object_format=object_format), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                revision, pack, workspace = self.fixture(root, object_format)
                repository = workspace / 'siblings' / 'native-lib' / 'repository.git'
                for _ in range(2):
                    result = self.import_sibling(workspace, 'native-lib', revision, pack)
                    self.assertEqual(result.returncode, 0, result.stderr.decode())
                self.assertFalse((workspace / 'repository.git').exists())
                self.assertEqual(self.git('--git-dir', repository, 'rev-parse', '--show-object-format').decode().strip(), object_format)
                self.assertEqual(self.git('--git-dir', repository, 'rev-parse', 'refs/heads/base').decode().strip(), revision)
                self.assertEqual(self.git('--git-dir', repository, 'config', 'lfs.storage').decode().strip(),
                                 str(workspace / 'siblings' / 'native-lib' / 'source' / 'lfs'))
                local = root / 'local'
                (local / 'file.txt').write_text('another revision\n')
                self.git('-C', local, '-c', 'user.name=Smoke', '-c', 'user.email=smoke@example.invalid', 'commit', '-am', 'Change')
                other = self.git('-C', local, 'rev-parse', 'HEAD').decode().strip()
                moved = self.import_sibling(workspace, 'native-lib', other,
                                            self.git('-C', local, 'pack-objects', '--stdout', '--revs', input=(other + '\n').encode()))
                self.assertEqual(moved.returncode, 3)
                self.assertIn(b'base revision mismatch', moved.stderr)
                self.assertEqual(self.git('--git-dir', repository, 'rev-parse', 'refs/heads/base').decode().strip(), revision)
                # The primary accepts its own base independently of the sibling's.
                (workspace / 'horizon-transfer.pack').write_bytes(pack)
                self.assertEqual(self.execute('horizon-worker-import', workspace, revision).returncode, 0)

    def test_sibling_uploads_never_touch_the_primary_staging_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            revision, pack, workspace = self.fixture(root, 'sha1')
            (workspace / 'horizon-transfer.pack').write_bytes(pack)
            self.archive(root, workspace, [{'path': 'module', 'revision': revision}], [pack])
            primary = {name: (workspace / name).read_bytes() for name in ['horizon-transfer.pack', 'horizon-source.tar']}
            self.assertEqual(self.import_sibling(workspace, 'lib', revision, pack).returncode, 0)
            self.archive(root, workspace, [], [], alias='lib')
            self.assertEqual(self.execute('horizon-worker-source', workspace, 'import', '--sibling', 'lib').returncode, 0)
            self.assertEqual([path.name for path in (workspace / 'siblings' / 'lib').iterdir() if path.name.startswith('horizon-')], [])
            self.assertEqual({name: (workspace / name).read_bytes() for name in primary}, primary)
            self.assertEqual(self.execute('horizon-worker-import', workspace, revision).returncode, 0)
            imported = self.execute('horizon-worker-source', workspace, 'import')
            self.assertEqual(imported.returncode, 0, imported.stderr.decode())
            self.assertEqual(json.loads((workspace / 'source' / 'manifest.json').read_text())['modules'][0]['path'], 'module')
            self.assertEqual(json.loads((workspace / 'siblings/lib/source/manifest.json').read_text())['modules'], [])

    def test_sibling_import_rejects_invalid_aliases_before_writing(self):
        with tempfile.TemporaryDirectory() as directory:
            revision, pack, workspace = self.fixture(Path(directory), 'sha1')
            (workspace / 'horizon-transfer.pack').write_bytes(pack)
            for alias in ['Lib', '1lib', '_lib', 'lib/x', '../lib', 'lib.x', 'a' * 65, '']:
                self.assertEqual(self.execute('horizon-worker-import', workspace, revision, '--sibling', alias).returncode, 2, alias)
            for args in [(revision, '--sibling'), (revision, '--other', 'lib'), (revision, '--sibling', 'lib', 'extra')]:
                self.assertEqual(self.execute('horizon-worker-import', workspace, *args).returncode, 2, args)
                self.assertNotEqual(self.execute('horizon-worker-source', workspace, 'import', *args[1:]).returncode, 0, args)
            self.assertNotEqual(self.execute('horizon-worker-source', workspace, 'import', '--sibling', '../lib').returncode, 0)
            self.assertFalse((workspace / 'siblings').exists())
            self.assertFalse((workspace / 'repository.git').exists())
            self.assertTrue((workspace / 'horizon-transfer.pack').exists())

    @unittest.skipUnless(shutil.which('git-lfs'), 'sibling LFS hydration needs git-lfs')
    def test_sibling_worktree_and_submodule_hydrate_lfs_from_the_sibling_store_only(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            clean = root / 'clean.gitconfig'
            clean.touch()
            plain = dict(os.environ, GIT_CONFIG_GLOBAL=str(clean), GIT_CONFIG_NOSYSTEM='1')
            content = b'sibling binary content\n'
            oid = hashlib.sha256(content).hexdigest()
            library = root / 'library'
            subprocess.run(['git', 'init', '--quiet', library], check=True, env=plain)
            (library / '.gitattributes').write_text('blob.bin filter=lfs diff=lfs merge=lfs -text\n')
            (library / 'blob.bin').write_text(f'version https://git-lfs.github.com/spec/v1\noid sha256:{oid}\nsize {len(content)}\n')
            subprocess.run(['git', '-C', library, 'add', '.'], check=True, env=plain)
            subprocess.run(['git', '-C', library, '-c', 'user.name=Smoke', '-c', 'user.email=smoke@example.invalid',
                            'commit', '--quiet', '-m', 'Add LFS fixture'], check=True, env=plain)
            revision = subprocess.check_output(['git', '-C', library, 'rev-parse', 'HEAD'], env=plain, text=True).strip()
            pack = subprocess.check_output(['git', '-C', library, 'pack-objects', '--stdout', '--revs'],
                                           input=(revision + '\n').encode(), env=plain)
            workspace = root / 'workspace'
            workspace.mkdir()
            # The worker's global configuration names the primary's store, which holds nothing.
            worker_config = root / 'worker.gitconfig'
            worker_config.write_text('[filter "lfs"]\n\tclean = git-lfs clean -- %f\n\tsmudge = git-lfs smudge -- %f\n'
                                     '\tprocess = git-lfs filter-process\n\trequired = true\n'
                                     f'[lfs]\n\tstorage = {workspace}/source/lfs\n')
            worker = dict(os.environ, GIT_CONFIG_GLOBAL=str(worker_config), GIT_CONFIG_NOSYSTEM='1')
            self.assertEqual(self.import_sibling(workspace, 'lib', revision, pack, env=worker).returncode, 0)
            self.archive(root, workspace, [{'path': 'nested/module', 'revision': revision}], [pack],
                         [('blob.bin', content), ('nested/module/blob.bin', content)], alias='lib')
            imported = self.execute('horizon-worker-source', workspace, 'import', '--sibling', 'lib', env=worker)
            self.assertEqual(imported.returncode, 0, imported.stderr.decode())
            self.assertFalse((workspace / 'source').exists())
            session = workspace / 'agents' / 'session-1'
            session.mkdir(parents=True)
            worktree = session / 'Library'
            subprocess.run(['git', '--git-dir', workspace / 'siblings/lib/repository.git', 'worktree', 'add', '--quiet',
                            '-b', 'agent/session-1', worktree, revision], check=True, env=worker, capture_output=True)
            self.assertEqual((worktree / 'blob.bin').read_bytes(), content)
            checked = self.execute('horizon-worker-source', workspace, 'checkout', str(worktree), '--sibling', 'lib', env=worker)
            self.assertEqual(checked.returncode, 0, checked.stderr.decode())
            module = worktree / 'nested/module'
            self.assertEqual((module / 'blob.bin').read_bytes(), content)
            self.assertEqual(subprocess.check_output(['git', '-C', module, 'config', 'lfs.storage'], env=worker, text=True).strip(),
                             str(workspace / 'siblings/lib/source/lfs'))
            self.assertEqual(subprocess.check_output(['git', '-C', module, 'remote', 'get-url', 'origin'], env=worker, text=True).strip(),
                             str(workspace / 'siblings/lib/source/module-0.git'))
            # Replay finishes an interrupted checkout in place.
            self.assertEqual(self.execute('horizon-worker-source', workspace, 'checkout', str(worktree), '--sibling', 'lib', env=worker).returncode, 0)

    def test_checkout_accepts_only_agent_worktrees_and_matching_material(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            revision, pack, workspace = self.fixture(root, 'sha1')
            self.assertEqual(self.import_sibling(workspace, 'lib', revision, pack).returncode, 0)
            self.archive(root, workspace, [], [], alias='lib')
            self.assertEqual(self.execute('horizon-worker-source', workspace, 'import', '--sibling', 'lib').returncode, 0)
            self.archive(root, workspace, [], [])
            self.assertEqual(self.execute('horizon-worker-source', workspace, 'import').returncode, 0)
            repository = workspace / 'siblings/lib/repository.git'
            single = workspace / 'agents' / 'single'
            single.mkdir(parents=True)
            nested = workspace / 'agents' / 'paired' / 'lib'
            (workspace / 'agents' / 'paired').mkdir()
            self.git('--git-dir', repository, 'worktree', 'add', nested, revision)
            (workspace / 'horizon-transfer.pack').write_bytes(pack)
            self.assertEqual(self.execute('horizon-worker-import', workspace, revision).returncode, 0)
            primary = workspace / 'agents' / 'paired' / 'app'
            self.git('--git-dir', workspace / 'repository.git', 'worktree', 'add', primary, revision)
            inside = single / 'sub'
            inside.mkdir()
            (inside / '.git').write_text('gitdir: elsewhere\n')
            (single / '.git').write_text('gitdir: elsewhere\n')
            accepted = [(single, ()), (primary, ()), (nested, ('--sibling', 'lib'))]
            # A nested worktree takes only its own repository's material.
            refused = [(nested, ()), (primary, ('--sibling', 'lib')),
                       (single, ('--sibling', 'lib')), (inside, ()), (inside, ('--sibling', 'lib')),
                       (workspace / 'agents', ()), (workspace / 'elsewhere' / 'x' / 'y', ()), (nested, ('--sibling', 'other')),
                       (nested, ('--sibling', 'Lib'))]
            for worktree, extra in accepted:
                result = self.execute('horizon-worker-source', workspace, 'checkout', str(worktree), *extra)
                self.assertEqual(result.returncode, 0, (worktree, extra, result.stderr.decode()))
            for worktree, extra in refused:
                self.assertNotEqual(self.execute('horizon-worker-source', workspace, 'checkout', str(worktree), *extra).returncode, 0,
                                    (worktree, extra))


if __name__ == '__main__': unittest.main()
