"""The configure step tells agents where the one checkout is and that extra worktrees are theirs."""
import json
import os
from pathlib import Path
import runpy
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).parent


class AgentGuidanceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.real_path = Path

    def path(self, value):
        if isinstance(value, str) and value.startswith('/workspace/'):
            return self.root / value.lstrip('/')
        return self.real_path(value)

    def configure(self, agents):
        capabilities = self.path('/workspace/capabilities.json')
        capabilities.parent.mkdir(parents=True, exist_ok=True)
        capabilities.write_text(json.dumps({'agents': agents, 'browsers': [], 'desktop': False}))

        def run(command, **kwargs):
            if command[0] == 'horizon-cloud-worker':
                payload = json.loads(kwargs['input'])
                return subprocess.CompletedProcess(command, 0, stdout=payload['original'] + payload['managed'])
            return subprocess.CompletedProcess(command, 0)
        with mock.patch('pathlib.Path', side_effect=self.path), mock.patch('subprocess.run', side_effect=run), \
                mock.patch.dict(os.environ, {}, clear=True):
            runpy.run_path(str(ROOT / 'horizon-worker-configure'), run_name='__main__')

    def guide(self, agent, name):
        path = self.root / 'workspace/home' / ('.' + agent) / name
        return path.read_text() if path.exists() else None

    def test_each_allowed_agent_learns_where_the_checkout_is_and_how_to_add_worktrees(self):
        self.configure(['claude', 'codex'])
        for agent, name in (('claude', 'CLAUDE.md'), ('codex', 'AGENTS.md')):
            text = self.guide(agent, name)
            self.assertIn('/workspace/checkout', text)
            self.assertIn('git -C /workspace/checkout worktree add /workspace/worktrees/<name> -b <branch>', text)
            self.assertIn('do not clone it again', text)

    def test_an_agent_the_profile_does_not_allow_gets_no_guide(self):
        self.configure(['claude'])
        self.assertIsNone(self.guide('codex', 'AGENTS.md'))
        self.assertIsNotNone(self.guide('claude', 'CLAUDE.md'))

    def test_reconfiguring_replaces_the_block_and_keeps_the_agents_own_notes(self):
        notes = self.root / 'workspace/home/.claude/CLAUDE.md'
        notes.parent.mkdir(parents=True)
        notes.write_text('# My notes\n\nPrefer small commits.\n')
        self.configure(['claude'])
        first = notes.read_text()
        self.assertTrue(first.startswith('# My notes\n\nPrefer small commits.\n\n<!-- horizon-cloud-worker:begin -->'))
        self.configure(['claude'])
        self.assertEqual(notes.read_text(), first, 'repeating changes nothing')
        notes.write_text(first.replace('worktrees are', 'worktrees were').replace('do not clone it again', 'stale text'))
        self.configure(['claude'])
        self.assertEqual(notes.read_text().count('horizon-cloud-worker:begin'), 1)
        self.assertNotIn('stale text', notes.read_text())
        self.assertIn('Prefer small commits.', notes.read_text())


if __name__ == '__main__':
    unittest.main()
