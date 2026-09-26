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
        'ANTHROPIC_WORKSPACE_ID', 'ANTHROPIC_CUSTOM_HEADERS']
Path(os.environ['CHILD_RECEIPT']).write_text(json.dumps({
    'args': sys.argv[1:], 'env': {key: os.environ.get(key) for key in keys}}))
sys.exit(23)
'''
        for name, body in [('claude', child), ('grok', child), ('sleep', 'import sys\nsys.exit(0)\n')]:
            tool = tools / name
            tool.write_text('#!' + sys.executable + '\n' + body)
            tool.chmod(0o700)
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
            'ANTHROPIC_CUSTOM_HEADERS': 'anthropic-workspace-id: synthetic-workspace'})

    def test_inherited_environment_cannot_enable_background_updates(self):
        self.assertEqual(self.launch(DISABLE_AUTOUPDATER='0')['env']['DISABLE_AUTOUPDATER'], '1')

    def test_other_agent_keeps_its_launch_environment_and_arguments(self):
        child = self.launch('grok')
        self.assertIsNone(child['env']['DISABLE_AUTOUPDATER'])
        self.assertIsNone(child['env']['ANTHROPIC_API_KEY'])
        self.assertEqual(child['args'], ['--no-leader'])


if __name__ == '__main__':
    unittest.main()
