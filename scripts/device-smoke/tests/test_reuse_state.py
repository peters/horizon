"""Private state that a fixture run leaves for the next run (#1905)."""
from __future__ import annotations

import os
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import sandbox  # noqa: E402


class FreshState(unittest.TestCase):
    def test_a_state_path_must_be_new(self):
        with tempfile.TemporaryDirectory() as parent:
            state = Path(parent) / 'run'
            self.assertEqual(sandbox.prepare_state(state, reuse=False), state)
            self.assertTrue(state.is_dir())
            with self.assertRaises(FileExistsError):
                sandbox.prepare_state(state, reuse=False)

    def test_prepare_refuses_a_private_home_that_is_already_there(self):
        with tempfile.TemporaryDirectory() as parent:
            state = sandbox.prepare_state(Path(parent) / 'run', reuse=False)
            sandbox.prepare(state)
            with self.assertRaises(FileExistsError):
                sandbox.prepare(state)


class ReusedState(unittest.TestCase):
    def test_a_reused_state_keeps_the_private_home_of_the_earlier_run(self):
        with tempfile.TemporaryDirectory() as parent:
            state = sandbox.prepare_state(Path(parent) / 'run', reuse=True)
            sandbox.prepare(state, reuse=True)
            kept = state / 'data' / 'home' / '.horizon' / 'cloud' / 'drafts.json'
            kept.parent.mkdir(parents=True)
            kept.write_text('{"kept": true}')

            self.assertEqual(sandbox.prepare_state(state, reuse=True), state)
            box = sandbox.prepare(state, reuse=True)

            self.assertEqual(kept.read_text(), '{"kept": true}')
            for directory in (box.data, box.private_home, state / 'data' / 'runtime', state / 'data' / 'tmp'):
                self.assertTrue(directory.is_dir(), directory)
                self.assertEqual(directory.stat().st_mode & 0o777, 0o700, directory)

    def test_a_reused_state_masks_the_same_private_home(self):
        with tempfile.TemporaryDirectory() as parent:
            state = sandbox.prepare_state(Path(parent) / 'run', reuse=True)
            first = sandbox.prepare(state, reuse=True)
            second = sandbox.prepare(state, reuse=True)
            home_binds = [
                first.namespace[index + 1]
                for index, item in enumerate(first.namespace)
                if item == '--bind' and first.namespace[index + 2] == str(Path.home())
            ]
            self.assertEqual(len(home_binds), 1)
            self.assertEqual(home_binds[0], str(second.private_home))
            self.assertEqual(Path(home_binds[0]).name, 'home')
            self.assertTrue(os.path.isdir(home_binds[0]))
