#!/usr/bin/env python3
"""The fixture binds the sound socket of the session only when a run names it.

A recorder that a panel starts runs inside the sandbox, and the sandbox binds a
runtime folder of its own over `/run/user/<uid>`, so the socket of the sound
server of the person is not there. `--host-audio` binds that one socket at the
same name and nothing else of the folder. These tests prove the shape of the
sandbox command and the line of a run without that socket. They need no display
and no bwrap.
"""
from __future__ import annotations

import os
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import sandbox  # noqa: E402
import serve  # noqa: E402


def binds_to(namespace, dest):
    """The source of every bind of `namespace` whose destination is `dest`."""
    found = []
    for index, item in enumerate(namespace):
        if item in ('--bind', '--ro-bind', '--ro-bind-try', '--dev-bind') and index + 2 < len(namespace):
            if namespace[index + 2] == dest:
                found.append(namespace[index + 1])
    return found


def bind_index(namespace, source, dest):
    """The position of the bind of `source` on `dest` in `namespace`."""
    for index, item in enumerate(namespace):
        if (item in ('--bind', '--ro-bind', '--ro-bind-try', '--dev-bind')
                and namespace[index + 1:index + 3] == [source, dest]):
            return index
    raise AssertionError(f'no bind of {source} on {dest}')


def audio_socket_in(folder):
    """A socket of the sound server shape, made under `folder`."""
    socket = Path(folder) / sandbox.HOST_AUDIO_SOCKET_NAME
    import socket as socket_module
    made = socket_module.socket(socket_module.AF_UNIX, socket_module.SOCK_STREAM)
    made.bind(str(socket))
    made.close()
    return socket


class HostAudioSocketTests(unittest.TestCase):
    def test_a_runtime_folder_without_the_socket_answers_none(self):
        with tempfile.TemporaryDirectory() as folder:
            self.assertIsNone(sandbox.host_audio_socket({'XDG_RUNTIME_DIR': folder}))

    def test_a_runtime_folder_with_the_socket_answers_it(self):
        with tempfile.TemporaryDirectory() as folder:
            socket = audio_socket_in(folder)
            self.assertEqual(sandbox.host_audio_socket({'XDG_RUNTIME_DIR': folder}), socket)

    def test_a_computer_without_a_runtime_folder_answers_none(self):
        self.assertIsNone(sandbox.host_audio_socket({}))

    def test_the_run_stops_with_one_line_without_that_socket(self):
        with tempfile.TemporaryDirectory() as folder:
            with self.assertRaises(SystemExit) as raised:
                serve.host_audio_bind({'XDG_RUNTIME_DIR': folder})
            message = str(raised.exception)
            self.assertIn('--host-audio', message)
            self.assertIn(sandbox.HOST_AUDIO_SOCKET_NAME, message)
            self.assertIn('without --host-audio', message, 'the line names the way out')
            self.assertEqual(len(message.splitlines()), 1, 'one line, not a traceback and a page')

    def test_the_run_answers_the_socket_when_the_computer_has_one(self):
        with tempfile.TemporaryDirectory() as folder:
            socket = audio_socket_in(folder)
            self.assertEqual(serve.host_audio_bind({'XDG_RUNTIME_DIR': folder}), socket)


class HostAudioBindTests(unittest.TestCase):
    def prepare(self, folder, **options):
        state = Path(folder) / 'state'
        state.mkdir()
        return sandbox.prepare(state, environ={}, **options)

    def test_a_run_without_the_option_binds_no_sound_socket(self):
        with tempfile.TemporaryDirectory() as folder:
            box = self.prepare(folder)
            self.assertEqual(
                binds_to(box.namespace, f'{box.runtime_dest}/{sandbox.HOST_AUDIO_SOCKET_NAME}'), [],
                'the private runtime folder holds no sound socket unless a run names one')

    def test_the_option_binds_that_one_socket_at_its_own_name(self):
        with tempfile.TemporaryDirectory() as folder:
            socket = audio_socket_in(folder)
            box = self.prepare(folder, host_audio=socket)
            self.assertEqual(
                binds_to(box.namespace, f'{box.runtime_dest}/{sandbox.HOST_AUDIO_SOCKET_NAME}'),
                [str(socket)])

    def test_the_bind_of_the_socket_follows_the_bind_of_the_folder(self):
        with tempfile.TemporaryDirectory() as folder:
            socket = audio_socket_in(folder)
            box = self.prepare(folder, host_audio=socket)
            folder_bind = bind_index(box.namespace, str(box.runtime), box.runtime_dest)
            socket_bind = bind_index(box.namespace, str(socket),
                                    f'{box.runtime_dest}/{sandbox.HOST_AUDIO_SOCKET_NAME}')
            self.assertGreater(socket_bind, folder_bind,
                               'the socket takes its name after the folder covers the name')

    def test_the_option_binds_nothing_else_of_the_runtime_folder(self):
        with tempfile.TemporaryDirectory() as folder:
            socket = audio_socket_in(folder)
            (Path(folder) / 'other-socket').write_text('not a socket\n')
            box = self.prepare(folder, host_audio=socket)
            sources = [source for source in binds_to(
                box.namespace, f'{box.runtime_dest}/other-socket')]
            self.assertEqual(sources, [], 'one socket, not the folder of the person')
            self.assertEqual(binds_to(box.namespace, box.runtime_dest), [str(box.runtime)],
                             'the private runtime folder itself is the only other bind there')


class HostAudioOptionTests(unittest.TestCase):
    def test_the_option_takes_no_value_and_writes_no_settings(self):
        parser = serve.arguments()
        args = parser.parse_args(['--horizon', '/opt/horizon', '--state', '/tmp/state'])
        self.assertFalse(args.host_audio)
        args = parser.parse_args(['--horizon', '/opt/horizon', '--state', '/tmp/state',
                                  '--host-audio'])
        self.assertTrue(args.host_audio)
        self.assertNotIn(('host_audio', '--host-audio'), serve.PLAIN_START_SETTINGS_OPTIONS,
                         'the option writes no settings, so a plain start keeps it')


if __name__ == '__main__':
    unittest.main()
