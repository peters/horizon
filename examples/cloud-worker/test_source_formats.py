"""Transfer committed objects with both Git object formats, including submodules and siblings."""
import hashlib
import json
import os
from pathlib import Path
import runpy
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest import mock

SCRIPTS = Path(__file__).parent
# Without an init.defaultBranch setting, as on a fresh worker.
UNCONFIGURED_GIT = dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM='1')

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

    def archive(self, root, workspace, modules, packs, assets=(), alias=None, lfs=None):
        source = root / 'transfer'
        shutil.rmtree(source, ignore_errors=True)
        (source / 'lfs').mkdir(parents=True)
        value = {'modules': modules, 'assets': [
            {'path': path, 'oid': hashlib.sha256(content).hexdigest(), 'size': len(content)} for path, content in assets]}
        if lfs is not None:
            value['lfs'] = lfs
        (source / 'manifest.json').write_text(json.dumps(value))
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
                    result = self.execute('horizon-worker-import', workspace, revision, env=UNCONFIGURED_GIT)
                    self.assertEqual(result.returncode, 0, result.stderr.decode())
                    # No Git advice (such as the initial branch hint) reaches deployment logs.
                    self.assertEqual(result.stderr, b'')
                self.assertEqual(self.git('--git-dir', workspace / 'repository.git', 'rev-parse', '--show-object-format').decode().strip(), object_format)
                self.assertEqual(self.git('--git-dir', workspace / 'repository.git', 'show', revision+':file.txt'), b'selected committed content\n')

    def test_submodule_archive_import_and_checkout_preserve_objects(self):
        for object_format in ['sha1', 'sha256']:
            with self.subTest(object_format=object_format), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                revision, pack, workspace = self.fixture(root, object_format)
                self.archive(root, workspace, [{'path': 'nested/module', 'revision': revision}], [pack])
                imported = self.execute('horizon-worker-source', workspace, 'import', env=UNCONFIGURED_GIT)
                self.assertEqual(imported.returncode, 0, imported.stderr.decode())
                self.assertEqual(imported.stderr, b'')
                agent = workspace / 'agents' / 'isolated'
                agent.mkdir(parents=True)
                checked = self.execute('horizon-worker-source', workspace, 'checkout', str(agent))
                self.assertEqual(checked.returncode, 0, checked.stderr.decode())
                # A raw gitlink without a .gitmodules mapping still checks out; the log names it.
                self.assertIn(b'nested/module has no .gitmodules mapping', checked.stderr)
                self.assertEqual((agent / 'nested/module/file.txt').read_text(), 'selected committed content\n')
                self.assertEqual(self.git('-C', agent / 'nested/module', 'rev-parse', 'HEAD').decode().strip(), revision)

    def test_pinned_submodule_pack_is_recorded_shallow_until_full_history_arrives(self):
        for object_format in ['sha1', 'sha256']:
            with self.subTest(object_format=object_format), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                _, _, workspace = self.fixture(root, object_format)
                local = root / 'local'
                (local / 'file.txt').write_text('pinned content\n')
                revision = self.commit_all(local, 'Pin')
                objects = self.git('-C', local, 'rev-list', '--objects', '--no-walk', revision)
                pinned = self.git('-C', local, 'pack-objects', '--stdout', input=objects)
                full = self.git('-C', local, 'pack-objects', '--stdout', '--revs', input=(revision + '\n').encode())
                module = workspace / 'source' / 'module-0.git'
                self.archive(root, workspace, [{'path': 'module', 'revision': revision}], [pinned])
                imported = self.execute('horizon-worker-source', workspace, 'import', env=UNCONFIGURED_GIT)
                self.assertEqual(imported.returncode, 0, imported.stderr.decode())
                self.assertEqual(imported.stderr, b'')
                self.assertEqual((module / 'shallow').read_text(), revision + '\n')
                agent = workspace / 'agents' / 'isolated'
                agent.mkdir(parents=True)
                checked = self.execute('horizon-worker-source', workspace, 'checkout', str(agent))
                self.assertEqual(checked.returncode, 0, checked.stderr.decode())
                self.assertEqual((agent / 'module/file.txt').read_text(), 'pinned content\n')
                self.assertEqual(self.git('-C', agent / 'module', 'rev-list', '--count', 'HEAD').strip(), b'1')
                self.git('-C', agent / 'module', 'fsck', '--no-dangling')
                # The parent commit alone, without its tree, is not full history.
                parent = self.git('-C', local, 'rev-parse', revision + '^').decode().strip()
                partial = self.git('-C', local, 'pack-objects', '--stdout', input=(parent + '\n').encode())
                self.archive(root, workspace, [{'path': 'module', 'revision': revision}], [partial])
                kept = self.execute('horizon-worker-source', workspace, 'import', env=UNCONFIGURED_GIT)
                self.assertEqual(kept.returncode, 0, kept.stderr.decode())
                self.assertEqual((module / 'shallow').read_text(), revision + '\n')
                self.assertEqual(self.git('--git-dir', module, 'rev-list', '--count', revision).strip(), b'1')
                # Full history replayed onto the same repository drops the shallow entry.
                self.archive(root, workspace, [{'path': 'module', 'revision': revision}], [full])
                replayed = self.execute('horizon-worker-source', workspace, 'import', env=UNCONFIGURED_GIT)
                self.assertEqual(replayed.returncode, 0, replayed.stderr.decode())
                self.assertFalse((module / 'shallow').exists())
                self.assertEqual(self.git('--git-dir', module, 'rev-list', '--count', revision).strip(), b'2')

    def test_shallow_state_is_durable_before_the_manifest_is_published(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _, _, workspace = self.fixture(root, 'sha1')
            local = root / 'local'
            (local / 'file.txt').write_text('pinned content\n')
            revision = self.commit_all(local, 'Pin')
            objects = self.git('-C', local, 'rev-list', '--objects', '--no-walk', revision)
            pinned = self.git('-C', local, 'pack-objects', '--stdout', input=objects)
            self.archive(root, workspace, [{'path': 'module', 'revision': revision}], [pinned])
            script = root / 'horizon-worker-source'
            script.write_text((SCRIPTS / 'horizon-worker-source').read_text().replace('/workspace', str(workspace)))
            helper = runpy.run_path(str(script), run_name='horizon_worker_source')
            module = workspace / 'source' / 'module-0.git'
            events = []
            real_replace, real_fsync = os.replace, os.fsync
            def replace(source, destination):
                events.append(('replace', Path(destination).name))
                return real_replace(source, destination)
            def fsync(descriptor):
                events.append(('fsync', Path(os.readlink(f'/proc/self/fd/{descriptor}'))))
                return real_fsync(descriptor)
            with mock.patch('os.replace', side_effect=replace), mock.patch('os.fsync', side_effect=fsync):
                helper['import_source'](workspace / 'source', workspace / 'horizon-source.tar')
            self.assertEqual((module / 'shallow').read_text(), revision + '\n')
            shallow = events.index(('replace', 'shallow'))
            published = events.index(('replace', 'manifest.json'))
            self.assertIn(('fsync', module / 'shallow.new'), events[:shallow])
            self.assertIn(('fsync', module), events[shallow:published])

    def test_submodule_pack_without_the_pinned_tree_is_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            revision, _, workspace = self.fixture(root, 'sha1')
            commit_only = self.git('-C', root / 'local', 'pack-objects', '--stdout', input=(revision + '\n').encode())
            self.archive(root, workspace, [{'path': 'module', 'revision': revision}], [commit_only])
            refused = self.execute('horizon-worker-source', workspace, 'import', env=UNCONFIGURED_GIT)
            self.assertNotEqual(refused.returncode, 0)
            self.assertFalse((workspace / 'source' / 'manifest.json').exists())

    def test_source_helper_reports_its_shallow_contract(self):
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory) / 'workspace'
            reply = self.execute('horizon-worker-source', workspace, '--shallow-contract')
            self.assertEqual((reply.returncode, reply.stdout), (0, b'horizon-source-shallow-contract=1\n'))
            self.assertFalse(workspace.exists(), 'the probe takes no lock and writes nothing')

    def commit_all(self, repository, message):
        self.git('-C', repository, 'add', '-A')
        self.git('-C', repository, '-c', 'user.name=Smoke', '-c', 'user.email=smoke@example.invalid', 'commit', '-m', message)
        return self.git('-C', repository, 'rev-parse', 'HEAD').decode().strip()

    def add_submodule(self, superproject, name, source, path):
        # A name unlike the path, so registration must read it from .gitmodules.
        self.git('-C', superproject, '-c', 'protocol.file.allow=always', 'submodule', 'add', '--quiet',
                 '--name', name, source, path)

    def test_shared_checkout_registers_nested_submodules_by_name_and_stays_offline(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            leaf_revision, leaf_pack, workspace = self.fixture(root, 'sha1')
            middle = root / 'middle'
            self.git('init', middle)
            self.add_submodule(middle, 'leaf-lib', root / 'local', 'deps/leaf')
            middle_revision = self.commit_all(middle, 'Add leaf')
            middle_pack = self.git('-C', middle, 'pack-objects', '--stdout', '--revs', input=(middle_revision + '\n').encode())
            superproject = root / 'superproject'
            self.git('init', superproject)
            self.add_submodule(superproject, 'middle-lib', middle, 'nested/module')
            self.commit_all(superproject, 'Add middle')
            self.git('clone', '--bare', superproject, workspace / 'repository.git')
            checkout = workspace / 'checkout'
            self.git('--git-dir', workspace / 'repository.git', 'worktree', 'add', '--detach', checkout, 'HEAD')
            self.archive(root, workspace, [{'path': 'nested/module', 'revision': middle_revision},
                                           {'path': 'nested/module/deps/leaf', 'revision': leaf_revision}],
                         [middle_pack, leaf_pack])
            self.assertEqual(self.execute('horizon-worker-source', workspace, 'import', env=UNCONFIGURED_GIT).returncode, 0)
            # Every original remote is gone, so any clone or fetch below would fail.
            for original in ['local', 'middle', 'superproject']:
                shutil.rmtree(root / original)
            for _ in range(2):
                checked = self.execute('horizon-worker-source', workspace, 'checkout', str(checkout))
                self.assertEqual(checked.returncode, 0, checked.stderr.decode())
            config = lambda repository, key: self.git('-C', repository, 'config', '--get', key).decode().strip()
            material = workspace / 'source'
            self.assertEqual(config(checkout, 'submodule.middle-lib.url'), str(material / 'module-0.git'))
            self.assertEqual(config(checkout / 'nested/module', 'submodule.leaf-lib.url'), str(material / 'module-1.git'))
            # The child registers in its enclosing module, never in the top-level superproject.
            self.assertNotIn(b'leaf-lib', self.git('--git-dir', workspace / 'repository.git', 'config', '--list'))
            status = self.git('-C', checkout, 'submodule', 'status', '--recursive').decode().splitlines()
            self.assertEqual([line.split()[:2] for line in status],
                             [[middle_revision, 'nested/module'], [leaf_revision, 'nested/module/deps/leaf']])
            self.assertTrue(all(line.startswith(' ') for line in status), status)
            self.git('-C', checkout, 'submodule', 'update', '--init', '--recursive')
            self.assertEqual((checkout / 'nested/module/deps/leaf/file.txt').read_text(), 'selected committed content\n')

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


    @unittest.skipUnless(shutil.which('git-lfs'), 'LFS selection needs git-lfs')
    def test_lfs_selection_leaves_only_paths_git_lfs_excludes_as_pointers(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            clean = root / 'clean.gitconfig'
            clean.touch()
            plain = dict(os.environ, GIT_CONFIG_GLOBAL=str(clean), GIT_CONFIG_NOSYSTEM='1')
            contents = {'keep.bin': b'kept binary content\n', 'fixtures/skip.bin': b'skipped binary content\n'}
            pointer = lambda content: (f'version https://git-lfs.github.com/spec/v1\noid sha256:{hashlib.sha256(content).hexdigest()}'
                                       f'\nsize {len(content)}\n')
            local = root / 'local'
            subprocess.run(['git', 'init', '--quiet', local], check=True, env=plain)
            (local / 'fixtures').mkdir()
            (local / '.gitattributes').write_text('*.bin filter=lfs diff=lfs merge=lfs -text\n')
            for path, content in contents.items():
                (local / path).write_text(pointer(content))
            subprocess.run(['git', '-C', local, 'add', '.'], check=True, env=plain)
            subprocess.run(['git', '-C', local, '-c', 'user.name=Smoke', '-c', 'user.email=smoke@example.invalid',
                            'commit', '--quiet', '-m', 'Add LFS fixtures'], check=True, env=plain)
            revision = subprocess.check_output(['git', '-C', local, 'rev-parse', 'HEAD'], env=plain, text=True).strip()
            pack = subprocess.check_output(['git', '-C', local, 'pack-objects', '--stdout', '--revs'],
                                           input=(revision + '\n').encode(), env=plain)
            workspace = root / 'workspace'
            workspace.mkdir()
            worker_config = root / 'worker.gitconfig'
            worker_config.write_text('[filter "lfs"]\n\tclean = git-lfs clean -- %f\n\tsmudge = git-lfs smudge -- %f\n'
                                     '\tprocess = git-lfs filter-process\n\trequired = true\n'
                                     f'[lfs]\n\tstorage = {workspace}/source/lfs\n')
            worker = dict(os.environ, GIT_CONFIG_GLOBAL=str(worker_config), GIT_CONFIG_NOSYSTEM='1')
            (workspace / 'horizon-transfer.pack').write_bytes(pack)
            self.assertEqual(self.execute('horizon-worker-import', workspace, revision, env=worker).returncode, 0)
            skipped = contents['fixtures/skip.bin']
            skipped = [{'path': 'fixtures/skip.bin', 'oid': hashlib.sha256(skipped).hexdigest(), 'size': len(skipped)}]
            kept = [('keep.bin', contents['keep.bin'])]
            refusals = [({'exclude': ['other/**'], 'skipped': skipped}, kept),  # git-lfs would smudge it
                        ({'exclude': ['fixtures/**'], 'skipped': skipped}, kept + [('fixtures/skip.bin', contents['fixtures/skip.bin'])]),
                        ({'exclude': ['fixtures/**,keep.bin'], 'skipped': skipped}, kept),
                        ({'exclude': ['fixtures/**'], 'skipped': skipped, 'unknown': []}, kept),
                        # Not lists: a string would be joined character by character.
                        ({'exclude': 'fixtures/**', 'skipped': skipped}, kept),
                        ({'exclude': ['fixtures/**'], 'skipped': skipped[0]}, kept),
                        ({'exclude': ['fixtures/**'], 'skipped': [dict(skipped[0], size='23')]}, kept),
                        # DEL, a C1 control and a format character.
                        ({'exclude': ['fixtures/**', 'a\x7fb'], 'skipped': skipped}, kept),
                        ({'exclude': ['fixtures/**', 'a\x85b'], 'skipped': skipped}, kept),
                        ({'exclude': ['fixtures/**', 'a\u200bb'], 'skipped': skipped}, kept),
                        ({'exclude': [f'p{index}' for index in range(64)] + ['fixtures/**'], 'skipped': skipped}, kept),
                        ([], kept)]
            for lfs, assets in refusals:
                self.archive(root, workspace, [], [], assets, lfs=lfs)
                refused = self.execute('horizon-worker-source', workspace, 'import', env=worker)
                self.assertNotEqual(refused.returncode, 0, lfs)
                self.assertFalse((workspace / 'source' / 'manifest.json').exists(), lfs)
            self.archive(root, workspace, [], [], kept, lfs={'exclude': ['fixtures/**'], 'skipped': skipped})
            imported = self.execute('horizon-worker-source', workspace, 'import', env=worker)
            self.assertEqual(imported.returncode, 0, imported.stderr.decode())
            repository = workspace / 'repository.git'
            self.assertEqual(subprocess.check_output(['git', '--git-dir', repository, 'config', 'lfs.fetchexclude'],
                                                     env=worker, text=True).strip(), 'fixtures/**')
            agent = workspace / 'agents' / 'session-1'
            subprocess.run(['git', '--git-dir', repository, 'worktree', 'add', '--quiet', '--detach', agent, revision],
                           check=True, env=worker, capture_output=True)
            self.assertEqual((agent / 'keep.bin').read_bytes(), contents['keep.bin'])
            self.assertEqual((agent / 'fixtures/skip.bin').read_text(), pointer(contents['fixtures/skip.bin']))
            self.assertEqual(subprocess.check_output(['git', '-C', agent, 'status', '--porcelain'], env=worker), b'')

if __name__ == '__main__': unittest.main()
