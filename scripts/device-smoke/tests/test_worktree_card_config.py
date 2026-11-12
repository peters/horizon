#!/usr/bin/env python3
"""The device fixture can show the worktree card without a model.

`--worktree-card` makes the throwaway repository, the two presets and the panel of
the scripted card agent, so a procedure watches `open_panel` and the card of the
worktrees with real git and no login. These tests prove the parts of the option
without a display and without Horizon.
"""
from __future__ import annotations

import argparse
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import serve  # noqa: E402


def args_with(**options):
    """The parsed options of a run, with every option of the fixture off."""
    values = {name: False for name, _ in serve.PLAIN_START_SETTINGS_OPTIONS}
    values['theme'] = None
    values['named_secret'] = None
    values['secret_row'] = None
    values.update(options)
    return argparse.Namespace(**values)


class WorktreeCardRepositoryTests(unittest.TestCase):
    def prepare(self, folder):
        state = Path(folder)
        (state / 'data').mkdir()
        return state, serve.worktree_card_repository(state)

    def test_the_repository_holds_one_file_and_one_commit(self):
        with tempfile.TemporaryDirectory() as folder:
            state, repo = self.prepare(folder)
            self.assertEqual(repo, state / 'data' / serve.WORKTREE_CARD_FOLDER)
            self.assertTrue((repo / '.git').is_dir(), 'a real repository')
            self.assertEqual((repo / 'app.txt').read_text(), 'one\n')
            log = subprocess.run(['git', '-C', str(repo), 'log', '--format=%s'],
                                 capture_output=True, text=True, check=True).stdout
            self.assertEqual(log.splitlines(), ['first commit'])
            branch = subprocess.run(['git', '-C', str(repo), 'symbolic-ref', '--short', 'HEAD'],
                                    capture_output=True, text=True, check=True).stdout.strip()
            self.assertEqual(branch, 'main')
            user = subprocess.run(['git', '-C', str(repo), 'config', 'user.email'],
                                  capture_output=True, text=True, check=True).stdout.strip()
            self.assertEqual(user, 'fixture@example.invalid',
                             'a local user name, so a commit needs no configuration')

    def test_a_second_run_keeps_the_repository_of_the_first(self):
        with tempfile.TemporaryDirectory() as folder:
            state, repo = self.prepare(folder)
            self.assertEqual(serve.worktree_card_repository(state), repo)
            log = subprocess.run(['git', '-C', str(repo), 'log', '--format=%s'],
                                 capture_output=True, text=True, check=True).stdout
            self.assertEqual(len(log.splitlines()), 1, 'the second call adds no commit')

    def test_the_card_agent_works_on_that_repository_alone(self):
        with tempfile.TemporaryDirectory() as folder:
            state, repo = self.prepare(folder)
            launcher = Path(serve.worktree_card_panel(Path('/opt/horizon'), repo, state)['command'])
            script = launcher.read_text()
            self.assertIn(f'CARD_AGENT_REPO={repo}', script)
            self.assertIn(f'CARD_AGENT_STATE={state}', script)
            self.assertIn('CARD_AGENT_PRESET=', script)
            # The worktrees of a task live beside the repository, under the same state.
            self.assertTrue(str(state / 'data').startswith(str(state)))


class WorktreeCardPanelTests(unittest.TestCase):
    def prepare(self, folder):
        state = Path(folder)
        (state / 'data').mkdir()
        repo = serve.worktree_card_repository(state)
        return state, repo, serve.worktree_card_panel(Path('/opt/horizon'), repo, state)

    def test_the_panel_is_the_agent_kind_that_gets_a_card_identity(self):
        with tempfile.TemporaryDirectory() as folder:
            _state, _repo, panel = self.prepare(folder)
            self.assertEqual(panel['kind'], 'claude', 'the agent kind whose panel holds an acting identity')
            self.assertEqual(panel['name'], 'Worktree card agent')
            self.assertEqual(panel['args'], [], 'the launcher reads Horizon arguments from "$@"')

    def test_the_launcher_runs_the_card_agent_with_the_scenario(self):
        with tempfile.TemporaryDirectory() as folder:
            _state, _repo, panel = self.prepare(folder)
            launcher = Path(panel['command'])
            self.assertTrue(launcher.is_file())
            self.assertEqual(launcher.stat().st_mode & stat.S_IXUSR, stat.S_IXUSR,
                             'the panel runs the launcher itself')
            script = launcher.read_text()
            self.assertTrue(script.startswith('#!/bin/sh\n'))
            self.assertIn('card_agent.py', script)
            self.assertIn('--scenario worktree', script)
            self.assertIn('--mcp-command', script)
            self.assertIn('--browser-mcp', script)
            self.assertIn('"$@"', script, 'the arguments of Horizon reach the script')
            self.assertEqual(panel['cwd'], str(Path(folder) / 'data'))
            self.assertTrue(Path(panel['cwd']).is_dir())

    def test_the_presets_are_the_task_shell_and_the_browser_page(self):
        kinds = {preset['name']: preset['kind'] for preset in serve.WORKTREE_CARD_PRESETS}
        self.assertEqual(kinds, {'Task shell': 'shell', 'Page': 'browser'})
        shell = serve.WORKTREE_CARD_PRESETS[0]
        self.assertEqual(shell['command'], '/bin/sh')
        self.assertEqual(serve.WORKTREE_CARD_PRESET, 'Task shell')

    def test_the_fixture_config_holds_the_presets_and_the_panel(self):
        with tempfile.TemporaryDirectory() as folder:
            _state, _repo, panel = self.prepare(folder)
            document = serve.application_document(
                '/state/data', presets=serve.WORKTREE_CARD_PRESETS, extra_panels=[panel])
            self.assertEqual(document['presets'], serve.WORKTREE_CARD_PRESETS)
            placed = [entry for entry in document['workspaces'][0]['terminals']
                      if entry.get('name') == panel['name']]
            self.assertEqual(len(placed), 1, 'the board holds the card agent panel once')
            for key in ('kind', 'command', 'args', 'cwd'):
                self.assertEqual(placed[0][key], panel[key], key)
            self.assertIn('position', placed[0], 'the fixture places the panel to the right')


class WorktreeCardOptionTests(unittest.TestCase):
    def test_the_option_is_parsed_and_off_by_default(self):
        parser = serve.arguments()
        plain = parser.parse_args(['--horizon', '/opt/horizon', '--state', '/state'])
        self.assertFalse(plain.worktree_card)
        chosen = parser.parse_args(
            ['--horizon', '/opt/horizon', '--state', '/state', '--worktree-card'])
        self.assertTrue(chosen.worktree_card)

    def test_a_plain_start_refuses_the_option(self):
        self.assertEqual(serve.plain_start_conflicts(args_with(worktree_card=True)),
                         ['--worktree-card'])


if __name__ == '__main__':
    unittest.main()
