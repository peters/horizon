"""Exercise the agent launcher with synthetic child executables and credentials."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


def root_available():
    # CI runners grant passwordless sudo; the two-user test then runs there as root.
    if os.geteuid() == 0:
        return True
    return bool(shutil.which('sudo')) and subprocess.run(
        ['sudo', '-n', 'true'], capture_output=True, timeout=20, check=False).returncode == 0


class AgentRuntimeTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.workspace = self.root / 'workspace'
        (self.workspace / 'home/.horizon').mkdir(parents=True)
        # Published by the root control service; its browser runtime root stays private.
        self.run_dir = self.root / 'run/horizon-worker'
        self.run_dir.mkdir(parents=True)
        (self.run_dir / 'browser-host-instance').write_text('test-host\n')
        (self.workspace / 'sessions/test-panel').mkdir(parents=True)
        credentials = self.workspace / 'credentials'
        credentials.mkdir()
        (credentials / 'anthropic-api-key').write_text('synthetic-key\n')
        (credentials / 'anthropic-workspace').write_text('synthetic-workspace\n')
        tools = self.root / 'bin'
        tools.mkdir()
        child = '''import json, os, sys
from pathlib import Path
keys = ['DISABLE_AUTOUPDATER', 'HOME', 'HORIZON', 'HORIZON_BROWSER_ACTOR',
        'HORIZON_BROWSER_HOST_INSTANCE', 'ANTHROPIC_API_KEY',
        'ANTHROPIC_WORKSPACE_ID', 'ANTHROPIC_CUSTOM_HEADERS', 'HORIZON_SESSION_DIR', 'HORIZON_GPU_LOCK']
Path(os.environ['CHILD_RECEIPT']).write_text(json.dumps({
    'args': sys.argv[1:], 'env': {key: os.environ.get(key) for key in keys},
    'path': os.environ['PATH'], 'toolkit': os.environ.get('TOOLKIT'),
    'layer': {key: os.environ[key] for key in ['_', 'IFS', 'BASH_ALIASES', 'UID'] if key in os.environ}}))
sys.exit(23)
'''
        for name, body in [('claude', child), ('grok', child), ('sleep', 'import sys\nsys.exit(0)\n')]:
            tool = tools / name
            tool.write_text('#!' + sys.executable + '\n' + body)
            tool.chmod(0o700)
        self.fragments = self.root / 'session-env.d'
        helper = tools / 'horizon-worker-session-env'
        helper.write_text('#!' + sys.executable + '\n' + Path(__file__).with_name('horizon-worker-session-env')
                          .read_text().replace('/etc/horizon-worker/session-env.d', str(self.fragments)))
        helper.chmod(0o700)
        self.tools = tools
        self.script = self.root / 'horizon-worker-run'
        source = Path(__file__).with_name('horizon-worker-run').read_text()
        self.script.write_text(source.replace('/workspace', str(self.workspace))
                               .replace('/run/horizon-worker', str(self.run_dir)))
        self.env = {'PATH': str(tools) + os.pathsep + os.defpath,
                    'CHILD_RECEIPT': str(self.root / 'child.json'), 'HORIZON': 'parent-host'}

    def launch(self, agent='claude', **environment):
        result = subprocess.run(['bash', str(self.script), 'test-panel', agent],
                                env=dict(self.env, **environment), capture_output=True,
                                text=True, timeout=10, check=True)
        self.assertEqual((self.workspace / 'sessions/test-panel/exit-status').read_text(), '23\n')
        self.assertIn('Session process exited with status 23', result.stdout)
        self.stderr = result.stderr
        return json.loads((self.root / 'child.json').read_text())

    def test_minimal_launch_disables_background_updates_and_preserves_contract(self):
        child = self.launch()
        self.assertEqual(child['args'][0], '--session-id')
        self.assertEqual(len(child['args'][1]), 36)
        self.assertEqual(child['args'][2:], ['--mcp-config', str(self.workspace / 'agent-mcp.json')])
        again = self.launch()
        self.assertEqual(again['args'], child['args'])
        project = self.workspace / 'home/.claude/projects/repo'
        project.mkdir(parents=True)
        (project / f"{child['args'][1]}.jsonl").write_text('{}\n')
        resumed = self.launch()
        self.assertEqual(resumed['args'], ['--resume', child['args'][1], '--mcp-config', str(self.workspace / 'agent-mcp.json')])
        self.assertEqual(child['env'], {
            'DISABLE_AUTOUPDATER': '1', 'HOME': str(self.workspace / 'home'),
            'HORIZON': None, 'HORIZON_BROWSER_ACTOR': 'horizon:cloud-test-panel',
            'HORIZON_BROWSER_HOST_INSTANCE': 'test-host', 'ANTHROPIC_API_KEY': 'synthetic-key',
            'ANTHROPIC_WORKSPACE_ID': 'synthetic-workspace',
            'ANTHROPIC_CUSTOM_HEADERS': 'anthropic-workspace-id: synthetic-workspace',
            'HORIZON_SESSION_DIR': str(self.workspace / 'session-data/test-panel'),
            'HORIZON_GPU_LOCK': str(self.workspace / 'locks/gpu.lock')})
        self.assertTrue((self.workspace / 'locks').is_dir())
        self.assertEqual(child['path'], self.env['PATH'])
        self.assertIsNone(child['toolkit'])

    def test_host_instance_comes_from_the_published_copy_not_the_private_runtime_root(self):
        private = self.workspace / 'home/.horizon'
        (private / 'cloud-host-instance').write_text('private-host\n')
        private.chmod(0)
        self.addCleanup(private.chmod, 0o700)
        child = self.launch('grok', HORIZON_BROWSER_HOST_INSTANCE='inherited-host')
        self.assertEqual(child['env']['HORIZON_BROWSER_HOST_INSTANCE'], 'test-host')
        self.assertEqual(self.stderr, '', 'an agent panel starts without errors')

    def test_missing_host_instance_is_reported_and_never_inherited(self):
        (self.run_dir / 'browser-host-instance').unlink()
        child = self.launch('grok', HORIZON_BROWSER_HOST_INSTANCE='inherited-host')
        self.assertIsNone(child['env']['HORIZON_BROWSER_HOST_INSTANCE'])
        self.assertIn('has not published its browser host instance', self.stderr)
        self.assertEqual(child['args'], ['--no-leader'], 'the agent still starts')

    @unittest.skipIf(os.geteuid() == 0, 'root reads a file of any mode')
    def test_unreadable_host_instance_takes_the_missing_path_without_a_shell_error(self):
        published = self.run_dir / 'browser-host-instance'
        published.chmod(0)
        self.addCleanup(published.chmod, 0o644)
        child = self.launch('grok')
        self.assertIsNone(child['env']['HORIZON_BROWSER_HOST_INSTANCE'])
        self.assertIn('has not published its browser host instance', self.stderr)
        self.assertNotIn('Permission denied', self.stderr)

    def test_empty_host_instance_is_not_exported(self):
        (self.run_dir / 'browser-host-instance').write_text('')
        child = self.launch('grok')
        self.assertIsNone(child['env']['HORIZON_BROWSER_HOST_INSTANCE'])
        self.assertIn('has not published its browser host instance', self.stderr)

    @unittest.skipUnless(shutil.which('setpriv') and shutil.which('chown') and root_available(),
                         'needs root or passwordless sudo to run the launcher as the worker agent')
    def test_unprivileged_agent_reads_the_value_but_not_root_private_state(self):
        if os.geteuid() != 0:
            name = f'{Path(__file__).stem}.{type(self).__name__}.{self._testMethodName}'
            nested = subprocess.run(['sudo', '-n', sys.executable, '-B', '-m', 'unittest', name],
                                    cwd=Path(__file__).parent, capture_output=True, text=True, timeout=120)
            self.assertEqual(nested.returncode, 0, nested.stderr)
            self.assertNotIn('skipped', nested.stderr)
            return
        # Ownership as on an isolated worker: root keeps the browser runtime root, the
        # agent owns the rest of the workspace and runs the launcher as UID 10001.
        private = self.workspace / 'home/.horizon'
        (private / 'cloud-host-instance').write_text('private-host\n')
        (private / 'cloud-browser-history').mkdir()
        os.chmod(private / 'cloud-host-instance', 0o600)
        self.root.chmod(0o755)
        (self.root / 'run').chmod(0o755)
        self.run_dir.chmod(0o755)
        (self.run_dir / 'browser-host-instance').chmod(0o644)
        for path in [self.tools, *self.tools.iterdir(), self.script]:
            path.chmod(0o755)
        subprocess.run(['chown', '-R', '10001:10001', str(self.workspace)], check=True)
        subprocess.run(['chown', '-R', '0:0', str(private)], check=True)
        private.chmod(0o700)
        receipt = self.root / 'agent'
        receipt.mkdir()
        os.chown(receipt, 10001, 10001)
        environment = dict(self.env, CHILD_RECEIPT=str(receipt / 'child.json'))
        result = subprocess.run(['setpriv', '--reuid=10001', '--regid=10001', '--clear-groups', '--',
                                 'bash', str(self.script), 'test-panel', 'grok'],
                                env=environment, capture_output=True, text=True, timeout=10, check=True)
        self.assertEqual(result.stderr, '')
        child = json.loads((receipt / 'child.json').read_text())
        self.assertEqual(child['env']['HORIZON_BROWSER_HOST_INSTANCE'], 'test-host')
        for path in [private / 'cloud-host-instance', private / 'cloud-browser-history']:
            denied = subprocess.run(['setpriv', '--reuid=10001', '--regid=10001', '--clear-groups', '--',
                                     'ls', str(path)], capture_output=True, text=True, timeout=10)
            self.assertNotEqual(denied.returncode, 0, path)
            self.assertIn('Permission denied', denied.stderr)

    def claude_config(self):
        return json.loads((self.workspace / 'home/.claude.json').read_text())

    def test_claude_starts_without_first_run_dialogs(self):
        key = 'sk-ant-api03-' + 'x' * 30 + 'ABCDEFGHIJKLMNOPQRST'
        (self.workspace / 'credentials/anthropic-api-key').write_text(key + '\n')
        self.launch()
        config = self.claude_config()
        self.assertTrue(config['hasCompletedOnboarding'])
        self.assertEqual(config['theme'], 'dark')
        self.assertEqual(config['customApiKeyResponses'], {'approved': [key[-20:]], 'rejected': []})
        self.assertEqual(config['projects'][str(self.workspace)],
                         {'hasTrustDialogAccepted': True, 'hasCompletedProjectOnboarding': True})
        self.assertEqual((self.workspace / 'home/.claude.json').stat().st_mode & 0o777, 0o600)
        self.assertNotIn(key, (self.workspace / 'home/.claude.json').read_text(), 'only the key tail is kept')

    def test_seeding_keeps_what_claude_saved_and_is_repeatable(self):
        key = 'sk-ant-api03-' + 'y' * 40
        (self.workspace / 'credentials/anthropic-api-key').write_text(key + '\n')
        saved = {'theme': 'light', 'numStartups': 7, 'projects': {'/elsewhere': {'allowedTools': ['Bash']}},
                 'customApiKeyResponses': {'approved': ['older'], 'rejected': [key[-20:]]}}
        (self.workspace / 'home/.claude.json').write_text(json.dumps(saved))
        self.launch()
        first = self.claude_config()
        self.assertEqual(first['theme'], 'light', 'a theme the person chose stays')
        self.assertEqual(first['numStartups'], 7)
        self.assertEqual(first['projects']['/elsewhere'], {'allowedTools': ['Bash']})
        self.assertEqual(first['customApiKeyResponses'], {'approved': ['older', key[-20:]], 'rejected': []})
        self.launch()
        self.assertEqual(self.claude_config(), first)

    def test_a_key_no_longer_than_the_approval_tail_is_never_copied(self):
        short = 'synthetic-key'
        (self.workspace / 'credentials/anthropic-api-key').write_text(short + '\n')
        self.launch()
        raw = (self.workspace / 'home/.claude.json').read_text()
        self.assertNotIn(short, raw)
        self.assertNotIn('customApiKeyResponses', json.loads(raw))

    def test_a_launch_that_finds_everything_in_place_does_not_rewrite_the_file(self):
        (self.workspace / 'credentials/anthropic-api-key').write_text('sk-ant-api03-' + 'z' * 40 + '\n')
        self.launch()
        config = self.workspace / 'home/.claude.json'
        before = config.stat()
        # Claude's own later save must survive the next launch.
        saved = json.loads(config.read_text())
        saved['numStartups'] = 3
        config.write_text(json.dumps(saved))
        mtime = config.stat().st_mtime_ns
        self.launch()
        self.assertEqual(config.stat().st_mtime_ns, mtime, 'nothing to add, so nothing is written')
        self.assertEqual(self.claude_config()['numStartups'], 3)
        self.assertGreaterEqual(mtime, before.st_mtime_ns)

    def test_a_subscription_worker_approves_no_key(self):
        (self.workspace / 'credentials/anthropic-api-key').unlink()
        self.launch()
        config = self.claude_config()
        self.assertTrue(config['hasCompletedOnboarding'])
        self.assertNotIn('customApiKeyResponses', config)

    def test_a_config_claude_cannot_read_is_replaced_not_fatal(self):
        (self.workspace / 'home/.claude.json').write_text('{ not json')
        child = self.launch()
        self.assertEqual(child['args'][0], '--session-id', 'the agent still starts')
        self.assertTrue(self.claude_config()['hasCompletedOnboarding'])

    def test_other_agents_leave_the_claude_config_alone(self):
        self.launch('grok')
        self.assertFalse((self.workspace / 'home/.claude.json').exists())

    def test_inherited_environment_cannot_enable_background_updates(self):
        self.assertEqual(self.launch(DISABLE_AUTOUPDATER='0')['env']['DISABLE_AUTOUPDATER'], '1')

    def test_other_agent_keeps_its_launch_environment_and_arguments(self):
        child = self.launch('grok')
        self.assertIsNone(child['env']['DISABLE_AUTOUPDATER'])
        self.assertIsNone(child['env']['ANTHROPIC_API_KEY'])
        self.assertEqual(child['args'], ['--no-leader'])
        self.assertEqual(child['env']['HORIZON_SESSION_DIR'], str(self.workspace / 'session-data/test-panel'))

    def test_inherited_session_directory_is_replaced(self):
        child = self.launch(HORIZON_SESSION_DIR='/elsewhere')
        self.assertEqual(child['env']['HORIZON_SESSION_DIR'], str(self.workspace / 'session-data/test-panel'))

    def test_image_layers_extend_the_session_environment(self):
        self.fragments.mkdir()
        (self.fragments / '20-library.env').write_text('PATH_PREPEND=/opt/cuda/bin\nTOOLKIT=library\n')
        (self.fragments / '30-consumer.env').write_text('PATH_PREPEND=/opt/consumer/bin\nTOOLKIT=consumer\n')
        child = self.launch('grok')
        self.assertEqual(child['path'], '/opt/consumer/bin:/opt/cuda/bin:' + self.env['PATH'])
        self.assertEqual(child['toolkit'], 'consumer')
        self.assertEqual(child['env']['HORIZON_SESSION_DIR'], str(self.workspace / 'session-data/test-panel'))

    def test_refused_session_environment_starts_no_process(self):
        self.fragments.mkdir()
        (self.fragments / '20-library.env').write_text('source /opt/toolkit.sh\n')
        result = subprocess.run(['bash', str(self.script), 'test-panel', 'claude'], env=self.env,
                                capture_output=True, text=True, timeout=10, check=True)
        self.assertFalse((self.root / 'child.json').exists())
        self.assertEqual((self.workspace / 'sessions/test-panel/exit-status').read_text(), '3\n')
        self.assertIn('20-library.env:1: expected KEY=VALUE', result.stderr)
        self.assertIn('no process was started', result.stdout)
        log = (self.workspace / 'session-env.log').read_text()
        self.assertIn('session test-panel: Session environment refused:', log)
        self.assertIn('20-library.env:1', log)

    def test_every_accepted_variable_reaches_the_agent_exactly(self):
        # Names a shell treats specially are passed through, never interpreted by the launcher.
        self.fragments.mkdir()
        (self.fragments / '20-layer.env').write_text('_=layer\nIFS=,\nBASH_ALIASES=alias\nUID=0\nTOOLKIT=a b\n')
        child = self.launch('grok')
        self.assertEqual(child['layer'], {'_': 'layer', 'IFS': ',', 'BASH_ALIASES': 'alias', 'UID': '0'})
        self.assertEqual(child['toolkit'], 'a b')
        self.assertEqual(child['args'], ['--no-leader'])

    def test_every_variable_the_launcher_sets_is_refused_in_a_layer(self):
        import re
        source = Path(__file__).with_name('horizon-worker-run').read_text()
        owned = set(re.findall(r'\bexport ((?:[A-Z_][A-Z0-9_]*(?:=\S*)? ?)+)', source))
        names = {word.split('=', 1)[0] for group in owned for word in group.split()}
        names |= set(re.findall(r'\bunset ([A-Z_][A-Z0-9_]*)', source))
        names |= {'PATH'}
        self.assertTrue({'HOME', 'DISPLAY', 'HORIZON', 'HORIZON_SESSION_DIR', 'ANTHROPIC_API_KEY'} <= names, names)
        self.fragments.mkdir()
        helper = self.tools / 'horizon-worker-session-env'
        for name in sorted(names):
            (self.fragments / '20-layer.env').write_text(name + '=layer\n')
            refused = subprocess.run([str(helper), 'check'], capture_output=True, text=True, timeout=10)
            self.assertEqual(refused.returncode, 1, name)
            self.assertIn(f'{name} cannot be set by an image layer', refused.stderr)

if __name__ == '__main__':
    unittest.main()
