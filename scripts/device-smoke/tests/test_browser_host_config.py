"""The device fixture names a browser host only when the procedure asks for one."""
from __future__ import annotations

from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import serve  # noqa: E402


class BrowserHostConfigTests(unittest.TestCase):
    def test_default_document_names_no_browser_host(self):
        document = serve.application_document('/tmp/fixture-data')
        self.assertNotIn('browser_host', document)

    def test_target_sets_the_entry_and_a_profile_travels_with_it(self):
        document = serve.application_document('/tmp/fixture-data', browser_host='browser-vm')
        self.assertEqual(document['browser_host'], {'ssh_target': 'browser-vm'})

        document = serve.application_document(
            '/tmp/fixture-data', browser_host='browser-vm', browser_host_profile='mail')
        self.assertEqual(document['browser_host'], {'ssh_target': 'browser-vm', 'profile': 'mail'})

    def test_the_browser_host_does_not_replace_the_panels(self):
        document = serve.application_document('/tmp/fixture-data', browser_host='browser-vm', presets=[])
        self.assertIn('terminals', document['workspaces'][0])
        self.assertEqual(document['browser_host'], {'ssh_target': 'browser-vm'})


if __name__ == '__main__':
    unittest.main()
