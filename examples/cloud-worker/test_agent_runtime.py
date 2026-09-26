"""Exercise the agent launcher with synthetic child executables and credentials."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class AgentRuntimeTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.workspace = self.root / 'workspace'
        (self.workspace / 'home/.horizon').mkdir(parents=True)
        (self.workspace / 'home/.horizon/cloud-host-instance').write_text('test-host\n')
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
        self.script.write_text(source.replace('/workspace', str(self.workspace)))
        self.env = {'PATH': str(tools) + os.pathsep + os.defpath,
                    'CHILD_RECEIPT': str(self.root / 'child.json'), 'HORIZON': 'parent-host'}

    def launch(self, agent='claude', **environment):
        result = subprocess.run(['bash', str(self.script), 'test-panel', agent],
                                env=dict(self.env, **environment), capture_output=True,
                                text=True, timeout=10, check=True)
        self.assertEqual((self.workspace / 'sessions/test-panel/exit-status').read_text(), '23\n')
        self.assertIn('Session process exited with status 23', result.stdout)
        return json.loads((self.root / 'child.json').read_text())

    def test_minimal_launch_disables_background_updates_and_preserves_contract(self):
        child = self.launch()
        self.assertEqual(child['args'], ['--mcp-config', str(self.workspace / 'agent-mcp.json')])
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
