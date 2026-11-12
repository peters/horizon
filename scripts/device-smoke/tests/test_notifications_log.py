"""The stand-in of a desktop notification records one call and no desktop shows it."""
from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import serve  # noqa: E402


class NotificationStandinTests(unittest.TestCase):

    def prepare(self, folder):
        state = Path(folder) / 'state'
        state.mkdir()
        bin_dir = serve.notification_standin(state)
        program = bin_dir / serve.NOTIFICATIONS_PROGRAM
        return state, program

    def test_the_program_lies_in_the_state_folder_and_runs(self):
        with tempfile.TemporaryDirectory() as folder:
            state, program = self.prepare(folder)
            # The sandbox binds the private home over the home of the person, so a
            # link into the checkout would point nowhere; the state folder is bound.
            self.assertFalse(program.is_symlink())
            self.assertIn(state.resolve(), program.resolve().parents)
            self.assertTrue(program.stat().st_mode & stat.S_IXUSR, 'the program is executable')
            self.assertTrue(program.read_text().startswith('#!/bin/sh'), 'the program names its shell')
            self.assertEqual(program.name, serve.NOTIFICATIONS_PROGRAM)

    def test_one_call_appends_one_line_of_json_with_the_arguments(self):
        with tempfile.TemporaryDirectory() as folder:
            state, program = self.prepare(folder)
            subprocess.run(
                [str(program), '--app-name=Horizon', '--', 'Horizon', 'Fix the sync needs you'],
                check=True,
            )
            subprocess.run([str(program), '--', 'Horizon', 'Write the docs is done'], check=True)
            lines = (state / serve.NOTIFICATIONS_LOG).read_text().splitlines()
            self.assertEqual(len(lines), 2, 'one line for each call')
            self.assertEqual(
                json.loads(lines[0]),
                ['--app-name=Horizon', '--', 'Horizon', 'Fix the sync needs you'],
            )
            self.assertEqual(json.loads(lines[1]), ['--', 'Horizon', 'Write the docs is done'])

    def test_a_name_that_looks_like_an_option_stays_text(self):
        with tempfile.TemporaryDirectory() as folder:
            state, program = self.prepare(folder)
            subprocess.run([str(program), '--', 'Horizon', '--urgency=critical needs you'], check=True)
            arguments = json.loads((state / serve.NOTIFICATIONS_LOG).read_text())
            self.assertEqual(arguments[1:], ['Horizon', '--urgency=critical needs you'])
            self.assertEqual(arguments[0], '--')

    def test_a_body_with_markup_arrives_as_the_text_that_horizon_escaped(self):
        with tempfile.TemporaryDirectory() as folder:
            state, program = self.prepare(folder)
            subprocess.run([str(program), '--', 'Horizon', '&lt;b&gt;Fix&lt;/b&gt; &amp; more'], check=True)
            arguments = json.loads((state / serve.NOTIFICATIONS_LOG).read_text())
            self.assertEqual(arguments[2], '&lt;b&gt;Fix&lt;/b&gt; &amp; more')

    def test_the_folder_of_the_stand_in_comes_first_on_the_path(self):
        with tempfile.TemporaryDirectory() as folder:
            state = Path(folder) / 'state'
            state.mkdir()
            bin_dir = serve.notification_standin(state)
            environment = serve.attention_environment({'PATH': '/usr/bin:/bin'}, bin_dir)
            first = environment['PATH'].split(':')[0]
            self.assertTrue(Path(first).is_dir(), 'the entry is a folder and not a file')
            program = Path(first) / serve.NOTIFICATIONS_PROGRAM
            self.assertTrue(program.is_file(), 'the folder holds the program of a notification')
            self.assertTrue(program.stat().st_mode & stat.S_IXUSR, 'the program is executable')
            self.assertEqual(environment['PATH'], f'{bin_dir}:/usr/bin:/bin')

    def test_every_entry_that_the_fixture_adds_to_the_path_is_a_folder(self):
        with tempfile.TemporaryDirectory() as folder:
            _state, program = self.prepare(folder)
            environment = serve.attention_environment({'PATH': '/usr/bin:/bin'}, program.parent)
            added = [entry for entry in environment['PATH'].split(':') if entry != '/usr/bin']
            added = [entry for entry in added if entry != '/bin']
            self.assertEqual(len(added), 1, 'the fixture adds one entry for the notifications')
            for entry in added:
                self.assertTrue(Path(entry).is_dir(), f'the fixture put a file on the PATH: {entry}')

    def test_an_entry_that_is_a_file_is_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            _state, program = self.prepare(folder)
            with self.assertRaises(RuntimeError) as raised:
                serve.attention_environment({'PATH': '/usr/bin:/bin'}, program)
            self.assertIn('not a folder', str(raised.exception))

    def test_the_panel_follows_the_log_of_the_notifications(self):
        with tempfile.TemporaryDirectory() as folder:
            state, _program = self.prepare(folder)
            panel = serve.notification_panel(state)
            self.assertEqual(panel['name'], serve.NOTIFICATIONS_PANEL)
            self.assertEqual(panel['kind'], 'shell')
            follow = ' '.join(panel['args'])
            self.assertIn(str((state / serve.NOTIFICATIONS_LOG).resolve()), follow)
            self.assertIn('tail -f', follow, 'the panel shows the line that arrives')

    def test_a_plain_start_refuses_the_stand_in(self):
        self.assertIn('--notifications-log', [flag for _name, flag in serve.PLAIN_START_SETTINGS_OPTIONS])


class PlainWindowTests(unittest.TestCase):

    def test_xterm_is_the_plain_window_when_the_computer_has_it(self):
        command = serve.plain_window_command(which=lambda name: '/usr/bin/' + name, python='/usr/bin/python3')
        self.assertEqual(command[0], 'xterm')
        self.assertIn(serve.PLAIN_WINDOW_TITLE, command)

    def test_xmessage_is_the_plain_window_without_xterm(self):
        command = serve.plain_window_command(
            which=lambda name: '/usr/bin/xmessage' if name == 'xmessage' else None,
            python='/usr/bin/python3',
        )
        self.assertEqual(command[0], 'xmessage')
        self.assertIn(serve.PLAIN_WINDOW_TITLE, command)

    def test_a_window_of_tk_is_the_plain_window_without_the_other_two(self):
        command = serve.plain_window_command(which=lambda name: None, python='/usr/bin/python3', has_tk=True)
        self.assertEqual(command[0], '/usr/bin/python3')
        self.assertIn('tkinter', command[2])
        self.assertIn(serve.PLAIN_WINDOW_TITLE, command[2])

    def test_a_computer_without_a_plain_window_says_what_is_missing(self):
        with self.assertRaises(SystemExit) as raised:
            serve.plain_window_command(which=lambda name: None, python='/usr/bin/python3', has_tk=False)
        self.assertIn('xterm', str(raised.exception))


if __name__ == '__main__':
    unittest.main()
