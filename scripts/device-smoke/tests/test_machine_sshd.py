"""The private machine of `--machine-sshd`: the real sshd of the fixture run.

`serve.py --machine-watch --machine-sshd <sshd>` runs the machine of the fixture as
one real `sshd` of the user of the run, inside a namespace of its own, so the
candidate uses the real `ssh` of this computer for the watch, for the card sockets,
for the removal and for a session. This test proves the parts of it that need no
namespace and no second computer:

  * the configuration that the sshd of the run reads, and the `ssh` configuration
    that the fixture seeds into the private home of the candidate;
  * the pieces of a split `sshd`, which a build keeps in `<prefix>/lib/openssh/`;
  * the one sentence of a run that lacks a program or names no sshd;
  * the folders, the two keys and the client file of a prepared machine;
  * the namespace and the stop list of the run.

What only a real run can show is in the README of the fixture and in the procedures
of the machine watch and of the machine agent cards.

Standard library only. Nothing is written outside the folder of a test.
"""
from __future__ import annotations

import json
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
import serve  # noqa: E402

SERVE = Path(__file__).resolve().parents[1] / 'serve.py'
PORT = 2225


def a_program(folder: Path) -> Path:
    """A stand-in of one program of the machine: a shell script that answers 0."""
    folder.parent.mkdir(parents=True, exist_ok=True)
    folder.write_text('#!/bin/sh\nexit 0\n', encoding='utf-8')
    folder.chmod(0o700)
    return folder


def an_sshd_prefix(root: Path) -> Path:
    """The pieces of an unpacked `sshd`: the program and its two session programs."""
    program = a_program(root / 'usr' / 'sbin' / 'sshd')
    for name in serve.MACHINE_SSHD_PROGRAMS:
        a_program(root / 'usr' / 'lib' / 'openssh' / name)
    return program


class OneRunOfTheFixture(unittest.TestCase):
    """A run of the fixture that its own arguments refuse."""

    def setUp(self):
        self.work = Path(tempfile.mkdtemp(prefix='hz-machine-sshd-'))
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)
        self.state = self.work / 'state'
        self.sshd = an_sshd_prefix(self.work / 'prefix')

    def run_fixture(self, *arguments):
        return subprocess.run(
            [sys.executable, str(SERVE), '--horizon', sys.executable, '--state', str(self.state),
             *arguments], capture_output=True, text=True, timeout=120)

    def test_the_option_makes_the_machine_of_the_watch_and_needs_it(self):
        result = self.run_fixture('--machine-sshd', str(self.sshd))

        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn('--machine-sshd makes the machine of --machine-watch a real sshd', result.stderr)
        self.assertFalse(self.state.exists(), 'a refused run creates no fixture state')

    def test_the_option_writes_the_ssh_configuration_of_its_own_machine(self):
        sources = self.work / 'sources'
        sources.mkdir()
        config, identity = sources / 'config', sources / 'key'
        for path in (config, identity):
            path.write_text('# of a procedure\n', encoding='utf-8')

        result = self.run_fixture('--machine-watch', '--machine-sshd', str(self.sshd),
                                  '--ssh-config', str(config), '--ssh-identity', str(identity))

        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn('writes the SSH configuration of its own machine', result.stderr)

    def test_a_path_that_is_no_file_is_refused_with_one_sentence(self):
        result = self.run_fixture('--machine-watch', '--machine-sshd', str(self.work / 'no-sshd'))

        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn('--machine-sshd must name the sshd program as a file that exists', result.stderr)
        self.assertFalse(self.state.exists(), 'a refused run creates no fixture state')


class ThePiecesOfAnSshd(unittest.TestCase):
    def setUp(self):
        self.work = Path(tempfile.mkdtemp(prefix='hz-machine-sshd-'))
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)
        self.state = self.work / 'state'
        self.state.mkdir()

    def test_the_session_programs_may_not_be_missing(self):
        sshd = a_program(self.work / 'prefix' / 'usr' / 'sbin' / 'sshd')

        sentence = serve.machine_sshd_requirement(sshd, which=lambda name: f'/usr/bin/{name}')

        self.assertIsNotNone(sentence)
        self.assertIn('sshd-session and sshd-auth in the prefix of the program', sentence)
        self.assertIn('sshd-auth', sentence)

    def test_a_computer_without_bwrap_says_so_in_one_sentence(self):
        sshd = an_sshd_prefix(self.work / 'prefix')

        sentence = serve.machine_sshd_requirement(
            sshd, which=lambda name: None if name == 'bwrap' else f'/usr/bin/{name}')

        self.assertEqual(sentence, '--machine-sshd needs bwrap on this computer, '
                                   'and this run finds no such program')

    def test_a_computer_that_has_every_piece_says_nothing(self):
        sshd = an_sshd_prefix(self.work / 'prefix')

        self.assertIsNone(serve.machine_sshd_requirement(sshd, which=lambda name: f'/usr/bin/{name}'))

    def test_the_port_of_the_machine_is_one_of_the_loopback_address(self):
        port = serve.free_loopback_port()

        self.assertIsInstance(port, int)
        self.assertGreater(port, 0)
        self.assertLess(port, 65536)


