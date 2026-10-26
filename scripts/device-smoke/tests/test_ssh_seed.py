"""The SSH configuration and identity that the fixture seeds into its private home (#1852)."""
from __future__ import annotations

import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import sandbox  # noqa: E402

SERVE = Path(__file__).resolve().parents[1] / 'serve.py'
CONFIG = """Host browser-vm
  HostName 127.0.0.1
  Port 2225
  User runner
  IdentityFile ~/.ssh/fixture_key
  StrictHostKeyChecking accept-new
"""
KEY = """-----BEGIN OPENSSH PRIVATE KEY-----
fixture identity, not a key of a person
-----END OPENSSH PRIVATE KEY-----
"""


class SeedSshTest(unittest.TestCase):
    def setUp(self):
        work = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, work, ignore_errors=True)
        self.home = work / 'data' / 'home'
        self.home.mkdir(mode=0o700, parents=True)
        self.source = work / 'sources'
        self.source.mkdir()
        self.config = self.source / 'config'
        self.config.write_text(CONFIG)
        self.identity = self.source / 'fixture_key'
        self.identity.write_text(KEY)
        os.chmod(self.identity, 0o644)
        self.state = work / 'state'
        self.state.mkdir()
        self.refused_state = work / 'refused-state'

    def run_fixture(self, *arguments):
        return subprocess.run(
            [sys.executable, str(SERVE), '--horizon', sys.executable, '--state', str(self.refused_state), *arguments],
            capture_output=True, text=True, timeout=120)

    def test_the_config_and_the_identity_land_private_in_the_home_of_the_fixture(self):
        seeded = sandbox.seed_ssh(self.home, self.config, self.identity)

        self.assertEqual(seeded, ['config', 'fixture_key'])
        folder = self.home / '.ssh'
        self.assertEqual(stat.S_IMODE(folder.stat().st_mode), 0o700, 'ssh requires a private folder')
        for name, text in (('config', CONFIG), ('fixture_key', KEY)):
            target = folder / name
            self.assertEqual(target.read_text(), text, name)
            self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o600, f'{name} keeps a private mode')

    def test_a_fixture_without_an_ssh_source_seeds_nothing(self):
        self.assertEqual(sandbox.seed_ssh(self.home), [])
        self.assertFalse((self.home / '.ssh').exists(), 'no configuration is invented for a local board')

    def test_one_source_alone_is_refused(self):
        state = self.run_fixture('--ssh-config', str(self.config))

        self.assertEqual(state.returncode, 2, state.stdout + state.stderr)
        self.assertIn('seeded together, or not at all', state.stderr)
        self.assertFalse(self.refused_state.exists(), 'a refused source creates no fixture state')

    def test_a_source_that_is_not_a_regular_file_is_refused(self):
        state = self.run_fixture('--ssh-config', str(self.source), '--ssh-identity', str(self.identity))

        self.assertEqual(state.returncode, 2, state.stdout + state.stderr)
        self.assertIn('--ssh-config must name a regular file that exists', state.stderr)
        self.assertFalse(self.refused_state.exists(), 'a refused source creates no fixture state')

    def test_an_identity_called_config_can_not_replace_the_configuration(self):
        identity = self.source / 'config'
        identity.write_text(KEY)

        with self.assertRaises(ValueError) as refused:
            sandbox.seed_ssh(self.home, self.config, identity)
        self.assertIn('must not be named config', str(refused.exception))

    def test_the_namespace_hides_the_system_include_that_ssh_refuses(self):
        box = sandbox.prepare(self.state)

        mounts = list(zip(box.namespace, box.namespace[1:]))
        self.assertEqual(
            ('--tmpfs', sandbox.SSH_SYSTEM_INCLUDE) in mounts,
            Path(sandbox.SSH_SYSTEM_INCLUDE).is_dir(),
            'a user namespace shows a root-owned file as nobody, and ssh then refuses it')

    def test_ssh_of_the_namespace_reaches_the_configuration_of_its_home(self):
        if not shutil.which('bwrap') or not Path(sandbox.SSH_SYSTEM_INCLUDE).is_dir():
            self.skipTest('needs bwrap and the system SSH include of this machine')
        if not shutil.which('ssh'):
            self.skipTest('needs the ssh client of this machine')
        box = sandbox.prepare(self.state)
        sandbox.seed_ssh(box.private_home, self.config, self.identity)

        supports_namespace = subprocess.run(
            [*box.namespace, 'true'], capture_output=True, text=True, timeout=60)
        if supports_namespace.returncode != 0:
            self.skipTest(
                'this machine cannot create the user namespace that the fixture uses: '
                + supports_namespace.stderr.strip().splitlines()[-1])

        result = subprocess.run(
            [*box.namespace, 'ssh', '-G', 'browser-vm'],
            capture_output=True, text=True, timeout=60)

        self.assertNotIn('Bad owner or permissions', result.stderr)
        self.assertEqual(result.returncode, 0, result.stderr)
        # Only the seeded configuration can supply these values: the alias has no
        # DNS record, and the port is not a default of the client.
        self.assertIn('hostname 127.0.0.1', result.stdout,
                      'ssh did not read the seeded configuration of its private home')
        self.assertIn('port 2225', result.stdout,
                      'ssh did not read the seeded configuration of its private home')
        self.assertIn('user runner', result.stdout,
                      'ssh did not read the seeded configuration of its private home')


if __name__ == '__main__':
    unittest.main()
