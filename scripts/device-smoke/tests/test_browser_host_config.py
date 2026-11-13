"""The device fixture names a browser host only when the procedure asks for one."""
from __future__ import annotations

import contextlib
import os
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

    def test_the_engine_and_the_command_of_a_firefox_host_travel_with_the_entry(self):
        document = serve.application_document(
            '/tmp/fixture-data', browser_host='browser-vm', browser_host_profile='fftest',
            browser_host_engine='firefox',
            browser_host_command='/home/fftest/.local/bin/horizon --browser-host-driver')
        self.assertEqual(document['browser_host'], {
            'ssh_target': 'browser-vm',
            'profile': 'fftest',
            'engine': 'firefox',
            'command': '/home/fftest/.local/bin/horizon --browser-host-driver',
        })

    def test_the_engine_and_the_command_are_options_of_a_named_host(self):
        parser = serve.arguments()
        args = parser.parse_args([
            '--horizon', '/bin/true', '--state', '/tmp/fixture-state', '--browser-host', 'browser-vm',
            '--browser-host-engine', 'firefox',
            '--browser-host-command', 'horizon --browser-host-driver'])
        self.assertEqual(args.browser_host_engine, 'firefox')
        self.assertEqual(args.browser_host_command, 'horizon --browser-host-driver')
        serve.check_browser_host_arguments(parser, args)

        with self.assertRaises(SystemExit):
            parser.parse_args([
                '--horizon', '/bin/true', '--state', '/tmp/fixture-state',
                '--browser-host-engine', 'webkit'])
        for arguments in (['--browser-host-engine', 'firefox'],
                          ['--browser-host-command', 'horizon --browser-host-driver']):
            alone = parser.parse_args(['--horizon', '/bin/true', '--state', '/tmp/fixture-state'] + arguments)
            with self.assertRaises(SystemExit), contextlib.redirect_stderr(open(os.devnull, 'w')):
                serve.check_browser_host_arguments(parser, alone)

    def test_the_browser_host_does_not_replace_the_panels(self):
        document = serve.application_document('/tmp/fixture-data', browser_host='browser-vm', presets=[])
        self.assertIn('terminals', document['workspaces'][0])
        self.assertEqual(document['browser_host'], {'ssh_target': 'browser-vm'})


if __name__ == '__main__':
    unittest.main()
