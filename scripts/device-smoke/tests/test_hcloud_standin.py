"""The stand-in of the Hetzner CLI answers the calls of the rent flow.

The stand-in of the Set up procedure takes the place of `hcloud`, so a real
agent can walk the rent flow without money. These tests run a copy of it in a
temporary folder and check the calls of the skill `horizon-machines`: the
read-only calls of step 1 and step 2, the create, the describe, the list and the
delete. They also hold the two promises of the program: it makes nothing, and a
call that it does not answer is refused with one line instead of a guess.
"""
from __future__ import annotations

import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

STAND_IN = Path(__file__).resolve().parents[1] / 'hcloud-standin.sh'


def run(folder, *arguments):
    """One call of the stand-in, in a copy of it in `folder`."""
    return subprocess.run(
        ['/bin/sh', str(folder / 'hcloud'), *arguments],
        capture_output=True, text=True, check=False, timeout=30,
    )


class StandInTests(unittest.TestCase):
    def setUp(self):
        self.folder = Path(tempfile.mkdtemp(prefix='hcloud-standin-'))
        self.addCleanup(shutil.rmtree, self.folder, True)
        shutil.copy(STAND_IN, self.folder / 'hcloud')
        (self.folder / 'hcloud').chmod(0o755)

    def test_the_read_only_calls_answer_in_the_words_of_the_reference(self):
        version = run(self.folder, 'version')
        self.assertEqual(version.returncode, 0, version.stderr)
        self.assertEqual(version.stdout.strip(), 'hcloud 1.69.0')

        context = run(self.folder, 'context', 'active')
        self.assertEqual(context.stdout.strip(), 'stand-in')

        types = run(self.folder, 'server-type', 'list', '-o', 'noheader')
        self.assertEqual(types.returncode, 0, types.stderr)
        self.assertIn('cx22', types.stdout)
        self.assertIn('cx42', types.stdout)

        places = run(self.folder, 'location', 'list', '-o', 'noheader')
        self.assertIn('hel1', places.stdout)
        self.assertIn('fsn1', places.stdout)

        images = run(self.folder, 'image', 'list', '-o', 'noheader')
        self.assertIn('ubuntu-24.04', images.stdout)

        # A read carries no price, as the columns of the real command carry none.
        self.assertNotIn('EUR', types.stdout + places.stdout + images.stdout)

    def test_the_list_is_empty_before_a_create_and_after_a_delete(self):
        empty = run(self.folder, 'server', 'list', '--quiet')
        self.assertEqual(empty.returncode, 0, empty.stderr)
        self.assertNotIn('running', empty.stdout)

        created = run(self.folder, 'server', 'create', '--name', 'build-1',
                      '--type', 'cx22', '--image', 'ubuntu-24.04', '--location', 'hel1',
                      '--ssh-key', 'build-1-key', '--firewall', 'build-1-fw', '-o', 'json')
        self.assertEqual(created.returncode, 0, created.stderr)
        self.assertEqual(json.loads(created.stdout)['server']['status'], 'running')
        self.assertEqual(created.stdout.count('\n'), 1, 'one JSON object on the standard output')

        listed = run(self.folder, 'server', 'list', '--quiet')
        self.assertIn('build-1', listed.stdout)

        described = json.loads(run(self.folder, 'server', 'describe', 'build-1', '-o', 'json').stdout)
        server = described['server']
        self.assertEqual(server['name'], 'build-1')
        self.assertEqual(server['location']['name'], 'hel1')
        self.assertEqual(server['server_type']['name'], 'cx22')
        self.assertEqual(
            server['public_net']['ipv4']['ip'], '127.0.0.1',
            'the SSH step must fail at once, so the walk reaches its failure card',
        )
        self.assertEqual(server['created'], '2026-01-01T09:40:00+00:00')

        deleted = run(self.folder, 'server', 'delete', 'build-1')
        self.assertEqual(deleted.returncode, 0, deleted.stderr)
        self.assertNotIn('build-1', run(self.folder, 'server', 'list', '--quiet').stdout)

    def test_a_describe_or_delete_without_a_machine_is_refused(self):
        for arguments in (('server', 'describe', '-o', 'json'), ('server', 'delete', 'build-1')):
            refused = run(self.folder, *arguments)
            self.assertEqual(refused.returncode, 1, arguments)
            self.assertIn('no machine', refused.stderr)
            self.assertEqual(refused.stdout, '')

    def test_a_call_outside_the_stand_in_is_refused_with_one_line(self):
        refused = run(self.folder, 'volume', 'create', '--name', 'data')
        self.assertEqual(refused.returncode, 1)
        self.assertEqual(len(refused.stderr.strip().splitlines()), 1, refused.stderr)
        self.assertIn('not part of the stand-in', refused.stderr)

    def test_the_program_writes_one_file_beside_itself_and_makes_nothing(self):
        before = set(self.folder.iterdir())
        run(self.folder, 'server', 'create', '--name', 'build-1')
        after = set(self.folder.iterdir())
        self.assertEqual(
            sorted(path.name for path in after - before), ['hcloud-standin.record'],
            'the stand-in keeps its one machine in one file beside itself',
        )
        self.assertEqual((self.folder / 'hcloud-standin.record').read_text().strip(), 'build-1')
        self.assertNotIn('build-1', run(self.folder, 'volume', 'create').stderr)

    def test_the_create_needs_a_name(self):
        refused = run(self.folder, 'server', 'create', '--type', 'cx22')
        self.assertEqual(refused.returncode, 1)
        self.assertIn('create needs --name', refused.stderr)

    def test_the_program_is_a_posix_shell_script_with_no_network_call(self):
        body = STAND_IN.read_text()
        self.assertTrue(body.startswith('#!/bin/sh\n'), 'the copy runs as /bin/sh')
        code = '\n'.join(line for line in body.splitlines() if not line.lstrip().startswith('#'))
        for forbidden in ('curl', 'wget', 'scp', 'ssh -', 'nc ', 'python'):
            self.assertNotIn(forbidden, code, f'the stand-in must not reach {forbidden}')


if __name__ == '__main__':
    unittest.main()
