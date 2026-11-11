"""The fixture starts the candidate with a saved session only when a lane asks for one."""
from __future__ import annotations

import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import serve  # noqa: E402

APP = Path('/opt/horizon')
CONFIG = Path('/run/horizon.json')


class ApplicationCommandTest(unittest.TestCase):
    def test_the_default_stays_ephemeral(self):
        command = serve.application_command(APP, CONFIG, saved_session=False)

        self.assertEqual(command, ['/opt/horizon', '--config', '/run/horizon.json', '--ephemeral'])

    def test_a_saved_session_keeps_its_session_file(self):
        command = serve.application_command(APP, CONFIG, saved_session=True)

        self.assertEqual(command, ['/opt/horizon', '--config', '/run/horizon.json', '--new-session'])
        self.assertNotIn('--ephemeral', command, '--saved-session keeps the session')

    def test_a_resumed_session_names_no_session_argument(self):
        command = serve.application_command(APP, CONFIG, saved_session=False, resume_session=True)

        self.assertEqual(command, ['/opt/horizon', '--config', '/run/horizon.json'])


if __name__ == '__main__':
    unittest.main()
