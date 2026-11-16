"""The device fixture keeps agent programs only when the procedure asks."""
from __future__ import annotations

import argparse
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import serve  # noqa: E402


class KeepAgentsConfigTests(unittest.TestCase):
    def test_the_default_document_keeps_no_agent_program(self):
        document = serve.application_document('/tmp/fixture-data')
        self.assertNotIn('features', document)

    def test_the_flag_sets_the_setting_of_this_computer(self):
        document = serve.application_document('/tmp/fixture-data', keep_agents=True)
        self.assertEqual(
            document['features'],
            {'keep_agents_through_restart': True})

    def test_a_plain_start_refuses_the_option(self):
        # The option writes the settings of the fixture, and a plain start keeps the
        # home as its own settings writer.
        values = {name: False for name, _ in serve.PLAIN_START_SETTINGS_OPTIONS}
        values['keep_agents'] = True
        refused = serve.plain_start_conflicts(argparse.Namespace(**values))
        self.assertIn('--keep-agents', refused)


if __name__ == '__main__':
    unittest.main()
