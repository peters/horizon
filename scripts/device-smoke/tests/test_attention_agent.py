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
        panels, bin_dir = serve.attention_agent(state, timer)
        return state, panels[0], bin_dir

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

    def test_several_panels_lie_in_their_own_cells_and_keep_their_own_folders(self):
        with tempfile.TemporaryDirectory() as folder:
            state = Path(folder) / 'state'
            state.mkdir()
            panels, bin_dir = serve.attention_agent(state, 0, count=3)
            self.assertEqual(len(panels), 3)
            self.assertEqual([panel['name'] for panel in panels],
                             ['Attention stand-in 1', 'Attention stand-in 2', 'Attention stand-in 3'])
            folders = [Path(panel['cwd']) for panel in panels]
            self.assertEqual(len(set(folders)), 3, 'each stand-in writes its own transcript')
            for panel, folder in zip(panels, folders):
                self.assertTrue(folder.is_dir(), 'the working folder of a panel exists')
                self.assertEqual(folder.parent, state / serve.ATTENTION_STANDIN_FOLDER)
                self.assertIsNone(panel['command'], 'the panel still starts the stand-in by its name')
                self.assertTrue(panel['work_resume']['enabled'])
                # No two panels share a place, so a task types into the panel that
                # it names.
                for other in panels:
                    if other is not panel:
                        self.assertNotEqual(other['position'], panel['position'])
            self.assertEqual(panels[0]['size'], list(serve.ATTENTION_ROW_SIZE))
            self.assertTrue((bin_dir / serve.ATTENTION_PROGRAM).is_file(),
                            'one program serves every panel of the run')

    def test_no_panel_of_a_run_covers_the_panel_of_the_notifications(self):
        window = (1480, 900)
        for count in range(1, serve.ATTENTION_MAX_AGENTS + 1):
            with tempfile.TemporaryDirectory() as folder:
                state = Path(folder) / 'state'
                state.mkdir()
                panels, _bin_dir = serve.attention_agent(state, 0, count=count)
                covered = [serve.notification_panel(state), *panels]
                for panel in covered:
                    left, top = panel['position']
                    width, height = panel['size']
                    self.assertGreaterEqual(left, 0, f'{panel["name"]} in {window}')
                    self.assertGreaterEqual(top, 0, f'{panel["name"]} in {window}')
                    self.assertLessEqual(left + width, window[0], f'{panel["name"]} in {window}')
                    self.assertLessEqual(top + height, window[1], f'{panel["name"]} in {window}')
                for index, panel in enumerate(covered):
                    for other in covered[index + 1:]:
                        self.assertFalse(self.overlap(panel, other),
                                         f'{panel["name"]} covers {other["name"]} with {count} panels')

    @staticmethod
    def overlap(one, other):
        left = max(one['position'][0], other['position'][0])
        top = max(one['position'][1], other['position'][1])
        right = min(one['position'][0] + one['size'][0], other['position'][0] + other['size'][0])
        bottom = min(one['position'][1] + one['size'][1], other['position'][1] + other['size'][1])
        return left < right and top < bottom

    def test_a_panel_of_its_own_command_is_the_second_shape_of_the_entry(self):
        with tempfile.TemporaryDirectory() as folder:
            state = Path(folder) / 'state'
            state.mkdir()
            panels, bin_dir = serve.attention_agent(state, 0, count=1, command=True)
            self.assertEqual(len(panels), 2)
            default, own = panels
            # The first shape keeps the shared entry: Horizon resolves the name of
            # the program on the PATH of the candidate and inserts its arguments.
            self.assertIsNone(default['command'])
            self.assertTrue(default['work_resume']['enabled'])
            # The second shape is a preset with a command of its own, which is the
            # stand-in itself, and its work continuation is off.
            shared = json.loads(serve.ATTENTION_COMMAND_PANEL_ENTRY.read_text())
            for key in ('name', 'kind', 'args', 'work_resume'):
                self.assertEqual(own[key], shared[key], f'the entry of the second shape keeps {key}')
            self.assertIsInstance(shared['command'], str, 'the shared entry names a command of its preset')
            self.assertEqual(own['command'], str(bin_dir / serve.ATTENTION_PROGRAM))
            self.assertTrue(Path(own['command']).is_file(), 'the command names a program of the run')
            self.assertFalse(own['work_resume']['enabled'], 'the panel offers no resume')
            self.assertEqual(own['kind'], 'claude', 'the kind whose hooks Horizon installs')
            self.assertEqual(own['args'], [])
            self.assertNotEqual(own['cwd'], default['cwd'], 'each panel writes its own transcript')
            for panel in panels:
                left, top = panel['position']
                width, height = panel['size']
                self.assertLessEqual(left + width, 1480, f'{panel["name"]} stays inside the window')
                self.assertLessEqual(top + height, 560, f'{panel["name"]} stays above the notifications')

    def test_the_two_shapes_together_stay_inside_the_bound(self):
        with tempfile.TemporaryDirectory() as folder:
            state = Path(folder) / 'state'
            state.mkdir()
            panels, _bin_dir = serve.attention_agent(state, 0, count=serve.ATTENTION_MAX_AGENTS - 1, command=True)
            self.assertEqual(len(panels), serve.ATTENTION_MAX_AGENTS)
            places = [tuple(panel['position']) for panel in panels]
            self.assertEqual(len(set(places)), len(places), 'no panel covers another')
            with self.assertRaises(SystemExit) as raised:
                serve.attention_agent(state, 0, count=serve.ATTENTION_MAX_AGENTS, command=True)
            self.assertIn('--attention-agent-command', str(raised.exception))

    def test_one_panel_keeps_the_entry_of_the_fixture(self):
        with tempfile.TemporaryDirectory() as folder:
            state = Path(folder) / 'state'
            state.mkdir()
            panels, _bin_dir = serve.attention_agent(state, 0, count=1)
            self.assertEqual(len(panels), 1)
            shared = json.loads(serve.ATTENTION_PANEL_ENTRY.read_text())
            self.assertEqual({key: value for key, value in panels[0].items() if key != 'cwd'}, shared)
            self.assertEqual(panels[0]['cwd'], str(state / serve.ATTENTION_STANDIN_FOLDER))

    def test_a_pi_panel_is_the_entry_of_the_pi_stand_in(self):
        with tempfile.TemporaryDirectory() as folder:
            state = Path(folder) / 'state'
            state.mkdir()
            panels, bin_dir = serve.attention_agent(state, 0, kind='pi')
            self.assertEqual(len(panels), 1)
            panel = panels[0]
            # A pi launch receives the report extension of the candidate with `-e`,
            # so the entry names no command and Horizon resolves `pi` on the PATH
            # of the candidate, where the stand-in is.
            self.assertIsNone(panel['command'])
            self.assertEqual(panel['kind'], 'pi', 'the agent kind whose report Horizon attaches')
            self.assertEqual(panel['args'], [])
            self.assertFalse(panel['work_resume']['enabled'], 'the panel offers no resume of pi work')
            shared = json.loads(serve.ATTENTION_PI_PANEL_ENTRY.read_text())
            self.assertEqual({key: value for key, value in panel.items() if key != 'cwd'}, shared)
            program = bin_dir / serve.ATTENTION_PI_PROGRAM
            self.assertEqual(program.name, 'pi', 'the name that the pi preset resolves')
            self.assertEqual(program.read_text(), SCRIPT.read_text(), 'the program is the stand-in itself')
            self.assertTrue(program.stat().st_mode & stat.S_IXUSR, 'the program is executable')
            self.assertFalse((bin_dir / serve.ATTENTION_PROGRAM).exists(), 'no claude name comes with a pi panel')

    def test_a_codex_panel_is_the_entry_of_the_codex_stand_in(self):
        with tempfile.TemporaryDirectory() as folder:
            state = Path(folder) / 'state'
            state.mkdir()
            panels, bin_dir = serve.attention_agent(state, 0, kind='codex')
            self.assertEqual(len(panels), 1)
            panel = panels[0]
            # A codex launch needs no argument for its state title, so the entry
            # names no command and Horizon resolves `codex` on the PATH of the
            # candidate, where the stand-in is.
            self.assertIsNone(panel['command'])
            self.assertEqual(panel['kind'], 'codex', 'the agent kind that writes its state in its title')
            # Every codex panel of the fixture starts the launch of the real program,
            # which the run shows before that program is ready for the person.
            self.assertEqual(panel['args'], [serve.ATTENTION_STARTUP_ARGUMENT])
            self.assertFalse(panel['work_resume']['enabled'], 'a codex panel continues no work')
            shared = json.loads(serve.ATTENTION_CODEX_PANEL_ENTRY.read_text())
            # The shared entry names no argument of a launch: no real program has
            # the argument that starts the launch of the stand-in.
            self.assertEqual(
                {key: value for key, value in panel.items() if key not in ('cwd', 'args')},
                {key: value for key, value in shared.items() if key != 'args'},
            )
            program = bin_dir / serve.ATTENTION_CODEX_PROGRAM
            self.assertEqual(program.name, 'codex', 'the name that the codex preset resolves')
            self.assertEqual(
                program.read_text(),
                (SCRIPT.parent / serve.ATTENTION_CODEX_STANDIN_PROGRAM).read_text(),
                'the program is the codex stand-in itself',
            )
            self.assertTrue(program.stat().st_mode & stat.S_IXUSR, 'the program is executable')
            self.assertFalse((bin_dir / serve.ATTENTION_PROGRAM).exists(), 'no claude name comes with a codex panel')

    def test_every_kind_of_the_stand_in_names_its_own_program_and_entry(self):
        self.assertEqual(set(serve.ATTENTION_KINDS), set(serve.ATTENTION_KIND_PROGRAM))
        self.assertEqual(set(serve.ATTENTION_KINDS), set(serve.ATTENTION_KIND_ENTRY))
        self.assertEqual(set(serve.ATTENTION_KINDS), set(serve.ATTENTION_KIND_SCRIPT))
        for kind in serve.ATTENTION_KINDS:
            entry = json.loads(serve.ATTENTION_KIND_ENTRY[kind].read_text())
            self.assertEqual(entry['kind'], kind, 'the entry is the kind that names it')
            script = SCRIPT.parent / serve.ATTENTION_KIND_SCRIPT[kind]
            self.assertTrue(script.is_file(), f'the program of {kind} is in this folder: {script}')
            self.assertIn(script.name, serve.PANEL_PROGRAMS, 'the fixture copies the program of the panel')

    def test_a_pi_panel_of_its_own_command_names_the_stand_in(self):
        with tempfile.TemporaryDirectory() as folder:
            state = Path(folder) / 'state'
            state.mkdir()
            panels, bin_dir = serve.attention_agent(state, 0, count=1, command=True, kind='pi')
            self.assertEqual(len(panels), 2)
            default, own = panels
            self.assertEqual({panel['kind'] for panel in panels}, {'pi'}, 'both panels are pi panels')
            self.assertIsNone(default['command'])
            self.assertEqual(own['command'], str(bin_dir / serve.ATTENTION_PI_PROGRAM))
            self.assertTrue(Path(own['command']).is_file(), 'the command names a program of the run')
            self.assertIn('with its own command', own['name'])
            self.assertNotEqual(own['cwd'], default['cwd'], 'each panel writes its own session')
            self.assertEqual(default['name'], 'Pi attention stand-in 1', 'a count names the cells')

    def test_the_protocol_of_a_codex_panel_is_an_argument_of_its_entry(self):
        with tempfile.TemporaryDirectory() as folder:
            state = Path(folder) / 'state'
            state.mkdir()
            panels, _bin_dir = serve.attention_agent(state, 0, kind='codex', protocol=True)
            self.assertEqual(panels[0]['kind'], 'codex')
            self.assertEqual(
                panels[0]['args'],
                [serve.ATTENTION_STARTUP_ARGUMENT, serve.ATTENTION_PROTOCOL_ARGUMENT],
                'the stand-in writes the launch of the real program and its reports',
            )
            self.assertIsNone(panels[0]['command'], 'the default command is the stand-in of the kind')

    def test_a_panel_without_the_protocol_names_the_startup_argument_and_nothing_else(self):
        with tempfile.TemporaryDirectory() as folder:
            state = Path(folder) / 'state'
            state.mkdir()
            panels, _bin_dir = serve.attention_agent(state, 0, kind='codex')
            self.assertEqual(
                panels[0]['args'],
                [serve.ATTENTION_STARTUP_ARGUMENT],
                'the titles alone are the report of that panel',
            )

    def test_an_unknown_agent_kind_is_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            state = Path(folder) / 'state'
            state.mkdir()
            with self.assertRaises(SystemExit) as raised:
                serve.attention_agent(state, 0, kind='gemini')
            self.assertIn('--attention-agent-kind', str(raised.exception))

    def test_a_count_outside_the_bound_is_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            state = Path(folder) / 'state'
            state.mkdir()
            for count in (0, serve.ATTENTION_MAX_AGENTS + 1):
                with self.assertRaises(SystemExit) as raised:
                    serve.attention_agent(state, 0, count=count)
                self.assertIn('--attention-agent-count', str(raised.exception))


if __name__ == '__main__':
    unittest.main()
