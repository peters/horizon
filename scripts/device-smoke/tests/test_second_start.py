"""A second start of the candidate inside the sandbox of a fixture that runs."""
from __future__ import annotations

import contextlib
import io

import subprocess
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import second_start  # noqa: E402

NAMESPACE = ['bwrap', '--ro-bind', '/', '/', '--chdir', '/fixture/data', '--']
FIXTURE_ENVIRONMENT = {
    'DISPLAY': ':97',
    'HOME': '/home/someone',
    'XDG_RUNTIME_DIR': '/fixture/data/runtime',
    'AWS_SECRET_ACCESS_KEY': 'kept-in-the-shell',
}
BASE = {'HOME': '/home/someone', 'AWS_SECRET_ACCESS_KEY': 'kept-in-the-shell'}


def written_state(parent):
    state = Path(parent) / 'run'
    state.mkdir()
    values = second_start.document(NAMESPACE, FIXTURE_ENVIRONMENT, BASE, '/fixture/horizon', 4242)
    second_start.write(state, values)
    return state, values


class Document(unittest.TestCase):
    def test_the_document_keeps_the_values_of_the_fixture_and_not_the_shell(self):
        values = second_start.document(NAMESPACE, FIXTURE_ENVIRONMENT, BASE, '/fixture/horizon', 4242)

        self.assertEqual(values['app'], '/fixture/horizon')
        self.assertEqual(values['namespace'], NAMESPACE)
        self.assertEqual(values['pid'], 4242)
        self.assertEqual(values['environment'], {
            'DISPLAY': ':97',
            'XDG_RUNTIME_DIR': '/fixture/data/runtime',
        })

    def test_the_file_of_a_second_start_is_private_and_goes_away_with_the_run(self):
        with tempfile.TemporaryDirectory() as parent:
            state, _ = written_state(parent)
            path = second_start.document_path(state)

            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            second_start.remove(state)
            self.assertFalse(path.exists())
            second_start.remove(state)


class Refusal(unittest.TestCase):
    def test_a_state_path_of_no_fixture_refuses(self):
        with tempfile.TemporaryDirectory() as parent:
            state = Path(parent) / 'run'
            state.mkdir()

            with self.assertRaises(SystemExit) as refused:
                second_start.values_of(state)
            self.assertIn('No fixture runs at', str(refused.exception.code))
            self.assertIn(second_start.DOCUMENT_NAME, str(refused.exception.code))

    def test_a_fixture_that_ended_refuses(self):
        with tempfile.TemporaryDirectory() as parent:
            state, _ = written_state(parent)

            with self.assertRaises(SystemExit) as refused:
                second_start.values_of(state, alive=lambda pid: False)
            self.assertIn('ended', str(refused.exception.code))
            self.assertIn('4242', str(refused.exception.code))

    def test_a_file_of_another_shape_refuses(self):
        with tempfile.TemporaryDirectory() as parent:
            state = Path(parent) / 'run'
            state.mkdir()
            second_start.document_path(state).write_text('not json')

            with self.assertRaises(SystemExit) as refused:
                second_start.values_of(state)
            self.assertIn('not readable', str(refused.exception.code))


class Start(unittest.TestCase):
    def test_the_start_enters_the_sandbox_of_the_fixture(self):
        with tempfile.TemporaryDirectory() as parent:
            state, values = written_state(parent)
            seen = {}

            def run(argv, env, timeout):
                seen.update(argv=argv, env=env, timeout=timeout)
                return subprocess.CompletedProcess(argv, 0)

            with contextlib.redirect_stdout(io.StringIO()) as printed:
                status = second_start.start(state, ['/fixture/folder'], run=run, alive=lambda pid: True, base={})

            self.assertEqual(status, 0)
            self.assertEqual(seen['argv'], [*NAMESPACE, '/fixture/horizon', '/fixture/folder'])
            self.assertEqual(seen['env']['DISPLAY'], ':97')
            self.assertEqual(seen['env']['XDG_RUNTIME_DIR'], values['environment']['XDG_RUNTIME_DIR'])
            self.assertIn('status 0', printed.getvalue())

    def test_the_environment_of_the_calling_shell_keeps_no_horizon_value(self):
        values = second_start.document(NAMESPACE, FIXTURE_ENVIRONMENT, BASE, '/fixture/horizon', 4242)

        environment = second_start.environment(values, base={'HORIZON_WORK_ID': '1', 'DISPLAY': ':0'})

        self.assertNotIn('HORIZON_WORK_ID', environment)
        self.assertEqual(environment['DISPLAY'], ':97')

    def test_the_status_of_the_candidate_is_the_status_of_the_second_start(self):
        with tempfile.TemporaryDirectory() as parent:
            state, _ = written_state(parent)

            def run(argv, env, timeout):
                return subprocess.CompletedProcess(argv, 2)

            with contextlib.redirect_stdout(io.StringIO()) as printed:
                status = second_start.start(state, [], run=run, alive=lambda pid: True, base={})

            self.assertEqual(status, 2)
            self.assertIn('status 2', printed.getvalue())


class Arguments(unittest.TestCase):
    def test_the_arguments_after_the_separator_belong_to_the_candidate(self):
        parsed = second_start.arguments(['--state', '/fixture/run', '--', '--new-session'])

        self.assertEqual(parsed.state, Path('/fixture/run'))
        self.assertEqual([item for item in parsed.start if item != '--'], ['--new-session'])

    def test_the_state_folder_is_needed(self):
        with self.assertRaises(SystemExit):
            second_start.arguments([])
