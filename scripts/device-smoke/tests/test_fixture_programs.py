"""No program of a fixture panel lies under the home of the person.

The fixture runs the candidate in a `bwrap` sandbox that binds the private home
over the home of the person, so a checkout under that home does not exist inside
it. A panel of the fixture therefore runs a copy of its program in the state
folder, which the sandbox binds on every run. These tests build the fixture
config of each option that runs such a program from a checkout under a fake home,
and they fail when a panel command or an argument names a path under that home.
"""
from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import serve  # noqa: E402

FIXTURE = Path(__file__).resolve().parents[1]


class FixtureProgramTests(unittest.TestCase):
    def prepare(self, folder):
        """A checkout under a fake home, its programs, and a state beside the home."""
        root = Path(folder).resolve()
        checkout = root / 'home' / 'repository' / 'scripts' / 'device-smoke'
        checkout.mkdir(parents=True)
        for name in serve.PANEL_PROGRAMS:
            (checkout / name).write_text((FIXTURE / name).read_text())
        state = root / 'state'
        state.mkdir()
        (state / 'data').mkdir()
        return root / 'home', checkout, state

    def panels_entry(self, folder, checkout, state):
        """A `--panels` file in the shape that the guide shows, with this checkout."""
        entry = {'name': 'Scripted card agent', 'kind': 'claude', 'command': '/bin/sh',
                 'args': ['-c', f'exec /usr/bin/python3 -u {checkout / serve.WORKTREE_CARD_PROGRAM} '
                                 '--mcp-command "horizon --browser-mcp" --scenario worktree "$@"'],
                 'cwd': str(state / 'data')}
        panels = Path(folder.resolve()) / 'panels.json'
        panels.write_text(json.dumps([entry]))
        return serve.load_panels(panels)

    def panel_strings(self, document):
        """Every string of a panel command or argument of the fixture config."""
        values = []
        for workspace in document['workspaces']:
            for panel in workspace['terminals']:
                values.append(panel.get('command'))
                values.extend(panel.get('args') or [])
        return [value for value in values if isinstance(value, str)]

    def test_no_panel_names_a_program_under_the_home(self):
        with tempfile.TemporaryDirectory() as folder:
            home, checkout, state = self.prepare(folder)
            repo = serve.worktree_card_repository(state)
            panels = {
                '--worktree-card': [serve.worktree_card_panel(Path('/opt/horizon'), repo, state,
                                                              checkout)],
                '--attention-agent': serve.attention_agent(state, 6.0, checkout)[0],
                '--frame-source-agent': [serve.frame_source_agent_panel(Path('/opt/horizon'), state,
                                                                        checkout)],
                '--panels': serve.panel_programs_in_state(
                    self.panels_entry(Path(folder), checkout, state), state, checkout),
            }
            for option, entries in panels.items():
                document = serve.application_document(str(state / 'data'), extra_panels=entries)
                self.assertTrue(entries, f'{option} places a panel')
                for value in self.panel_strings(document):
                    self.assertNotIn(str(home), value,
                                     f'{option}: a panel names a path under the home of the person')
                    self.assertNotIn(str(checkout), value,
                                     f'{option}: a panel names the folder of the checkout')

    def test_each_option_runs_the_copy_in_the_state_folder(self):
        with tempfile.TemporaryDirectory() as folder:
            _home, checkout, state = self.prepare(folder)
            repo = serve.worktree_card_repository(state)
            serve.worktree_card_panel(Path('/opt/horizon'), repo, state, checkout)
            _panels, bin_dir = serve.attention_agent(state, 6.0, checkout)
            serve.frame_source_agent_panel(Path('/opt/horizon'), state, checkout)
            for name, copied in (
                (serve.WORKTREE_CARD_PROGRAM, state / 'data' / serve.WORKTREE_CARD_PROGRAM),
                (serve.ATTENTION_STANDIN_PROGRAM, bin_dir / serve.ATTENTION_PROGRAM),
                (serve.FRAME_SOURCE_AGENT_PROGRAM, state / 'data' / serve.FRAME_SOURCE_AGENT_PROGRAM),
            ):
                self.assertEqual(copied.read_text(), (checkout / name).read_text(),
                                 f'{name} is the copy of the program of the checkout')
                self.assertFalse(copied.is_symlink(), 'a link into the checkout is no program')
                self.assertTrue(copied.stat().st_mode & 0o100, 'the program is executable')

    def test_the_client_panel_runs_the_copy_with_the_server_of_the_candidate(self):
        with tempfile.TemporaryDirectory() as folder:
            _home, checkout, state = self.prepare(folder)
            panel = serve.frame_source_agent_panel(Path('/opt/horizon'), state, checkout)
            launcher = Path(panel['command'])
            body = launcher.read_text()
            copy = state / 'data' / serve.FRAME_SOURCE_AGENT_PROGRAM
            self.assertIn(str(copy), body, 'the launcher runs the copy in the state folder')
            self.assertNotIn(str(checkout), body, 'the launcher names no checkout path')
            self.assertIn('/opt/horizon --browser-mcp', body, 'and the server of the candidate')
            self.assertIn('"$@"', body, 'Horizon gives its own arguments to the copy')
            self.assertTrue(launcher.stat().st_mode & 0o100, 'the launcher is executable')
            self.assertEqual(panel['kind'], 'claude', 'the panel is an agent panel')

    def test_a_frame_source_line_runs_the_copy_of_the_standin(self):
        with tempfile.TemporaryDirectory() as folder:
            home, checkout, state = self.prepare(folder)
            section = serve.frame_sources_section(
                ['standin=/usr/bin/python3 {standin} --width 640 --height 360'], state, checkout)
            command = section['sources'][0]['command']
            copy = state / 'data' / serve.PANEL_PROGRAM_FOLDER / serve.FRAME_SOURCE_STANDIN_PROGRAM
            self.assertIn(str(copy), command, 'the line runs the copy in the state folder')
            self.assertNotIn(str(checkout), command, 'the line names no checkout path')
            self.assertNotIn(str(home), command, 'and no path under the home of the person')
            self.assertEqual(copy.read_text(), (checkout / serve.FRAME_SOURCE_STANDIN_PROGRAM).read_text(),
                             'the copy is the stand-in of the checkout')
            self.assertTrue(copy.stat().st_mode & 0o100, 'the stand-in is executable')

    def test_a_frame_source_line_without_the_placeholder_stays_as_typed(self):
        with tempfile.TemporaryDirectory() as folder:
            _home, _checkout, state = self.prepare(folder)
            section = serve.frame_sources_section(
                ['bench=/usr/bin/a-frame-source-program --size 640x360'], state)
            self.assertEqual(section['sources'][0]['command'],
                             '/usr/bin/a-frame-source-program --size 640x360')

    def test_a_panels_entry_names_the_copy_of_its_program(self):
        with tempfile.TemporaryDirectory() as folder:
            _home, checkout, state = self.prepare(folder)
            panels = serve.panel_programs_in_state(
                self.panels_entry(Path(folder), checkout, state), state, checkout)
            copy = state / 'data' / serve.PANEL_PROGRAM_FOLDER / serve.WORKTREE_CARD_PROGRAM
            self.assertIn(str(copy), panels[0]['args'][1], 'the panel runs the copy')
            self.assertNotIn(str(checkout), panels[0]['args'][1], 'the panel names no checkout path')
            self.assertEqual(copy.read_text(), (checkout / serve.WORKTREE_CARD_PROGRAM).read_text(),
                             'the copy is the program of the checkout')
            self.assertEqual(panels[0]['command'], '/bin/sh', 'the rest of the entry stays')

    def test_a_panels_entry_of_another_program_is_unchanged(self):
        with tempfile.TemporaryDirectory() as folder:
            _home, checkout, state = self.prepare(folder)
            entry = {'name': 'A shell panel', 'kind': 'shell', 'command': '/bin/sh',
                     'args': ['-c', 'exec tail -f /dev/null'], 'cwd': str(state / 'data')}
            panels = serve.panel_programs_in_state([entry], state, checkout)
            self.assertEqual(panels[0], entry)
            self.assertFalse((state / 'data' / serve.PANEL_PROGRAM_FOLDER).exists(),
                             'a run that names no program of the fixture writes no copy')


if __name__ == '__main__':
    unittest.main()
