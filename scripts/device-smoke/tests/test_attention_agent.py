"""The device fixture opens the stand-in agent that shows the attention states."""
from __future__ import annotations

import json
import os
import stat
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import serve  # noqa: E402

HERE = Path(__file__).resolve().parent
SCRIPT = HERE.parent / 'agent_attention_standin.py'


class AttentionAgentTests(unittest.TestCase):

    def prepare(self, folder, timer=6.0):
        state = Path(folder) / 'state'
        state.mkdir()
        panel, bin_dir = serve.attention_agent(state, timer)
        return state, panel, bin_dir

    def test_the_panel_entry_is_a_claude_panel_of_the_default_command(self):
        with tempfile.TemporaryDirectory() as folder:
            _state, panel, _bin_dir = self.prepare(folder)
            # Horizon inserts its own arguments between the program of a panel and
            # the arguments of the panel entry, and it attaches the work ledger of
            # a Claude panel only when the entry names no command. The entry that
            # the Rust test of the command line builder reads is the same file.
            self.assertIsNone(panel['command'], 'a command of the entry would take the launch arguments')
            self.assertEqual(panel['args'], [])
            self.assertEqual(panel['kind'], 'claude', 'the panel kind whose hooks Horizon installs')
            self.assertTrue(panel['work_resume']['enabled'], 'the policy that attaches the work ledger')
            self.assertEqual(panel['name'], 'Attention stand-in')

    def test_the_shared_entry_is_the_file_that_the_command_line_test_reads(self):
        with tempfile.TemporaryDirectory() as folder:
            state, panel, _bin_dir = self.prepare(folder)
            shared = json.loads(serve.ATTENTION_PANEL_ENTRY.read_text())
            self.assertEqual({key: value for key, value in panel.items() if key != 'cwd'}, shared)
            self.assertEqual(panel['cwd'], str(state / serve.ATTENTION_STANDIN_FOLDER))

    def test_the_stand_in_is_the_program_that_the_name_resolves_to(self):
        with tempfile.TemporaryDirectory() as folder:
            state, _panel, bin_dir = self.prepare(folder)
            program = bin_dir / serve.ATTENTION_PROGRAM
            # The sandbox binds the private home over the home of the person, so a
            # link into a checkout under that home points nowhere inside it. The
            # program of a panel is a copy in the state folder, which is bound.
            self.assertFalse(program.is_symlink(), 'a link into the checkout is no program inside the sandbox')
            self.assertIn(state.resolve(), program.resolve().parents, 'the program lies in the state folder')
            self.assertEqual(program.read_text(), SCRIPT.read_text(), 'the program is the stand-in itself')
            self.assertTrue(program.stat().st_mode & stat.S_IXUSR, 'the program is executable')
            self.assertTrue(SCRIPT.read_text().startswith('#!'), 'the program names its interpreter')

    def test_a_program_outside_the_state_folder_is_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            state = Path(folder) / 'state'
            state.mkdir()
            outside = Path(folder) / 'checkout' / 'standin.py'
            outside.parent.mkdir()
            outside.write_text('#!/bin/sh\nexit 0\n')
            with self.assertRaises(RuntimeError):
                serve.program_in_state(outside, Path(folder) / 'bin' / 'standin', state)
            self.assertFalse((Path(folder) / 'bin' / 'standin').exists(), 'a refused program is not written')

    def test_a_program_of_the_state_folder_replaces_the_file_that_is_there(self):
        with tempfile.TemporaryDirectory() as folder:
            state = Path(folder) / 'state'
            state.mkdir()
            destination = state / 'bin' / 'standin'
            destination.parent.mkdir()
            destination.symlink_to(SCRIPT)
            serve.program_in_state(SCRIPT, destination, state)
            self.assertFalse(destination.is_symlink(), 'the run does not keep the link of an older run')
            self.assertEqual(destination.read_text(), SCRIPT.read_text())

    def test_the_timer_of_the_run_is_the_document_beside_the_stand_in(self):
        with tempfile.TemporaryDirectory() as folder:
            _state, _panel, bin_dir = self.prepare(folder, timer=6.0)
            document = json.loads((bin_dir / serve.ATTENTION_TIMER_FILE).read_text())
            self.assertEqual(document, {'timer': 6.0})

    def test_a_run_without_a_timer_waits_for_the_lines_of_the_person(self):
        with tempfile.TemporaryDirectory() as folder:
            _state, _panel, bin_dir = self.prepare(folder, timer=None)
            document = json.loads((bin_dir / serve.ATTENTION_TIMER_FILE).read_text())
            self.assertEqual(document, {'timer': 0}, 'zero sends no event of its own')

    def test_the_stand_in_comes_first_on_the_path_of_the_candidate(self):
        with tempfile.TemporaryDirectory() as folder:
            _state, _panel, bin_dir = self.prepare(folder)
            environment = serve.attention_environment({'PATH': '/usr/bin:/bin', 'HOME': folder}, bin_dir)
            self.assertEqual(environment['PATH'], f'{bin_dir}:/usr/bin:/bin')
            self.assertEqual(environment['HOME'], folder)
            original = {'PATH': '/usr/bin'}
            serve.attention_environment(original, bin_dir)
            self.assertEqual(original['PATH'], '/usr/bin', 'the environment of the caller is unchanged')
            self.assertTrue(os.path.isdir(bin_dir))

    def test_a_plain_start_refuses_the_stand_in(self):
        self.assertIn('--attention-agent', [flag for _name, flag in serve.PLAIN_START_SETTINGS_OPTIONS])


if __name__ == '__main__':
    unittest.main()