class TheConfigurationOfTheMachine(unittest.TestCase):
    def setUp(self):
        self.work = Path(tempfile.mkdtemp(prefix='hz-machine-sshd-'))
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)
        self.state = self.work / 'state'
        self.state.mkdir()
        self.machine = serve.MachineSshd(self.state, an_sshd_prefix(self.work / 'prefix'), port=PORT)

    def test_the_sshd_of_the_run_reads_every_piece_of_the_machine(self):
        text = serve.machine_sshd_config(self.machine)

        for line in ('Port 2225', 'ListenAddress 127.0.0.1', f'HostKey {self.machine.host_key}',
                     f'AuthorizedKeysFile {self.machine.authorized_keys}',
                     f'PidFile {self.machine.pid_file}', 'UsePAM no',
                     'PasswordAuthentication no', 'StrictModes no',
                     'AllowStreamLocalForwarding yes', 'StreamLocalBindUnlink no',
                     f'SshdSessionPath {self.machine.session}',
                     f'SshdAuthPath {self.machine.auth}',
                     f'SetEnv HORIZON_CARDS_DIR={serve.MACHINE_CARDS_DIR}', 'LogLevel VERBOSE'):
            self.assertIn(line + '\n', text)
        # The session programs lie beside the given program, in the prefix of the build.
        prefix = self.work / 'prefix' / 'usr' / 'lib' / 'openssh'
        self.assertEqual(self.machine.session, prefix / 'sshd-session')
        self.assertEqual(self.machine.auth, prefix / 'sshd-auth')
        # The folder of the cards is the /tmp of the machine itself, which is the path
        # that the forward of the candidate binds there and the relay reads.
        self.assertEqual(serve.MACHINE_CARDS_DIR, '/tmp/horizon-cards')

    def test_the_seeded_ssh_configuration_sends_the_alias_to_the_machine(self):
        text = serve.machine_sshd_ssh_config(self.machine)

        for line in (f'Host {serve.MACHINE_WATCH_MACHINE}', '  HostName 127.0.0.1',
                     f'  Port {PORT}', f'  User {self.machine.user}',
                     f'  IdentityFile ~/.ssh/{self.machine.identity.name}',
                     f'  UserKnownHostsFile {self.machine.work / serve.MACHINE_SSHD_KNOWN_HOSTS}',
                     '  StrictHostKeyChecking accept-new'):
            self.assertIn(line + '\n', text)
        # No run writes the key of its own machine into the known hosts of the person.
        self.assertNotIn('~/.ssh/known_hosts', text)
        self.assertTrue(str(self.machine.state) in serve.machine_sshd_ssh_config(self.machine))

    def test_the_namespace_of_the_machine_gives_it_a_tmp_and_a_home_of_its_own(self):
        namespace = self.machine.namespace()
        binds = {namespace[index + 2]: namespace[index + 1]
                 for index, word in enumerate(namespace) if word == '--bind'}

        self.assertEqual(binds['/tmp'], str(self.machine.tmp),
                         'the sockets of the cards and the tmux server stay out of /tmp here')
        self.assertEqual(binds[str(self.machine.user_home)], str(self.machine.home),
                         'no ~/.codex, ~/.ssh or tmux server of the person is read')
        self.assertEqual(binds[str(self.state)], str(self.state),
                         'the programs of the fixture are found where it wrote them')
        self.assertEqual(namespace[-1], '--')
        self.assertEqual(self.machine.command('true')[len(namespace):], ['true'])
        self.assertIn('--ro-bind', namespace)

    def test_the_manifest_names_the_machine_and_what_its_end_ends(self):
        described = self.machine.describe()

        self.assertEqual(described['port'], PORT)
        self.assertEqual(described['stop'], list(serve.MACHINE_SSHD_STOP))
        for name in ('sshd', 'log', 'home', 'tmp'):
            self.assertIn(name, described)


