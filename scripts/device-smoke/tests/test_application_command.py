"""The fixture starts the candidate with a saved session only when a lane asks for one."""
from __future__ import annotations

import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import serve  # noqa: E402

APP = Path('/opt/horizon')
CONFIG = Path('/run/horizon.yaml')


class ApplicationCommandTest(unittest.TestCase):
    def test_the_default_stays_ephemeral(self):
        command = serve.application_command(APP, CONFIG, saved_session=False)

        self.assertEqual(command, ['/opt/horizon', '--config', '/run/horizon.yaml', '--ephemeral'])

    def test_a_saved_session_keeps_its_session_file(self):
        command = serve.application_command(APP, CONFIG, saved_session=True)

        self.assertEqual(command, ['/opt/horizon', '--config', '/run/horizon.yaml', '--new-session'])
        self.assertNotIn('--ephemeral', command, 'a cloud needs a saved session')


if __name__ == '__main__':
    unittest.main()
