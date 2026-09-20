"""Capability contract and MCP reconciliation with a synthetic worker filesystem."""
import contextlib
import io
import json
import os
from pathlib import Path
import runpy
import subprocess
import tempfile
import tomllib
import unittest
from unittest import mock

ROOT = Path(__file__).parent
RUN = subprocess.run
WORKER = os.environ.get("HORIZON_TEST_CLOUD_WORKER")


class CapabilitiesTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.real_path = Path
        self.available = {'agents': ['codex', 'claude'], 'browsers': [], 'desktop': True}
        self.write('/etc/horizon-worker/capabilities.json', self.available)
        self.write('/workspace/capabilities.json', self.available)

    def path(self, value):
        if isinstance(value, str) and value.startswith(('/workspace/', '/etc/horizon-worker/')):
            return self.root / value.lstrip('/')
        return self.real_path(value)

    def write(self, name, value):
        path = self.path(name)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value))

    def run_check(self, *args, missing=()):
        output = io.StringIO()
        with mock.patch('pathlib.Path', side_effect=self.path), \
                mock.patch('sys.argv', ['horizon-worker-check', *args]), \
                mock.patch.dict(os.environ, {}, clear=True), \
                mock.patch('subprocess.run') as commands, \
                mock.patch('shutil.which', side_effect=lambda name: None if name in missing else '/bin/' + name), \
                contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            try:
                runpy.run_path(str(ROOT / 'horizon-worker-check'), run_name='__main__')
                status = 0
            except SystemExit as error:
                status = error.code
        return status, output.getvalue(), commands.call_args_list

    def test_native_contract_does_not_probe_browsers_or_unselected_agents(self):
        status, output, commands = self.run_check(missing=('grok', 'firefox', 'geckodriver', 'horizon-browser'))
        self.assertEqual(status, 0, output)
        self.assertIn('horizon-capabilities-contract=1', output)
        self.assertNotIn('grok', str(commands))
        self.assertNotIn('firefox', str(commands))
        self.assertIn('horizon-device', str(commands))

    def test_remote_only_requires_tools_and_tunnel_but_no_local_browser(self):
        selected = {'browserstack': {'targets': ['iphone'], 'local_ports': [8080]}}
        self.write('/workspace/capabilities.json', selected)
        self.assertEqual(self.run_check()[0], 1)
        self.write('/etc/horizon-worker/capabilities.json', {'browserstack': {'targets': []}})
        status, output, commands = self.run_check(missing=('firefox', 'geckodriver', 'google-chrome-stable', 'Xvfb'))
        self.assertEqual(status, 0, output)
        self.assertIn('horizon-browserstack-contract=1', output)
        self.assertIn('BrowserStackLocal', str(commands))
        for name in ['BrowserStackLocal', 'horizon-browser', 'horizon-worker-browserstack']:
            self.assertEqual(self.run_check(missing=(name,))[0], 1)

    def test_missing_requested_agent_or_desktop_fails(self):
        for binary in ['codex', 'claude', 'Xvfb', 'horizon-device']:
            status, _, _ = self.run_check(missing=(binary,))
            self.assertEqual(status, 1, binary)

    def test_unavailable_request_and_disabled_session_fail_without_writing_state(self):
        for args in [('--capabilities-json', '{"browsers":["firefox"]}'), ('--agent', 'grok')]:
            status, _, _ = self.run_check(*args)
            self.assertEqual(status, 1)
        self.assertFalse(self.path('/workspace/sessions').exists())
        self.assertFalse(self.path('/workspace/agents').exists())
        status, _, _ = self.run_check('--agent', 'shell')
        self.assertEqual(status, 0)

    def test_minimal_contract_does_not_require_optional_executables(self):
        self.write('/etc/horizon-worker/capabilities.json', {})
        self.write('/workspace/capabilities.json', {})
        status, output, commands = self.run_check(missing=('codex', 'claude', 'grok', 'Xvfb', 'horizon-device', 'horizon-browser'))
        self.assertEqual(status, 0, output)
        self.assertEqual([call.args[0] for call in commands], [['git', 'lfs', 'version']])

    def test_firefox_requires_its_driver(self):
        caps = {'browsers': ['firefox']}
        self.write('/etc/horizon-worker/capabilities.json', caps)
        self.write('/workspace/capabilities.json', caps)
        status, _, _ = self.run_check(missing=('geckodriver',))
        self.assertEqual(status, 1)
        status, output, commands = self.run_check(missing=('codex', 'claude', 'grok', 'Xvfb'))
        self.assertEqual(status, 0, output)
        self.assertIn('geckodriver', str(commands))

    def test_readiness_refuses_profile_drift_before_contacting_services(self):
        for active, requested in [(self.available, {}), ({}, self.available)]:
            self.write('/workspace/capabilities.json', active)
            with mock.patch('socket.create_connection') as connect:
                status, _, _ = self.run_check('--ready', '--capabilities-json', json.dumps(requested))
            self.assertEqual(status, 1)
            connect.assert_not_called()

    def test_readiness_probes_control_service_and_only_selected_desktop(self):
        for caps, expected in [({}, [47280]), (self.available, [47280, 5900])]:
            self.write('/workspace/capabilities.json', caps)
            connection = mock.MagicMock()
            stream = connection.__enter__.return_value.makefile.return_value.__enter__.return_value
            stream.readline.return_value = b'{"browsers":[],"error":null}\n'
            stream.read.return_value = b'RFB 003.008\n'
            with mock.patch('socket.create_connection', return_value=connection) as connect:
                status, output, _ = self.run_check('--ready')
            self.assertEqual(status, 0, output)
            self.assertEqual([call.args[0][1] for call in connect.call_args_list], expected)
            stream.readline.return_value = b'{"error":"not ready"}\n'
            with mock.patch('socket.create_connection', return_value=connection):
                status, _, _ = self.run_check('--ready')
            self.assertEqual(status, 1)

    def configure(self):
        def run(command, **kwargs):
            if command[0] == 'horizon-cloud-worker':
                return RUN([WORKER, *command[1:]], **kwargs)
            return subprocess.CompletedProcess(command, 0)
        with mock.patch('pathlib.Path', side_effect=self.path), mock.patch('subprocess.run', side_effect=run):
            runpy.run_path(str(ROOT / 'horizon-worker-configure'), run_name='__main__')

    @unittest.skipUnless(WORKER, "set HORIZON_TEST_CLOUD_WORKER to the matching built helper")
    def test_reconfigure_removes_disabled_managed_tools_and_preserves_other_settings(self):
        full = dict(self.available, browsers=['chromium'], agents=['codex', 'claude', 'grok'])
        self.write('/workspace/capabilities.json', full)
        self.configure()
        config = self.path('/workspace/home/.codex/config.toml')
        with config.open('a') as output:
            output.write('\n[mcp_servers.project]\ncommand = "project-tool"\n')
        self.write('/workspace/capabilities.json', self.available)
        self.configure()
        actual = tomllib.loads(config.read_text())['mcp_servers']
        self.assertEqual(set(actual), {'horizon-device', 'project'})
        self.assertEqual(actual['project']['command'], 'project-tool')
        servers = json.loads(self.path('/workspace/agent-mcp.json').read_text())['mcpServers']
        self.assertEqual(set(servers), {'horizon-device'})
        self.assertNotIn('horizon-browser', self.path('/workspace/home/.grok/config.toml').read_text())
        self.write('/workspace/capabilities.json', {})
        self.configure()
        self.assertEqual(set(tomllib.loads(config.read_text())['mcp_servers']), {'project'})
        self.assertEqual(json.loads(self.path('/workspace/agent-mcp.json').read_text())['mcpServers'], {})


if __name__ == '__main__':
    unittest.main()
