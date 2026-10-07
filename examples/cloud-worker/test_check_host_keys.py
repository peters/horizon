"""The guard that keeps SSH host keys out of every layer of a published worker image."""
import gzip
import importlib.machinery
import importlib.util
import io
import json
from pathlib import Path
import re
import tarfile
import unittest

loader = importlib.machinery.SourceFileLoader('worker_host_keys', str(Path(__file__).with_name('check-host-keys.py')))
spec = importlib.util.spec_from_loader(loader.name, loader)
host_keys = importlib.util.module_from_spec(spec)
loader.exec_module(host_keys)


def tar_bytes(files, links=None):
    """A tar archive of `files`, a mapping of path to content, and of symbolic `links`."""
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode='w') as tar:
        for name, content in files.items():
            info = tarfile.TarInfo(name)
            info.size = len(content)
            tar.addfile(info, io.BytesIO(content))
        for name, target in (links or {}).items():
            info = tarfile.TarInfo(name)
            info.type = tarfile.SYMTYPE
            info.linkname = target
            tar.addfile(info)
    return buffer.getvalue()


def saved_image(layers, listed=None):
    """A `docker save` archive in OCI layout with `layers` as blobs and a config blob."""
    blobs = {f'blobs/sha256/{index:064x}': layer for index, layer in enumerate(layers, 1)}
    config = json.dumps({'architecture': 'amd64', 'rootfs': {'type': 'layers'}}).encode() * 20
    manifest = [{'Config': 'blobs/sha256/' + 'c' * 64, 'Layers': list(blobs) if listed is None else listed}]
    return io.BytesIO(tar_bytes({
        'oci-layout': b'{"imageLayoutVersion": "1.0.0"}',
        **blobs,
        'blobs/sha256/' + 'c' * 64: config,
        'manifest.json': json.dumps(manifest).encode(),
    }))


KEY = b'synthetic host key material'


class HostKeyTests(unittest.TestCase):
    def test_a_clean_image_has_no_host_keys(self):
        image = saved_image([tar_bytes({'etc/ssh/sshd_config': b'Port 22\n', 'usr/bin/ssh': b'binary'})])
        self.assertEqual(host_keys.image_host_keys(image), {})

    def test_a_key_removed_by_a_later_layer_is_still_found(self):
        first = tar_bytes({'etc/ssh/ssh_host_ed25519_key': KEY, 'etc/ssh/ssh_host_ed25519_key.pub': KEY})
        later = tar_bytes({'etc/ssh/.wh.ssh_host_ed25519_key': b'', 'etc/ssh/.wh.ssh_host_ed25519_key.pub': b''})
        found = host_keys.image_host_keys(saved_image([first, later]))
        self.assertEqual(list(found.values()),
                         [['etc/ssh/ssh_host_ed25519_key', 'etc/ssh/ssh_host_ed25519_key.pub']])

    def test_compressed_layers_and_relative_paths_are_read(self):
        layer = gzip.compress(tar_bytes({'./etc/ssh/ssh_host_rsa_key': KEY}))
        found = host_keys.image_host_keys(saved_image([layer]))
        self.assertEqual(list(found.values()), [['./etc/ssh/ssh_host_rsa_key']])

    def test_similar_names_elsewhere_are_not_host_keys(self):
        layer = tar_bytes({'usr/share/doc/etc/ssh/notes': b'', 'etc/ssh/ssh_config': b'', 'etc/ssh/sshd_config.d/x': b''})
        self.assertEqual(host_keys.image_host_keys(saved_image([layer])), {})

    def test_a_listed_layer_that_cannot_be_read_fails(self):
        image = saved_image([b'not a layer' * 100])
        with self.assertRaisesRegex(ValueError, 'could not be read'):
            host_keys.image_host_keys(image)

    def test_a_malformed_or_truncated_layer_is_never_clean(self):
        layer = tar_bytes({'usr/bin/tool': b'x' * 600, 'etc/ssh/ssh_host_rsa_key': KEY})
        corrupt = layer[:1536] + b'\xff' * 512 + layer[2048:]
        for broken in (corrupt, layer[:1536]):
            with self.assertRaisesRegex(ValueError, 'could not be read'):
                host_keys.image_host_keys(saved_image([broken]))

    def test_a_layer_saved_as_a_link_to_another_counts_as_read(self):
        first = tar_bytes({'etc/ssh/ssh_host_ed25519_key': KEY})
        legacy = tar_bytes(
            {'a/layer.tar': first, 'manifest.json': json.dumps([{'Layers': ['a/layer.tar', 'b/layer.tar']}]).encode()},
            links={'b/layer.tar': '../a/layer.tar'})
        found = host_keys.image_host_keys(io.BytesIO(legacy))
        self.assertEqual(sorted(found), ['a/layer.tar', 'b/layer.tar'])

    def test_an_archive_without_a_manifest_fails(self):
        with self.assertRaisesRegex(ValueError, 'manifest'):
            host_keys.image_host_keys(io.BytesIO(tar_bytes({'blobs/sha256/' + 'a' * 64: tar_bytes({})})))

    def test_the_recipe_removes_host_keys_in_the_layer_that_installs_the_server(self):
        recipe = Path(__file__).with_name('Dockerfile').read_text()
        instructions = re.split(r'\n(?=[A-Z]+ )', recipe.replace('\\\n', ' '))
        installs = [line for line in instructions if line.startswith('RUN ') and 'openssh-server' in line]
        self.assertEqual(len(installs), 1)
        self.assertIn('rm -f /etc/ssh/ssh_host_*', installs[0])


if __name__ == '__main__':
    unittest.main()
