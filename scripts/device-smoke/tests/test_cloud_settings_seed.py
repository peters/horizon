"""The cloud settings of a procedure reach the private home of the fixture."""
import stat
import tempfile
import unittest
from pathlib import Path

import sys

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import sandbox  # noqa: E402


class SeedCloudSettingsTest(unittest.TestCase):
    def test_a_folder_of_settings_reaches_the_private_home_privately(self):
        with tempfile.TemporaryDirectory() as source_root, tempfile.TemporaryDirectory() as home:
            source = Path(source_root)
            (source / 'settings.json').write_text('{"hetzner": {}}')
            (source / 'credentials').mkdir()
            (source / 'credentials' / 'token').write_text('secret')
            (source / 'identity').mkdir()
            (source / 'identity' / 'ed25519').write_text('key')

            seeded = sandbox.seed_cloud_settings(home, source)

            self.assertEqual(3, seeded, 'every file of the folder is copied')
            target = Path(home) / '.horizon' / 'cloud'
            self.assertEqual('secret', (target / 'credentials' / 'token').read_text())
            self.assertEqual('key', (target / 'identity' / 'ed25519').read_text())
            for folder in [target, target / 'credentials', target / 'identity']:
                self.assertEqual(0o700, stat.S_IMODE(folder.stat().st_mode), folder)
            for name in ['settings.json', 'credentials/token', 'identity/ed25519']:
                mode = stat.S_IMODE((target / name).stat().st_mode)
                self.assertEqual(0o600, mode, name)

    def test_no_folder_seeds_nothing(self):
        with tempfile.TemporaryDirectory() as home:
            self.assertEqual(0, sandbox.seed_cloud_settings(home, None))
            self.assertFalse((Path(home) / '.horizon').exists(), 'no settings, no folder')

    def test_a_symlink_is_refused(self):
        with tempfile.TemporaryDirectory() as source_root, tempfile.TemporaryDirectory() as home:
            source = Path(source_root)
            (source / 'settings.json').write_text('{}')
            (source / 'elsewhere').symlink_to('/etc/hostname')
            with self.assertRaises(ValueError):
                sandbox.seed_cloud_settings(home, source)

    def test_a_path_that_is_not_a_folder_is_refused(self):
        with tempfile.TemporaryDirectory() as source_root, tempfile.TemporaryDirectory() as home:
            source = Path(source_root) / 'settings.json'
            source.write_text('{}')
            with self.assertRaises(ValueError):
                sandbox.seed_cloud_settings(home, source)


if __name__ == '__main__':
    unittest.main()
