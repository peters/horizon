"""The Tailscale network of the person is not in the namespace of the fixture.

Horizon lists the machines of Tailscale in the overlay of the remote hosts, so a
recording of a fixture could show the private network of the computer that ran it.
"""
from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import sandbox  # noqa: E402


class TailscaleHiddenTest(unittest.TestCase):
    def setUp(self):
        self.work = Path(tempfile.mkdtemp()).resolve()
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)
        self.state = self.work / 'state'
        self.state.mkdir()
        self.bin = self.work / 'bin'
        self.bin.mkdir()
        self.program = self.bin / sandbox.TAILSCALE_PROGRAM
        self.program.write_text('#!/bin/sh\necho \'{"Peer": {"a": {"HostName": "a real machine"}}}\'\n')
        self.program.chmod(0o755)
        self.socket_dir = self.work / 'run' / 'tailscale'
        self.socket_dir.mkdir(parents=True)
        (self.socket_dir / 'tailscaled.sock').write_text('the socket of the person')
        self.environ = {'PATH': str(self.bin), 'HOME': str(Path.home())}

    def prepare(self):
        with mock.patch.object(sandbox, 'TAILSCALE_SOCKET_DIRS', (str(self.socket_dir), str(self.work / 'absent'))):
            return sandbox.prepare(self.state, environ=self.environ)

    def mounts(self, box):
        return list(zip(box.namespace, box.namespace[1:], box.namespace[2:]))

    def test_the_socket_folder_and_the_program_are_replaced(self):
        box = self.prepare()
        stand_in = box.data / sandbox.NO_TAILSCALE_NAME

        pairs = list(zip(box.namespace, box.namespace[1:]))
        self.assertIn(('--tmpfs', str(self.socket_dir)), pairs, 'an empty folder hides the local socket')
        self.assertNotIn(('--tmpfs', str(self.work / 'absent')), pairs, 'a folder that is not there gets no mount')
        self.assertIn(('--ro-bind', str(stand_in), str(self.program)), self.mounts(box),
                      'a program of no network takes the place of tailscale')
        self.assertTrue(os.access(stand_in, os.X_OK))
        answer = subprocess.run([str(stand_in), 'status', '--json'], capture_output=True, text=True, timeout=30)
        self.assertEqual((answer.returncode, answer.stdout.strip()), (0, '{}'), 'the network has no machine')

    def test_a_link_to_a_program_with_another_name_is_not_replaced(self):
        launcher = self.bin / 'launcher'
        launcher.write_text('#!/bin/sh\n')
        launcher.chmod(0o755)
        self.program.unlink()
        self.program.symlink_to(launcher)

        self.assertEqual(sandbox.tailscale_programs(self.environ), [],
                         'a file in the place of a launcher would replace it for every program')
        self.assertNotIn(str(launcher), self.prepare().namespace)

    def test_the_candidate_reads_a_network_with_no_machine(self):
        if not shutil.which('bwrap'):
            self.skipTest('needs bwrap')
        box = self.prepare()
        supports_namespace = subprocess.run([*box.namespace, 'true'], capture_output=True, text=True, timeout=60)
        if supports_namespace.returncode != 0:
            self.skipTest('this machine cannot create the user namespace that the fixture uses: '
                          + supports_namespace.stderr.strip().splitlines()[-1])

        status = subprocess.run([*box.namespace, str(self.program), 'status', '--json'],
                                capture_output=True, text=True, timeout=60)
        listed = subprocess.run([*box.namespace, 'ls', '-A', str(self.socket_dir)],
                                capture_output=True, text=True, timeout=60)

        self.assertEqual((status.returncode, status.stdout.strip()), (0, '{}'), status.stderr)
        self.assertEqual((listed.returncode, listed.stdout.strip()), (0, ''), listed.stderr)


if __name__ == '__main__':
    unittest.main()
