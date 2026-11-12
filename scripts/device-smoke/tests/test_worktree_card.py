#!/usr/bin/env python3
"""Self test for the `worktree` scenario of card_agent.py, with real git.

The repository runs it with the other device smoke tests:

    python3 -B -m unittest discover -s scripts/device-smoke/tests -v

Every test makes its own repository under TMPDIR and never touches a repository
of this computer. The block_panel answers come from a fake source in this file,
so no Horizon and no agent program runs. The tests prove:

  * the slug rule of the worktree skill;
  * the rows of a repository with three worktrees: the clean main worktree, a
    worktree with a file that is not committed, and a worktree that is ahead;
  * the refusal of a repository that is not under the state folder;
  * the start of the scenario: one task worktree, then `open_panel` with the
    preset, the folder and the title;
  * the second press of merge and of remove, and the second button that goes
    away at the next other click;
  * the conflict that the card aborts and reports;
  * the refusal of the main worktree;
  * the three refusals of `open_panel` that the panel prints and the card never
    draws.

Standard library only. Nothing is written outside the folder of this test.
"""
from __future__ import annotations

import sys

# Importing card_agent.py for its git helpers must not leave a __pycache__.
sys.dont_write_bytecode = True

from pathlib import Path
import contextlib
import io
import os
import subprocess
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import card_agent  # noqa: E402

REPO_ENV = card_agent.WORKTREE_REPO_ENV
STATE_ENV = card_agent.WORKTREE_STATE_ENV
PRESET_ENV = card_agent.WORKTREE_PRESET_ENV


def git(repo, *arguments: str) -> str:
    """One git command of a test, in one repository, with an argument list."""
    result = subprocess.run(['git', '-C', str(repo), *arguments],
                            capture_output=True, text=True, encoding='utf-8')
    if result.returncode != 0:
        raise AssertionError(f'git {arguments} in {repo} failed: {result.stderr.strip()}')
    return result.stdout


class FixtureRepository:
    """A throwaway repository, and the worktrees that a test asks it to make."""

    def __init__(self, root: Path):
        self.state = root / 'state'
        self.state.mkdir()
        self.repo = self.state / 'data' / 'repo'
        self.repo.mkdir(parents=True)
        git(self.repo, 'init', '-b', 'main')
        git(self.repo, 'config', 'user.name', 'Fixture Test')
        git(self.repo, 'config', 'user.email', 'fixture@example.invalid')
        (self.repo / 'app.txt').write_text('one\n')
        git(self.repo, 'add', 'app.txt')
        git(self.repo, 'commit', '-m', 'first commit')

    def worktree(self, slug: str, commits: int = 0, dirty: bool = False) -> Path:
        """One task worktree beside the repository, with commits and a dirty file."""
        folder = self.state / 'data' / 'repo-wt' / slug
        git(self.repo, 'worktree', 'add', '-b', f'task/{slug}', str(folder), 'main')
        for index in range(commits):
            (folder / 'app.txt').write_text(f'{slug} {index}\n')
            git(folder, 'add', 'app.txt')
            git(folder, 'commit', '-m', f'change {index} of {slug}')
        if dirty:
            (folder / 'dirty.txt').write_text('not committed\n')
        return folder

    def environment(self) -> dict:
        return {REPO_ENV: str(self.repo), STATE_ENV: str(self.state),
                PRESET_ENV: card_agent.DEFAULT_WORKTREE_PRESET}


class FakeHost:
    """The block_panel answers of a test, and the requests it recorded."""

    def __init__(self, answers=()):
        self.answers = list(answers)
        self.calls: list[dict] = []

    def call_tool(self, name, arguments, timeout=None):
        if name != 'block_panel':
            raise AssertionError(f'the card agent asked for the tool {name}')
        self.calls.append(dict(arguments))
        if self.answers:
            return self.answers.pop(0)
        return {'status': 'updated', 'panel_id': arguments.get('panel_id') or 'p1', 'revision': 2}

    def operations(self) -> list[str]:
        return [call.get('operation') for call in self.calls]


def agent_for(host: FakeHost) -> card_agent.CardAgent:
    return card_agent.CardAgent(host, 'continuous', 60.0)


def buttons_of(card, rich=True) -> list[dict]:
    document = card.document(rich)
    for component in document['components']:
        if component['type'] == 'buttons':
            return component['items']
    raise AssertionError('the document holds no buttons')


def button_ids(card, rich=True) -> list[str]:
    return [button['id'] for button in buttons_of(card, rich)]


def rows_of(card, rich=True) -> list[dict]:
    document = card.document(rich)
    for component in document['components']:
        if component['type'] == 'list':
            return component['rows']
    raise AssertionError('the document holds no list')


