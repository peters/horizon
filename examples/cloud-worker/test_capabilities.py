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

    def run_check(self, *args, missing=(), environment=None, reported=None):
        output = io.StringIO()
        def run(command, **kwargs):
            return subprocess.CompletedProcess(command, 0, stdout=reported.get(command[0], b''))
        with mock.patch('pathlib.Path', side_effect=self.path), \
                mock.patch('sys.argv', ['horizon-worker-check', *args]), \
                mock.patch.dict(os.environ, environment or {}, clear=True), \
                mock.patch('subprocess.run', side_effect=run if reported is not None else None) as commands, \
                mock.patch('shutil.which', side_effect=lambda name: None if name in missing else '/bin/' + name), \
                contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            try:
                runpy.run_path(str(ROOT / 'horizon-worker-check'), run_name='__main__')
                status = 0
            except SystemExit as error:
                status = error.code
        return status, output.getvalue(), commands.call_args_list

    def test_idle_stop_is_reported_only_when_the_supervisor_declares_it(self):
        declared = {'horizon-worker-supervise': b'horizon-idle-stop-contract=1\n'}
        for reported, missing, expected in [(declared, (), True),
                                            ({'horizon-worker-supervise': b''}, (), False),
                                            ({'horizon-worker-supervise': b'horizon-idle-stop-contract=1 extra\n'}, (), False),
                                            (declared, ('horizon-worker-idle',), False)]:
            status, output, _ = self.run_check(missing=missing, reported=reported)
            self.assertEqual(status, 0, output)
            self.assertEqual('horizon-idle-stop-contract=1' in output.splitlines(), expected, (reported, missing))

    def test_old_python_source_apis_fail_before_runtime_probes(self):
        for module, attribute in [('hashlib', 'file_digest'), ('tarfile', 'data_filter')]:
            imported = __import__(module)
            original = getattr(imported, attribute)
            try:
                delattr(imported, attribute)
                status, output, commands = self.run_check()
            finally:
                setattr(imported, attribute, original)
            self.assertEqual(status, 1)
            self.assertIn('Worker contract requires Python', output)
            self.assertEqual(commands, [])

    def test_native_contract_does_not_probe_browsers_or_unselected_agents(self):
        status, output, commands = self.run_check(missing=('grok', 'firefox', 'geckodriver', 'horizon-browser'))
        self.assertEqual(status, 0, output)
        self.assertIn('horizon-capabilities-contract=1', output)
        self.assertIn('horizon-session-restart-contract=1', output.splitlines())
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
        for binary in ['codex', 'claude', 'Xvfb', 'xprop', 'xdpyinfo', 'horizon-device', 'horizon-worker-supervise']:
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
        self.assertEqual([call.args[0] for call in commands],
                         [['git', 'lfs', 'version'], ['horizon-worker-supervise', '--idle-stop-contract']])

    def test_environment_selection_rejects_full_defaults_on_minimal_images(self):
        self.write('/etc/horizon-worker/capabilities.json', {})
        self.write('/workspace/capabilities.json', {})
        full = {'agents': ['codex', 'claude', 'grok'], 'browsers': ['chromium'], 'desktop': True}
        status, _, commands = self.run_check(environment={'HORIZON_WORKER_CAPABILITIES': json.dumps(full)})
        self.assertEqual(status, 1)
        self.assertEqual(commands, [])
        status, output, _ = self.run_check(environment={'HORIZON_WORKER_CAPABILITIES': '{}'})
        self.assertEqual(status, 0, output)
        self.assertIn('horizon-capabilities-contract=1', output)

    def test_firefox_requires_its_driver(self):
        caps = {'browsers': ['firefox']}
        self.write('/etc/horizon-worker/capabilities.json', caps)
        self.write('/workspace/capabilities.json', caps)
        status, _, _ = self.run_check(missing=('geckodriver',))
        self.assertEqual(status, 1)
        status, output, commands = self.run_check(missing=('codex', 'claude', 'grok', 'Xvfb'))
        self.assertEqual(status, 0, output)
        self.assertIn('geckodriver', str(commands))

    def test_recorded_agent_versions_must_be_reported_by_each_selected_agent(self):
        self.write('/etc/horizon-worker/agent-versions.json', {'codex': '0.156.1', 'claude': '2.1.281', 'grok': '1.0.41'})
        reported = {'codex': b'codex-cli 0.156.1\n', 'claude': b'2.1.281 (Claude Code)\n'}
        status, output, commands = self.run_check(reported=reported)
        self.assertEqual(status, 0, output)
        for agent in ['codex', 'claude']:
            self.assertIn(mock.call([agent, '--version'], check=True, timeout=20,
                                    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL), commands)
        self.assertNotIn('grok', str(commands))
        for claude in [b'v2.1.281\n', b'Claude Code\n2.1.281\n']:
            status, output, _ = self.run_check(reported=dict(reported, claude=claude))
            self.assertEqual(status, 0, output)
        for claude in [b'2.1.280 (Claude Code)\n', b'2.1.2811\n', b'2.1.281-beta.1\n', b'2.1.281.4\n', b'12.1.281\n', b'1.0.0+2.1.281\n', b'1.0.0-2.1.281\n', b'']:
            status, output, _ = self.run_check(reported=dict(reported, claude=claude))
            self.assertEqual(status, 1, claude)
            self.assertIn('claude does not report its recorded version 2.1.281', output)

    def test_recorded_versions_must_cover_selected_agents_and_be_plain_versions(self):
        reported = {'codex': b'codex-cli 0.156.1\n', 'claude': b'2.1.281 (Claude Code)\n'}
        for record in [{'codex': '0.156.1'}, [], {'claude': '2.1.281', 'codex': '$(id)'},
                       {'claude': '2.1.281', 'codex': '0.156.1', 'other': '1.0.0'},
                       {'claude': '2.1.281', 'codex': '0.156.1-'}, {'claude': '2.1.281', 'codex': 156},
                       {'claude': '2.1.281', 'codex': '01.2.3'}, {'claude': '2.1.281', 'codex': '1.2.3-01'},
                       {'claude': '2.1.281', 'codex': '1.2.3-a..b'}, {'claude': '2.1.281', 'codex': '1.2.3+a..b'}]:
            self.write('/etc/horizon-worker/agent-versions.json', record)
            status, _, commands = self.run_check(reported=reported)
            self.assertEqual(status, 1, record)
            self.assertNotIn("'claude'", str(commands), record)
        self.path('/etc/horizon-worker/agent-versions.json').write_text('{')
        self.assertEqual(self.run_check(reported=reported)[0], 1)

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
                status, output, commands = self.run_check('--ready')
            self.assertEqual(status, 0, output)
            self.assertIn(mock.call(['horizon-worker-supervise', '--check'], check=True, timeout=20,
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL), commands)
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
        self.assertEqual(set(actual), {'horizon-device', 'horizon-cloud-companions', 'project'})
        self.assertEqual(actual['project']['command'], 'project-tool')
        servers = json.loads(self.path('/workspace/agent-mcp.json').read_text())['mcpServers']
        self.assertEqual(set(servers), {'horizon-device', 'horizon-cloud-companions'})
        self.assertEqual(servers['horizon-cloud-companions'], {
            'command': '/usr/local/bin/horizon-cloud-worker', 'args': ['companions', 'mcp']})
        self.assertNotIn('horizon-browser', self.path('/workspace/home/.grok/config.toml').read_text())
        self.assertNotIn('horizon-cloud-companions', self.path('/workspace/home/.grok/config.toml').read_text())
        self.write('/workspace/capabilities.json', {})
        self.configure()
        self.assertEqual(set(tomllib.loads(config.read_text())['mcp_servers']), {'project'})
        self.assertEqual(json.loads(self.path('/workspace/agent-mcp.json').read_text())['mcpServers'], {})


if __name__ == '__main__':
    unittest.main()
