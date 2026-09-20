"""Transfer committed objects with both Git object formats, including submodules."""
import hashlib
import json
from pathlib import Path
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

    def execute(self, name, workspace, *args):
        script = workspace.parent / name
        script.write_text((SCRIPTS / name).read_text().replace('/workspace', str(workspace)))
        command = ['bash' if name == 'horizon-worker-import' else 'python3', str(script), *args]
        return subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

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
                source = root / 'transfer'
                source.mkdir()
                (source / 'manifest.json').write_text(json.dumps({'modules':[{'path':'nested/module','revision':revision}],'assets':[]}))
                (source / 'module-0.pack').write_bytes(pack)
                with tarfile.open(workspace / 'horizon-source.tar', 'w') as archive:
                    for path in source.iterdir(): archive.add(path, arcname=path.name)
                imported = self.execute('horizon-worker-source', workspace, 'import')
                self.assertEqual(imported.returncode, 0, imported.stderr.decode())
                agent = workspace / 'agents' / 'isolated'
                agent.mkdir(parents=True)
                checked = self.execute('horizon-worker-source', workspace, 'checkout', str(agent))
                self.assertEqual(checked.returncode, 0, checked.stderr.decode())
                self.assertEqual((agent / 'nested/module/file.txt').read_text(), 'selected committed content\n')
                self.assertEqual(self.git('-C', agent / 'nested/module', 'rev-parse', 'HEAD').decode().strip(), revision)

if __name__ == '__main__': unittest.main()