def texts_of(card, rich=True) -> str:
    document = card.document(rich)
    return '\n'.join(component['text'] for component in document['components']
                     if component['type'] == 'text')


class WorktreeCardTest(unittest.TestCase):
    def prepare(self):
        """A throwaway repository with a dirty worktree and a worktree that is ahead."""
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        fixture = FixtureRepository(Path(folder.name))
        fixture.worktree('dirty', dirty=True)
        fixture.worktree('ahead', commits=2)
        worktrees = card_agent.read_worktrees(fixture.repo, 'main')
        ahead = [worktree for worktree in worktrees if worktree.name == 'ahead'][0]
        host = FakeHost()
        card = card_agent.WorktreeCard(
            fixture.repo, fixture.state, card_agent.DEFAULT_WORKTREE_PRESET, 'main',
            worktrees, ahead)
        card.panel_id = 'p1'
        return fixture, host, card

    # -- the slug and the start -------------------------------------------

    def test_the_slug_of_a_task_follows_the_rules_of_the_skill(self):
        self.assertEqual(card_agent.slug_of('Fix login redirect!'), 'fix-login-redirect')
        self.assertEqual(card_agent.slug_of('  Two   spaces  '), 'two-spaces')
        self.assertEqual(card_agent.slug_of('x' * 60), 'x' * 40)
        self.assertEqual(card_agent.slug_of('...'), 'task')
        for task in ('A task', 'task/\u00e5', 'x' * 100, ''):
            slug = card_agent.slug_of(task)
            self.assertLessEqual(len(slug), card_agent.MAX_SLUG_CHARS, slug)
            self.assertTrue(all(character.isalnum() or character == '-' for character in slug),
                            slug)

    def test_the_start_makes_one_worktree_and_opens_its_panel(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        fixture = FixtureRepository(Path(folder.name))
        host = FakeHost([{'status': 'opened', 'panel_id': 's1'}])
        printed = io.StringIO()
        with mock.patch.dict(os.environ, fixture.environment()), contextlib.redirect_stdout(printed):
            card = agent_for(host).prepare_worktree()

        task = card.task
        self.assertTrue(task.path.is_dir(), 'the task folder is there')
        self.assertEqual(task.path.parent, fixture.state / 'data' / 'repo-wt')
        self.assertEqual(task.branch, 'task/first-change')
        self.assertIn(str(task.path), git(fixture.repo, 'worktree', 'list', '--porcelain'))
        request = host.calls[0]
        self.assertEqual(request['operation'], 'open_panel')
        self.assertEqual(request['preset'], card_agent.DEFAULT_WORKTREE_PRESET)
        self.assertEqual(request['directory'], str(task.path))
        self.assertEqual(request['title'], card_agent.WORKTREE_TASK)
        self.assertIn('opened', printed.getvalue())
        self.assertIn('panel s1', printed.getvalue())
        # The card shows the worktrees that the start left behind.
        self.assertEqual(len(card.worktrees), 2)

    def test_a_repository_outside_the_state_folder_is_refused(self):
        outside = tempfile.TemporaryDirectory()
        self.addCleanup(outside.cleanup)
        elsewhere = FixtureRepository(Path(outside.name))
        inside = tempfile.TemporaryDirectory()
        self.addCleanup(inside.cleanup)
        state = Path(inside.name)
        (state / 'data').mkdir()
        environment = {REPO_ENV: str(elsewhere.repo), STATE_ENV: str(state)}
        with mock.patch.dict(os.environ, environment), self.assertRaises(card_agent.Fatal):
            card_agent.worktree_environment()

        # A folder inside the repository is not the root of the repository.
        environment = {REPO_ENV: str(elsewhere.repo / 'sub'), STATE_ENV: str(elsewhere.state)}
        (elsewhere.repo / 'sub').mkdir()
        with mock.patch.dict(os.environ, environment), self.assertRaises(card_agent.Fatal):
            card_agent.worktree_environment()

        environment = elsewhere.environment()
        with mock.patch.dict(os.environ, environment):
            repo, state, preset = card_agent.worktree_environment()
        self.assertEqual(repo, elsewhere.repo.resolve())
        self.assertEqual(state, elsewhere.state.resolve())
        self.assertEqual(preset, card_agent.DEFAULT_WORKTREE_PRESET)
        self.assertFalse(card_agent.is_under(elsewhere.state, elsewhere.state.parent))

    # -- the rows of the card ---------------------------------------------

    def test_the_rows_of_three_worktrees_show_the_state_of_each_one(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        fixture = FixtureRepository(Path(folder.name))
        fixture.worktree('dirty', dirty=True)
        fixture.worktree('ahead', commits=2)

        worktrees = card_agent.read_worktrees(fixture.repo, 'main')
        # `git worktree list` answers the main worktree first, then by path.
        self.assertEqual([worktree.name for worktree in worktrees], ['repo', 'ahead', 'dirty'])
        self.assertTrue(worktrees[0].main, 'the first worktree of the list is the main one')
        self.assertEqual((worktrees[0].ahead, worktrees[0].files_changed), (0, 0))
        self.assertEqual((worktrees[1].ahead, worktrees[1].files_changed), (2, 0))
        self.assertEqual((worktrees[2].ahead, worktrees[2].files_changed), (0, 1))
        self.assertIn('main worktree', worktrees[0].subtitle())
        self.assertIn('1 file changed', worktrees[2].subtitle())
        self.assertIn('ahead 2', worktrees[1].subtitle())
        self.assertIn('last commit', worktrees[1].subtitle())

        card = card_agent.WorktreeCard(fixture.repo, fixture.state, 'Task shell', 'main',
                                       worktrees, worktrees[1])
        card.panel_id = 'p1'
        rows = rows_of(card)
        self.assertEqual([row['title'] for row in rows], ['repo', 'ahead', 'dirty'])
        self.assertEqual([row['id'] for row in rows],
                         ['worktree.repo', 'worktree.ahead', 'worktree.dirty'])
        self.assertEqual([row.get('selected') for row in rows], [None, True, None])
        # The text under the list holds the stat and the last subjects of the row.
        text = texts_of(card)
        self.assertIn('app.txt', text)
        self.assertIn('change 1 of ahead', text)
        self.assertIn('change 0 of ahead', text)
        self.assertLessEqual(len(text), card_agent.WORKTREE_TEXT_CHARS)

    def test_show_changes_lists_the_changed_files_and_their_diff(self):
        _fixture, host, card = self.prepare()
        agent = agent_for(host)
        agent.react_worktree(card, 'changes')

        self.assertEqual(card.view, 'changes')
        self.assertEqual(rows_of(card)[0]['title'], 'app.txt')
        self.assertIn('app.txt', texts_of(card))
        self.assertIn('refresh', button_ids(card))
        self.assertIn('worktrees', button_ids(card))

        # The row of the file selects it, and the button returns to the worktrees.
        agent.react_worktree(card, 'file.0')
        self.assertEqual(card.file, 'app.txt')
        agent.react_worktree(card, 'worktrees')
        self.assertEqual(card.view, 'worktrees')
        self.assertEqual(card.files, [])

    # -- merge ------------------------------------------------------------

    def test_the_second_press_of_merge_is_required(self):
        fixture, host, card = self.prepare()
        agent = agent_for(host)
        ahead = card.selected_worktree()
        self.assertEqual(ahead.name, 'ahead')
        before = git(fixture.repo, 'log', '--format=%s', '-1', 'main')

        agent.react_worktree(card, 'merge')
        self.assertEqual(card.pending, 'merge')
        self.assertIn('merge.confirm', button_ids(card))
        confirm = [button for button in buttons_of(card) if button['id'] == 'merge.confirm'][0]
        self.assertEqual(confirm['label'], 'Merge task/ahead into main')
        self.assertEqual(git(fixture.repo, 'log', '--format=%s', '-1', 'main'), before,
                         'no merge runs before the second press')

        # Another click drops the second button that the person did not press.
        agent.react_worktree(card, 'refresh')
        self.assertIsNone(card.pending)
        self.assertNotIn('merge.confirm', button_ids(card))
        self.assertEqual(git(fixture.repo, 'log', '--format=%s', '-1', 'main'), before)

        agent.react_worktree(card, 'merge')
        agent.react_worktree(card, 'merge.confirm')
        self.assertIsNone(card.pending)
        subjects = git(fixture.repo, 'log', '--format=%s', '-5', 'main')
        self.assertIn('change 1 of ahead', subjects, 'the commits of the branch are in the base')
        self.assertIn('change 0 of ahead', subjects)
        self.assertIn('Merge branch', subjects, 'the merge keeps its own commit')
        self.assertIn('Merged task/ahead into main', card.note)
        self.assertIn('Nothing was pushed', card.note)
        self.assertEqual(card.selected_worktree().ahead, 0, 'the branch is in the base now')

    def test_a_dirty_worktree_and_a_branch_that_is_not_ahead_are_refused(self):
        _fixture, host, card = self.prepare()
        agent = agent_for(host)
        agent.react_worktree(card, 'worktree.dirty')
        agent.react_worktree(card, 'merge')
        self.assertIsNone(card.pending)
        self.assertNotIn('merge.confirm', button_ids(card))
        self.assertIn('not committed', card.note)

        agent.react_worktree(card, 'worktree.repo')
        agent.react_worktree(card, 'merge')
        self.assertIn('main worktree', card.note)

    def test_a_clean_branch_that_is_not_ahead_is_refused(self):
        _fixture, host, card = self.prepare()
        agent = agent_for(host)
        agent.react_worktree(card, 'worktree.dirty')
        # The file goes away, so the branch holds no change of its own.
        card.selected_worktree().files_changed = 0
        agent.react_worktree(card, 'merge')
        self.assertIsNone(card.pending)
        self.assertNotIn('merge.confirm', button_ids(card))
        self.assertIn('is not ahead of main', card.note)

    def test_the_scenario_never_pushes(self):
        source = Path(card_agent.__file__).read_text()
        self.assertNotIn("'push'", source)
        self.assertNotIn('"push"', source)

    def test_a_conflict_is_aborted_and_reported(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        fixture = FixtureRepository(Path(folder.name))
        task = fixture.worktree('conflict')
        (task / 'app.txt').write_text('the branch\n')
        git(task, 'add', 'app.txt')
        git(task, 'commit', '-m', 'the branch change')
        (fixture.repo / 'app.txt').write_text('the base\n')
        git(fixture.repo, 'add', 'app.txt')
        git(fixture.repo, 'commit', '-m', 'the base change')

        worktrees = card_agent.read_worktrees(fixture.repo, 'main')
        conflict = [worktree for worktree in worktrees if worktree.name == 'conflict'][0]
        host = FakeHost()
        card = card_agent.WorktreeCard(fixture.repo, fixture.state, 'Task shell', 'main',
                                       worktrees, conflict)
        card.panel_id = 'p1'
        agent = agent_for(host)
        agent.react_worktree(card, 'merge')
        agent.react_worktree(card, 'merge.confirm')

        self.assertIn('conflicted in app.txt', card.note)
        self.assertIn('aborted', card.note)
        self.assertEqual(git(fixture.repo, 'status', '--porcelain'), '',
                         'the base worktree holds no merge in progress')
        self.assertFalse((fixture.repo / '.git' / 'MERGE_HEAD').exists())
        self.assertEqual(git(fixture.repo, 'log', '--format=%s', '-1', 'main').strip(),
                         'the base change')

    # -- remove -----------------------------------------------------------

    def test_the_second_press_of_remove_is_required(self):
        fixture, host, card = self.prepare()
        agent = agent_for(host)
        agent.react_worktree(card, 'worktree.dirty')
        dirty = card.selected_worktree()

        agent.react_worktree(card, 'remove')
        self.assertEqual(card.pending, 'remove')
        self.assertIn('remove.confirm', button_ids(card))
        self.assertIn('dirty.txt', card.details)
        self.assertTrue(dirty.path.is_dir(), 'no remove runs before the second press')

        agent.react_worktree(card, 'changes')
        self.assertIsNone(card.pending)
        self.assertNotIn('remove.confirm', button_ids(card))
        self.assertTrue(dirty.path.is_dir())

        agent.react_worktree(card, 'remove')
        agent.react_worktree(card, 'remove.confirm')
        self.assertFalse(dirty.path.exists(), 'the second press removes the folder')
        self.assertIn('Removed dirty', card.note)
        self.assertIn('merged, so it is deleted', card.note)
        # The agent opened the panel of the task for the person and does not close
        # it, so the card says where the panel stands after the remove.
        self.assertIn(f'The panel of {dirty.name} is still open; its folder is removed. '
                      'Close it yourself.', card.note)
        self.assertEqual(git(fixture.repo, 'branch', '--list', 'task/dirty').strip(), '',
                         'a branch that is merged goes away with its worktree')

    def test_a_branch_that_is_not_merged_stays(self):
        fixture, host, card = self.prepare()
        agent = agent_for(host)
        folder = fixture.worktree('unmerged', commits=1, dirty=True)
        agent.react_worktree(card, 'refresh')
        agent.react_worktree(card, 'worktree.unmerged')
        self.assertEqual(card.selected_worktree().path, folder)
        agent.react_worktree(card, 'merge')
        self.assertIn('not committed', card.note)
        self.assertIsNone(card.pending, 'a dirty worktree is refused, and nothing waits')

        agent.react_worktree(card, 'remove')
        agent.react_worktree(card, 'remove.confirm')
        self.assertFalse(folder.exists())
        self.assertIn('not merged, so it stays', card.note)
        self.assertTrue(git(fixture.repo, 'rev-parse', '--verify', 'refs/heads/task/unmerged').strip(),
                        'a branch that is not merged stays')

    def test_a_merged_branch_is_deleted_with_its_worktree(self):
        fixture, host, card = self.prepare()
        agent = agent_for(host)
        self.assertEqual(card.selected_worktree().name, 'ahead')
        agent.react_worktree(card, 'merge')
        agent.react_worktree(card, 'merge.confirm')
        agent.react_worktree(card, 'remove')
        agent.react_worktree(card, 'remove.confirm')

        self.assertEqual(git(fixture.repo, 'branch', '--list', 'task/ahead').strip(), '',
                         'the merged branch is deleted')
        self.assertFalse((fixture.state / 'data' / 'repo-wt' / 'ahead').exists())

    def test_the_main_worktree_is_refused(self):
        fixture, host, card = self.prepare()
        agent = agent_for(host)
        agent.react_worktree(card, 'worktree.repo')
        main = card.selected_worktree()
        self.assertTrue(main.main)

        agent.react_worktree(card, 'merge')
        self.assertIsNone(card.pending)
        self.assertNotIn('merge.confirm', button_ids(card))
        self.assertIn('main worktree', card.note)

        agent.react_worktree(card, 'remove')
        self.assertIsNone(card.pending)
        self.assertNotIn('remove.confirm', button_ids(card))
        self.assertIn('main worktree', card.note)
        self.assertTrue(main.path.is_dir())

    def test_a_folder_outside_the_state_folder_is_never_removed(self):
        fixture, host, card = self.prepare()
        agent = agent_for(host)
        agent.react_worktree(card, 'worktree.dirty')
        outside = tempfile.TemporaryDirectory()
        self.addCleanup(outside.cleanup)
        moved = card.selected_worktree()
        moved.path = Path(outside.name) / 'elsewhere'
        card.selected = moved.row_id

        agent.react_worktree(card, 'remove')
        self.assertIn('is not under', card.note)
        self.assertNotIn('remove.confirm', button_ids(card))
        self.assertTrue(Path(outside.name).is_dir())

    # -- the refusals that the panel prints -------------------------------

    def test_the_three_open_panel_requests_are_refused(self):
        _fixture, host, card = self.prepare()
        host.calls.clear()
        host.answers = [
            {'status': 'failed', 'code': 'unknown_preset',
             'message': 'Task shell (ts), Page'},
            {'status': 'failed', 'code': 'directory_missing',
             'message': 'the directory does not exist'},
            {'status': 'failed', 'code': 'invalid_request',
             'message': 'a browser preset opens no program of this computer'},
        ]
        printed = io.StringIO()
        with contextlib.redirect_stdout(printed):
            agent_for(host).refuse_open_panel_requests(card)

        self.assertEqual(host.operations(), ['open_panel', 'open_panel', 'open_panel'])
        self.assertEqual(host.calls[0]['preset'], 'No such preset')
        self.assertEqual(host.calls[0]['directory'], str(card.task.path))
        self.assertEqual(host.calls[1]['preset'], card_agent.DEFAULT_WORKTREE_PRESET)
        self.assertEqual(host.calls[1]['directory'], str(card.state / 'gone'))
        self.assertEqual(host.calls[2]['preset'], card_agent.WORKTREE_BROWSER_PRESET)
        lines = printed.getvalue()
        self.assertIn('unknown_preset', lines)
        self.assertIn('Task shell (ts), Page', lines, 'the names of the presets are printed')
        self.assertIn('directory_missing', lines)
        self.assertIn('invalid_request', lines)
        self.assertEqual(len([line for line in lines.splitlines() if 'open_panel' in line]), 3)
        # The card itself never carries a refusal of the start.
        self.assertNotIn('unknown_preset', texts_of(card))

    def test_the_document_fits_the_card_model(self):
        _fixture, _host, card = self.prepare()
        document = card.document(True)
        self.assertLessEqual(len(document['components']), 64)
        buttons = buttons_of(card)
        self.assertLessEqual(len(buttons), 8)
        self.assertEqual([button['id'] for button in buttons],
                         ['refresh', 'changes', 'merge', 'remove'])
        self.assertEqual([button.get('tone') for button in buttons],
                         [None, None, 'primary', 'danger'])
        for component in document['components']:
            if component['type'] == 'text':
                self.assertLessEqual(len(component['text']), card_agent.WORKTREE_TEXT_CHARS)


if __name__ == '__main__':
    unittest.main()