class ThePrepareOfTheMachine(unittest.TestCase):
    def setUp(self):
        if shutil.which('ssh-keygen') is None:
            self.skipTest('needs ssh-keygen, which mints the two keys of the machine')
        self.work = Path(tempfile.mkdtemp(prefix='hz-machine-sshd-'))
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)
        self.state = self.work / 'state'
        self.state.mkdir()
        self.machine = serve.MachineSshd(self.state, an_sshd_prefix(self.work / 'prefix'), port=PORT)

    def test_the_machine_gets_its_folders_keys_and_configurations(self):
        self.machine.prepare()

        for folder in (self.machine.work, self.machine.home, self.machine.tmp):
            self.assertEqual(stat.S_IMODE(folder.stat().st_mode), 0o700, folder)
        for key in (self.machine.host_key, self.machine.identity):
            self.assertTrue(key.is_file(), key)
            self.assertEqual(stat.S_IMODE(key.stat().st_mode), 0o600, 'ssh requires a private key')
        self.assertEqual(self.machine.authorized_keys.read_text(encoding='utf-8'),
                         self.machine.public_key.read_text(encoding='utf-8'),
                         'the client key of the run is the one key that the machine takes')
        self.assertEqual(self.machine.config.read_text(encoding='utf-8'),
                         serve.machine_sshd_config(self.machine))
        self.assertEqual(self.machine.ssh_config.read_text(encoding='utf-8'),
                         serve.machine_sshd_ssh_config(self.machine))

    def test_a_second_run_in_the_same_state_keeps_its_keys(self):
        self.machine.prepare()
        host_key = self.machine.host_key.read_bytes()

        self.machine.prepare()

        self.assertEqual(self.machine.host_key.read_bytes(), host_key,
                         'the key of the machine survives the second prepare of a reused state')

    def test_the_seeded_home_of_the_candidate_reaches_the_machine(self):
        self.machine.prepare()
        home = self.work / 'data' / 'home'
        home.mkdir(parents=True)

        seeded = sandbox.seed_ssh(home, *self.machine.ssh_sources())

        self.assertEqual(seeded, [sandbox.SSH_CONFIG_NAME, serve.MACHINE_SSHD_IDENTITY])
        config = home / '.ssh' / sandbox.SSH_CONFIG_NAME
        self.assertEqual(config.read_text(encoding='utf-8'),
                         serve.machine_sshd_ssh_config(self.machine))
        self.assertEqual(stat.S_IMODE(config.stat().st_mode), 0o600)
        self.assertEqual((home / '.ssh' / serve.MACHINE_SSHD_IDENTITY).read_bytes(),
                         self.machine.identity.read_bytes())

    def test_the_record_of_the_machine_names_its_folder_only_when_one_is_given(self):
        self.machine.prepare()

        plain = json.loads(serve.machine_watch_record(
            self.work / 'home', serve.MACHINE_SSHD_PROVIDER).read_text(encoding='utf-8'))
        named = json.loads(serve.machine_watch_record(
            self.work / 'home', serve.MACHINE_SSHD_PROVIDER,
            '/home/example/example-repo').read_text(encoding='utf-8'))

        self.assertNotIn('folder', plain['machines'][0],
                         'a record with no folder has the fields of a run without the option')
        self.assertEqual(named['machines'][0]['folder'], '/home/example/example-repo')

    def test_the_record_of_the_machine_names_the_alias_of_the_configuration(self):
        self.machine.prepare()

        written = json.loads(serve.machine_watch_record(
            self.work / 'home', serve.MACHINE_SSHD_PROVIDER).read_text(encoding='utf-8'))
        record = written['machines'][0]

        self.assertEqual(record['name'], serve.MACHINE_WATCH_MACHINE)
        self.assertEqual(record['host'], serve.MACHINE_WATCH_MACHINE,
                         'the route is the alias that the seeded configuration resolves')
        self.assertEqual(record['provider'], serve.MACHINE_SSHD_PROVIDER)
        self.assertIs(record[serve.MACHINE_RECORD_CARDS], True,
                      'the machine of a real sshd serves a card socket for each session')


class TheEndOfTheRun(unittest.TestCase):
    def setUp(self):
        self.work = Path(tempfile.mkdtemp(prefix='hz-machine-sshd-'))
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)
        self.state = self.work / 'state'
        self.state.mkdir()
        self.machine = serve.MachineSshd(self.state, an_sshd_prefix(self.work / 'prefix'), port=PORT)

    def test_the_stop_list_ends_the_sshd_and_then_the_tmux_server_of_the_machine(self):
        self.assertEqual(serve.MACHINE_SSHD_STOP, ('sshd', 'tmux'),
                         'the tmux server of new-session -d outlives the sshd that started it')
        self.machine.process = object()
        ended, ran = [], []

        steps = serve.machine_sshd_stop(self.machine, end=ended.append,
                                        run=lambda command, **options: ran.append(command))

        self.assertEqual(steps, list(serve.MACHINE_SSHD_STOP))
        self.assertEqual(ended, [self.machine.process], 'the sshd of the machine ends first')
        self.assertEqual(len(ran), 1)
        self.assertEqual(ran[0][-2:], ['tmux', 'kill-server'])
        self.assertTrue(ran[0][:-2], 'the tmux server is stopped inside the namespace of the machine')

    def test_a_machine_that_never_started_ends_nothing(self):
        ended, ran = [], []

        steps = serve.machine_sshd_stop(self.machine, end=ended.append,
                                        run=lambda command, **options: ran.append(command))

        self.assertEqual(steps, [])
        self.assertEqual((ended, ran), ([], []))

    def test_the_environment_of_the_machine_keeps_no_tmux_of_the_shell(self):
        environment = serve.machine_sshd_environment(
            {'PATH': '/usr/bin', 'TMUX': '/tmp/tmux-1000/default,1,0', 'TMUX_TMPDIR': '/tmp', 'HOME': '/home/x'})

        self.assertEqual(environment, {'PATH': '/usr/bin', 'HOME': '/home/x'},
                         'a run that a person starts inside tmux hands the machine no server')


if __name__ == '__main__':
    unittest.main()
