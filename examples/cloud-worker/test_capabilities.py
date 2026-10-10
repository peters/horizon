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


# The credential a provider gives a worker to stop itself with, as RunPod does.
STOPS_ITSELF = {'RUNPOD_POD_ID': 'pod123', 'RUNPOD_API_KEY': 'pod-scoped'}


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

    def run_check(self, *args, missing=(), environment=None, reported=None, run=None, identities=(10001, 10001),
                  bin_dir='/bin'):
        output = io.StringIO()
        def reply(command, **kwargs):
            if command[:1] == ['/usr/bin/id']:
                value = identities[0 if command[1] == '-u' else 1]
                return subprocess.CompletedProcess(command, 0, stdout=str(value).encode())
            if run is not None:
                return run(command, **kwargs)
            return subprocess.CompletedProcess(command, 0, stdout=(reported or {}).get(command[0], b''))
        with mock.patch('pathlib.Path', side_effect=self.path), \
                mock.patch('sys.argv', ['horizon-worker-check', *args]), \
                mock.patch.dict(os.environ, environment or {}, clear=True), \
                mock.patch('subprocess.run', side_effect=reply) as commands, \
                mock.patch('shutil.which', side_effect=lambda name: None if name in missing else f'{bin_dir}/{name}'), \
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

    def test_the_idle_record_is_reported_only_with_idle_stop_and_a_watcher_that_keeps_one(self):
        declared = {'horizon-worker-supervise': b'horizon-idle-stop-contract=1\n',
                    'horizon-worker-idle': b'horizon-idle-report-contract=1\n'}
        older = {'horizon-worker-supervise': b'horizon-idle-stop-contract=1\n', 'horizon-worker-idle': b''}
        for reported, missing, expected in [(declared, (), True), (older, (), False),
                                            ({**declared, 'horizon-worker-supervise': b''}, (), False),
                                            (declared, ('horizon-worker-idle',), False)]:
            status, output, _ = self.run_check(missing=missing, reported=reported)
            self.assertEqual(status, 0, output)
            self.assertEqual('horizon-idle-report-contract=1' in output.splitlines(), expected, (reported, missing))

    def test_github_chain_is_reported_only_with_the_service_and_a_supervisor_that_starts_it(self):
        marker = 'horizon-github-chain-contract=1'
        declared = {'horizon-worker-supervise': (marker + '\n').encode()}
        binaries = self.root / 'bin'
        binaries.mkdir()
        (binaries / 'horizon-worker-github').touch()
        # Helpers elsewhere on PATH do not count: the service loads them beside itself.
        elsewhere = self.root / 'elsewhere'
        elsewhere.mkdir()
        helpers = ('horizon-worker-github-common', 'horizon-worker-github-agents', 'horizon-worker-github-git',
                   'horizon-worker-github-http', 'horizon-worker-git-auth')
        for helper in helpers:
            (elsewhere / helper).touch()
        for reported, missing, args, beside, expected in [
                (declared, (), ('--git-auth',), helpers, True),
                ({'horizon-worker-supervise': b''}, (), ('--git-auth',), helpers, False),
                (declared, ('horizon-worker-github',), ('--git-auth',), helpers, False),
                (declared, (), ('--git-auth',), ('horizon-worker-git-auth', 'horizon-worker-github-agents'), False),
                (declared, (), ('--git-auth',), ('horizon-worker-github-common', 'horizon-worker-git-auth'), False),
                (declared, (), ('--git-auth',), ('horizon-worker-github-common', 'horizon-worker-github-agents'),
                 False),
                # Without the agent isolation launcher the service would refuse to run.
                (declared, ('horizon-worker-tailnet',), ('--git-auth',), helpers, False),
                # Without openssl the Git proxy cannot make its certificate authority.
                (declared, ('openssl',), ('--git-auth',), helpers, False),
                (declared, (), (), helpers, False)]:
            for helper in helpers:
                path = binaries / helper
                path.unlink(missing_ok=True)
                if helper in beside:
                    path.touch(mode=0o755)
            with mock.patch('os.readlink', return_value='/usr/local/bin/horizon-worker-git-auth'):
                status, output, _ = self.run_check(*args, missing=missing, reported=reported, bin_dir=str(binaries))
            self.assertEqual(status, 0, output)
            self.assertEqual(marker in output.splitlines(), expected, (reported, missing, args, beside))
        # A Git helper beside the service that cannot run does not count either.
        (binaries / 'horizon-worker-git-auth').chmod(0o644)
        with mock.patch('os.readlink', return_value='/usr/local/bin/horizon-worker-git-auth'):
            _, output, _ = self.run_check('--git-auth', reported=declared, bin_dir=str(binaries))
        self.assertNotIn(marker, output.splitlines())

    def test_source_features_are_reported_only_when_the_source_helper_declares_them(self):
        for option, marker in [('--shallow-contract', 'horizon-source-shallow-contract=1'),
                               ('--lfs-selection-contract', 'horizon-source-lfs-selection-contract=1')]:
            for reply, expected in [((marker + '\n').encode(), True), (b'', False), ((marker + ' extra\n').encode(), False)]:
                def run(command, **kwargs):
                    declared = command == ['horizon-worker-source', option]
                    return subprocess.CompletedProcess(command, 0, stdout=reply if declared else b'')
                status, output, _ = self.run_check(run=run)
                self.assertEqual(status, 0, output)
                lines = output.splitlines()
                self.assertEqual(marker in lines, expected, (option, reply))
                self.assertEqual(len([line for line in lines if line.startswith('horizon-source-') and line != 'horizon-source-contract=1']),
                                 int(expected), (option, reply))

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

    def test_a_base_without_bash_4_4_at_bin_bash_fails(self):
        def run(command, **kwargs):
            if command[0] == '/bin/bash':
                raise subprocess.CalledProcessError(1, command)
            return subprocess.CompletedProcess(command, 0, stdout=b'')
        status, output, _ = self.run_check(run=run)
        self.assertEqual(status, 1)
        self.assertIn('Worker contract requires Bash 4.4 or newer at /bin/bash', output)

    def test_native_contract_does_not_probe_browsers_or_unselected_agents(self):
        status, output, commands = self.run_check(missing=('grok', 'firefox', 'geckodriver', 'horizon-browser'))
        self.assertEqual(status, 0, output)
        self.assertIn('horizon-capabilities-contract=1', output)
        self.assertIn('horizon-session-restart-contract=1', output.splitlines())
        self.assertNotIn('grok', str(commands))
        self.assertNotIn('firefox', str(commands))
        self.assertIn('horizon-device', str(commands))

    def test_siblings_are_reported_only_with_the_manifest_helper(self):
        for missing, expected in [((), True), (('horizon-worker-siblings',), False)]:
            status, output, _ = self.run_check(missing=missing)
            self.assertEqual(status, 0, output)
            self.assertEqual('horizon-siblings-contract=1' in output.splitlines(), expected, missing)
        # A session-only check reports no contract markers.
        status, output, _ = self.run_check('--agent', 'shell')
        self.assertEqual((status, output), (0, ''))

    def test_session_environment_and_gpu_lock_are_reported_only_with_their_helpers(self):
        for marker, helper in [('horizon-session-env-contract=1', 'horizon-worker-session-env'),
                               ('horizon-gpu-lock-contract=1', 'horizon-worker-gpu-lock')]:
            for missing, expected in [((), True), ((helper,), False)]:
                status, output, _ = self.run_check(missing=missing)
                self.assertEqual(status, 0, output)
                self.assertEqual(marker in output.splitlines(), expected, missing)

    def test_refused_session_environment_fails_the_image_and_every_session_start(self):
        def run(command, **kwargs):
            if command[0] == 'horizon-worker-session-env':
                raise subprocess.CalledProcessError(1, command)
            return subprocess.CompletedProcess(command, 0, stdout=b'')
        for args in [(), ('--agent', 'shell'), ('--ready',)]:
            status, output, _ = self.run_check(*args, run=run)
            self.assertEqual(status, 1, args)
            self.assertNotIn('contract=1', output)
        # Without the helper there is nothing to validate.
        status, output, _ = self.run_check('--agent', 'shell', missing=('horizon-worker-session-env',))
        self.assertEqual((status, output), (0, ''))

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
                         [['horizon-worker-session-env', 'check'],
                          ['/bin/bash', '-c', '(( BASH_VERSINFO[0] * 100 + BASH_VERSINFO[1] >= 404 ))'],
                          ['git', 'lfs', 'version'],
                          ['horizon-worker-supervise', '--idle-stop-contract'],
                          ['horizon-worker-source', '--shallow-contract'],
                          ['horizon-worker-source', '--lfs-selection-contract'],
                          ['/usr/bin/setpriv', '--help'],
                          ['/usr/bin/id', '-u', 'horizon-agent'],
                          ['/usr/bin/id', '-g', 'horizon-agent'],
                          ['horizon-worker-tailnet', '--stable-name-contract'],
                          ['horizon-worker-tailnet', '--tagged-enrollment-contract']])

    def test_tailnet_contract_refuses_mismatched_isolation_uid_or_gid(self):
        for identity in [(0, 10001), (10002, 10001), (10001, 0), (10001, 10002)]:
            with self.subTest(identity=identity):
                status, output, _ = self.run_check(identities=identity)
                self.assertEqual(status, 1)
                self.assertNotIn('horizon-tailnet-contract=1', output)

    def test_current_helpers_require_valid_isolation_even_without_tailscale(self):
        for identities, expected in [((10001, 10001), 0), ((10002, 10001), 1), ((10001, 10002), 1)]:
            with self.subTest(identities=identities):
                status, output, _ = self.run_check(missing=('tailscale', 'tailscaled'), identities=identities)
                self.assertEqual(status, expected, output)
                self.assertNotIn('horizon-tailnet-contract=1', output)

    def test_stable_tailnet_names_are_reported_only_when_the_tailnet_helper_declares_them(self):
        marker = 'horizon-tailnet-contract=2'
        older = (1, b'')  # An older helper rejects the unknown option.
        for (code, reply), missing, expected in [((0, (marker + '\n').encode()), (), True), ((0, b''), (), False),
                                                 ((0, (marker + ' extra\n').encode()), (), False), (older, (), False),
                                                 ((0, (marker + '\n').encode()), ('tailscale',), False)]:
            with self.subTest(code=code, reply=reply, missing=missing):
                def run(command, **kwargs):
                    declared = command == ['horizon-worker-tailnet', '--stable-name-contract']
                    return subprocess.CompletedProcess(command, code if declared else 0, stdout=reply if declared else b'')
                status, output, _ = self.run_check(run=run, missing=missing)
                self.assertEqual(status, 0, output)
                self.assertEqual(marker in output.splitlines(), expected)

    def test_tagged_enrollment_is_reported_only_when_the_helper_declares_it(self):
        marker = 'horizon-tailnet-contract=3'
        for code, reply, expected in [(0, (marker + '\n').encode(), True), (0, b'', False),
                                      (0, (marker + ' extra\n').encode(), False), (1, b'', False)]:
            with self.subTest(code=code, reply=reply):
                def run(command, **kwargs):
                    declared = command == ['horizon-worker-tailnet', '--tagged-enrollment-contract']
                    return subprocess.CompletedProcess(command, code if declared else 0, stdout=reply if declared else b'')
                status, output, _ = self.run_check(run=run)
                self.assertEqual(status, 0, output)
                self.assertEqual(marker in output.splitlines(), expected)

    def test_tailnet_contract_requires_the_privilege_drop_runtime(self):
        status, output, _ = self.run_check(missing=('/usr/bin/setpriv',))
        self.assertEqual(status, 1)
        self.assertNotIn('horizon-tailnet-contract=1', output)

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
            self.assertIn(mock.call([agent, '--version'], check=True, timeout=20, env=mock.ANY,
                                    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL), commands)
        self.assertNotIn('grok', str(commands))
        for claude in [b'v2.1.281\n', b'Claude Code\n2.1.281\n']:
            status, output, _ = self.run_check(reported=dict(reported, claude=claude))
            self.assertEqual(status, 0, output)
        for claude in [b'2.1.280 (Claude Code)\n', b'2.1.2811\n', b'2.1.281-beta.1\n', b'2.1.281.4\n', b'12.1.281\n', b'1.0.0+2.1.281\n', b'1.0.0-2.1.281\n', b'']:
            status, output, _ = self.run_check(reported=dict(reported, claude=claude))
            self.assertEqual(status, 1, claude)
            self.assertIn('claude does not report its recorded version 2.1.281', output)

    def test_a_layer_record_naming_only_its_own_agent_pins_that_agent(self):
        reported = {'codex': b'codex-cli 0.156.1\n', 'claude': b'2.1.281 (Claude Code)\n'}
        self.write('/etc/horizon-worker/agent-versions.json', {'codex': '0.156.1'})
        status, output, commands = self.run_check(reported=reported)
        self.assertEqual(status, 0, output)
        self.assertIn(mock.call(['codex', '--version'], check=True, timeout=20, env=mock.ANY,
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL), commands)
        self.assertIn(mock.call(['claude', '--version'], check=True, timeout=20, env=mock.ANY,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL), commands)
        status, output, _ = self.run_check(reported=dict(reported, codex=b'codex-cli 0.156.2\n'))
        self.assertEqual(status, 1)
        self.assertIn('codex does not report its recorded version 0.156.1', output)

    def test_agent_probes_never_write_into_the_workspace_home(self):
        # The check runs as root after the workspace home was handed to the isolated agent.
        # An agent CLI that creates its state directory on --version (Codex creates ~/.codex)
        # must not leave a root-owned directory there, or the agent cannot configure itself.
        reported = {'codex': b'codex-cli 0.156.1\n', 'claude': b'2.1.281 (Claude Code)\n'}
        environment = {'HOME': '/workspace/home', 'CODEX_HOME': '/workspace/home/.codex', 'PATH': '/usr/bin'}
        for record in [None, {'codex': '0.156.1'}]:
            if record is not None:
                self.write('/etc/horizon-worker/agent-versions.json', record)
            status, output, commands = self.run_check(environment=environment, reported=reported)
            self.assertEqual(status, 0, output)
            probes = [call for call in commands if call.args[0][1:] == ['--version'] and call.args[0][0] in {'codex', 'claude'}]
            self.assertEqual(sorted(call.args[0][0] for call in probes), ['claude', 'codex'], record)
            for call in probes:
                probe = call.kwargs['env']
                self.assertFalse(probe['HOME'].startswith('/workspace'), (record, probe))
                self.assertNotIn('CODEX_HOME', probe)
                self.assertEqual(probe['PATH'], '/usr/bin')

    def test_recorded_versions_must_be_plain_versions_of_known_agents(self):
        reported = {'codex': b'codex-cli 0.156.1\n', 'claude': b'2.1.281 (Claude Code)\n'}
        for record in [[], {'claude': '2.1.281', 'codex': '$(id)'},
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
            # Readiness also reports when the container started, from the kernel's PID 1 record.
            started = [line for line in output.splitlines() if line.startswith('horizon-container-started=')]
            self.assertEqual(len(started), 1, output)
            self.assertGreater(int(started[0].split('=', 1)[1]), 1_600_000_000_000)
            self.assertIn(mock.call(['horizon-worker-supervise', '--check'], check=True, timeout=20,
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL), commands)
            self.assertEqual([call.args[0][1] for call in connect.call_args_list], expected)
            stream.readline.return_value = b'{"error":"not ready"}\n'
            with mock.patch('socket.create_connection', return_value=connection):
                status, _, _ = self.run_check('--ready')
            self.assertEqual(status, 1)

    def test_readiness_reports_the_newest_stop_an_agent_asked_for(self):
        self.write('/workspace/capabilities.json', {})
        connection = mock.MagicMock()
        stream = connection.__enter__.return_value.makefile.return_value.__enter__.return_value
        stream.readline.return_value = b'{"browsers":[],"error":null}\n'
        reported = {'horizon-worker-stop': b'{"at": 1790000000000, "reason": "PR 12 merged"}\n'}
        with mock.patch('socket.create_connection', return_value=connection):
            status, output, _ = self.run_check('--ready', reported=reported)
        self.assertEqual(status, 0, output)
        self.assertIn('horizon-last-self-stop={"at":1790000000000,"reason":"PR 12 merged"}', output.splitlines())
        self.assertIn('horizon-self-stop-contract=1', output.splitlines())
        # Without a record the image still says it supports them; an older image says neither.
        # A failed read reports neither, so Horizon keeps the reason it showed.
        for reported, missing, supported in [({'horizon-worker-stop': b'null\n'}, (), True),
                                             ({'horizon-worker-stop': b'not json\n'}, (), False),
                                             ({}, ('horizon-worker-stop',), False)]:
            with mock.patch('socket.create_connection', return_value=connection):
                status, output, _ = self.run_check('--ready', reported=reported, missing=missing)
            self.assertEqual(status, 0, output)
            self.assertNotIn('horizon-last-self-stop=', output)
            self.assertEqual('horizon-self-stop-contract=1' in output.splitlines(), supported)

    def test_agents_get_the_stop_tool_only_where_the_profile_opts_in(self):
        self.write('/workspace/capabilities.json', {'agents': ['claude']})
        for environment, expected in [(dict(STOPS_ITSELF, HORIZON_IDLE_STOP_MINUTES='30'), True), ({}, False),
                                      # Horizon stops a worker without a credential, as on Hetzner.
                                      ({'HORIZON_IDLE_STOP_MINUTES': '30'}, False),
                                      # A period the watcher refuses leaves it passive.
                                      (dict(STOPS_ITSELF, HORIZON_IDLE_STOP_MINUTES='5'), False),
                                      (dict(STOPS_ITSELF, HORIZON_IDLE_STOP_MINUTES='1441'), False),
                                      ({'HORIZON_WORKER_SELF_STOP_AVAILABLE': '1', 'HORIZON_IDLE_STOP_MINUTES': '30'}, True)]:
            with mock.patch.dict(os.environ, environment, clear=True):
                self.configure()
            servers = json.loads(self.path('/workspace/agent-mcp.json').read_text())['mcpServers']
            self.assertEqual('horizon-worker' in servers, expected, environment)
        with mock.patch.dict(os.environ, dict(STOPS_ITSELF, HORIZON_IDLE_STOP_MINUTES='30'), clear=True):
            self.configure()
        self.assertEqual(json.loads(self.path('/workspace/agent-mcp.json').read_text())['mcpServers']['horizon-worker'],
                         {'command': '/usr/local/bin/horizon-worker-stop', 'args': ['mcp']})

    def test_agents_get_the_github_access_tool_where_the_image_has_the_service(self):
        for agents, service, expected in [(['claude'], '/usr/local/bin/horizon-worker-github', True),
                                          (['claude'], None, False), ([], '/usr/local/bin/horizon-worker-github', False)]:
            self.write('/workspace/capabilities.json', {'agents': agents})
            with mock.patch.dict(os.environ, {}, clear=True), \
                    mock.patch('shutil.which', side_effect=lambda name: service if name == 'horizon-worker-github' else None):
                self.configure()
            servers = json.loads(self.path('/workspace/agent-mcp.json').read_text())['mcpServers']
            self.assertEqual(servers.get('horizon-github'), {'command': '/usr/local/bin/horizon-worker-github',
                                                             'args': ['mcp']} if expected else None, (agents, service))

    @unittest.skipUnless(WORKER, "set HORIZON_TEST_CLOUD_WORKER to the matching built helper")
    def test_codex_and_grok_accept_the_stop_tool_and_drop_it_when_opted_out(self):
        self.write('/workspace/capabilities.json', {'agents': ['codex', 'grok']})
        with mock.patch.dict(os.environ, dict(STOPS_ITSELF, HORIZON_IDLE_STOP_MINUTES='30'), clear=True):
            self.configure()
        for agent in ['codex', 'grok']:
            servers = tomllib.loads(self.path(f'/workspace/home/.{agent}/config.toml').read_text())['mcp_servers']
            self.assertEqual(servers['horizon-worker']['command'], '/usr/local/bin/horizon-worker-stop')
        with mock.patch.dict(os.environ, {}, clear=True):
            self.configure()
        for agent in ['codex', 'grok']:
            servers = tomllib.loads(self.path(f'/workspace/home/.{agent}/config.toml').read_text())['mcp_servers']
            self.assertNotIn('horizon-worker', servers)

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
        self.assertEqual(set(actual), {'horizon-device', 'horizon-cloud-companions', 'horizon-local-network', 'project'})
        self.assertEqual(actual['project']['command'], 'project-tool')
        servers = json.loads(self.path('/workspace/agent-mcp.json').read_text())['mcpServers']
        self.assertEqual(set(servers), {'horizon-device', 'horizon-cloud-companions', 'horizon-local-network'})
        self.assertEqual(servers['horizon-cloud-companions'], {
            'command': '/usr/local/bin/horizon-cloud-worker', 'args': ['companions', 'mcp']})
        self.assertNotIn('horizon-browser', self.path('/workspace/home/.grok/config.toml').read_text())
        self.assertEqual(servers['horizon-local-network'], {
            'command': '/usr/local/bin/horizon-cloud-worker', 'args': ['local-network', 'mcp']})
        self.assertNotIn('horizon-cloud-companions', self.path('/workspace/home/.grok/config.toml').read_text())
        self.assertNotIn('horizon-local-network', self.path('/workspace/home/.grok/config.toml').read_text())
        self.write('/workspace/capabilities.json', {})
        self.configure()
        self.assertEqual(set(tomllib.loads(config.read_text())['mcp_servers']), {'project'})
        self.assertEqual(json.loads(self.path('/workspace/agent-mcp.json').read_text())['mcpServers'], {})


if __name__ == '__main__':
    unittest.main()
