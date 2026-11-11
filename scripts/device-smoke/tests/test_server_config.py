"""The device fixture names a server only when the procedure asks for one."""
from __future__ import annotations

from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import sandbox  # noqa: E402
import serve  # noqa: E402


class ServerConfigTests(unittest.TestCase):
    def test_default_document_names_no_server(self):
        document = serve.application_document('/tmp/fixture-data')
        self.assertNotIn('server', document)

    def test_target_sets_the_entry_and_a_command_travels_with_it(self):
        document = serve.application_document('/tmp/fixture-data', server_target='server-vm')
        self.assertEqual(document['server'], {'ssh_target': 'server-vm'})

        document = serve.application_document(
            '/tmp/fixture-data', server_target='server-vm',
            server_command='/opt/horizon --server-link')
        self.assertEqual(
            document['server'], {'ssh_target': 'server-vm', 'command': '/opt/horizon --server-link'})

    def test_an_address_sets_the_entry_of_a_listener(self):
        document = serve.application_document('/tmp/fixture-data', server_address='127.0.0.1:7000')
        self.assertEqual(document['server'], {'address': '127.0.0.1:7000'})

        document = serve.application_document(
            '/tmp/fixture-data', server_target='server-vm', server_address='127.0.0.1:7000')
        self.assertEqual(
            document['server'], {'ssh_target': 'server-vm', 'address': '127.0.0.1:7000'},
            "the ssh link stays a second transport beside the address")

    def test_a_command_without_a_target_names_no_server(self):
        document = serve.application_document('/tmp/fixture-data', server_command='/opt/horizon --server-link')
        self.assertNotIn('server', document)

    def test_the_fixture_option_of_an_address_is_the_entry_that_it_writes(self):
        parser = serve.arguments()
        args = parser.parse_args([
            '--horizon', '/bin/true', '--state', '/tmp/state', '--server-address', '127.0.0.1:7000'])
        self.assertEqual(args.server_address, '127.0.0.1:7000')
        args = parser.parse_args(['--horizon', '/bin/true', '--state', '/tmp/state'])
        self.assertIsNone(args.server_address, "the listener of a server is off without the option")

    def test_a_server_does_not_replace_the_panels_or_the_browser_host(self):
        document = serve.application_document(
            '/tmp/fixture-data', server_target='server-vm', browser_host='browser-vm', presets=[])
        self.assertIn('terminals', document['workspaces'][0])
        self.assertEqual(document['browser_host'], {'ssh_target': 'browser-vm'})
        self.assertEqual(document['server'], {'ssh_target': 'server-vm'})


class TwoFixturesAtOnce(unittest.TestCase):
    """Two fixtures run at the same time, each with its own state and display.

    The display comes from Xvfb's own free-display choice and every port is bound
    at zero, so the only thing a run must not share is its state: two runs with
    two paths keep two private homes, two private runtimes and two session buses.
    """

    def test_two_states_share_no_private_path(self):
        with tempfile.TemporaryDirectory() as parent:
            first = sandbox.prepare(sandbox.prepare_state(Path(parent) / 'a', reuse=False))
            second = sandbox.prepare(sandbox.prepare_state(Path(parent) / 'b', reuse=False))

            for left, right in (
                    (first.data, second.data),
                    (first.private_home, second.private_home),
                    (first.private_tmp, second.private_tmp),
                    (first.host_socket, second.host_socket)):
                self.assertNotEqual(left, right)
            self.assertEqual(
                first.bus_address, second.bus_address,
                'both namespaces see the standard bus path, each over its own socket')
            self.assertEqual(
                serve.application_document(str(first.data))['workspaces'][0]['name'],
                serve.application_document(str(second.data))['workspaces'][0]['name'],
                'two fixtures place their own boards in their own data folders')


if __name__ == '__main__':
    unittest.main()
