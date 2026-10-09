"""The device fixture keeps Firefox system access off unless the procedure asks."""
from __future__ import annotations

from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import serve  # noqa: E402


class FirefoxSystemAccessConfigTests(unittest.TestCase):
    def test_default_document_omits_the_browser_section(self):
        document = serve.application_document('/tmp/fixture-data')
        self.assertNotIn('browser', document)

    def test_flag_sets_firefox_and_the_sign_in_pages(self):
        document = serve.application_document('/tmp/fixture-data', firefox_system_access=True)
        self.assertEqual(document['browser'], {'backend': 'firefox', 'firefox_system_access': True})
        commands = [item.get('command') for item in document['workspaces'][0]['terminals']]
        self.assertIn('https://accounts.google.com/ServiceLogin?hl=en', commands)
        self.assertIn('https://x.com/i/flow/login', commands)

    def test_device_address_still_replaces_the_terminals(self):
        document = serve.application_document(
            '/tmp/fixture-data', device_address='127.0.0.1:5900', firefox_system_access=True)
        terminals = document['workspaces'][0]['terminals']
        self.assertEqual(len(terminals), 1)
        self.assertEqual(terminals[0]['kind'], 'device')
        self.assertEqual(terminals[0]['command'], '127.0.0.1:5900')
        self.assertEqual(document['browser'], {'backend': 'firefox', 'firefox_system_access': True})


if __name__ == '__main__':
    unittest.main()
