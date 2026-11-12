"""The device fixture opens the stand-in agent that shows the attention states."""
from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import serve  # noqa: E402


class AttentionAgentTests(unittest.TestCase):

    def prepare(self, folder, timer=6.0):
        state = Path(folder) / 'state'
        state.mkdir()
        home = Path(folder) / 'home'
        home.mkdir()
        panel = serve.attention_agent(home, Path('/usr/bin/candidate'), state, timer)
        return state, home, panel

    def test_the_fixture_registers_the_launch_of_the_panel_before_it_starts(self):
        with tempfile.TemporaryDirectory() as folder:
            state, home, panel = self.prepare(folder)
            self.assertEqual(panel['kind'], 'claude', 'the panel kind that the hooks of Horizon know')
            panel_id = serve.ATTENTION_PANEL_ID
            encoded = '%' + ''.join(f'{byte:02x}' for byte in panel_id.encode())
            record = home / '.horizon' / 'agent-work' / f'{encoded}.json'
            self.assertTrue(record.is_file(), 'the work record of the launch')
            document = json.loads(record.read_text())
            self.assertEqual(document['panel_local_id'], panel_id)
            self.assertEqual(document['owner_token'], serve.ATTENTION_OWNER)
            self.assertEqual(document['kind'], 'claude')
            marker = home / '.horizon' / 'agent-work' / 'health' / encoded / serve.ATTENTION_OWNER
            self.assertEqual(marker.read_text(), '1', 'the health marker of the launch')
            # The script reads the same document that the fixture wrote.
            config = json.loads(Path(panel['args'][1].split('--config ')[1].split(' ')[0]).read_text())
            self.assertEqual(config['root'], str(home / '.horizon'))
            self.assertEqual(config['panel'], panel_id)
            self.assertEqual(config['exe'], '/usr/bin/candidate')

    def test_the_panel_runs_the_stand_in_through_a_shell_that_passes_the_launch_arguments(self):
        with tempfile.TemporaryDirectory() as folder:
            _state, _home, panel = self.prepare(folder)
            self.assertEqual(panel['command'], '/bin/sh')
            self.assertEqual(panel['args'][0], '-c')
            command = panel['args'][1]
            self.assertIn('agent_attention_standin.py', command)
            self.assertIn('--timer 6.0', command)
            self.assertTrue(command.endswith('"$@"'), 'Horizon adds its own launch arguments')
            self.assertEqual(panel['args'][2], 'attention-stand-in')

    def test_a_run_without_a_timer_waits_for_the_lines_of_the_person(self):
        with tempfile.TemporaryDirectory() as folder:
            _state, _home, panel = self.prepare(folder, timer=None)
            self.assertNotIn('--timer', panel['args'][1])

    def test_a_plain_start_refuses_the_stand_in(self):
        self.assertIn('--attention-agent', [flag for _name, flag in serve.PLAIN_START_SETTINGS_OPTIONS])


if __name__ == '__main__':
    unittest.main()
