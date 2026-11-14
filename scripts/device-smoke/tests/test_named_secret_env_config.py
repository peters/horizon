"""The device fixture reads the value of a named secret from its own environment.

`--named-secret NAME=VALUE` puts the value in an argument, which every process of
the same user reads from `/proc/<pid>/cmdline` on Linux. `--named-secret-env NAME`
takes the value from the environment of the fixture instead, so the value is in
no argument list, and the fixture refuses a variable that is not set or empty.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import serve  # noqa: E402


def rows_of(path):
    """The id and the body of every row of a secrets file."""
    connection = sqlite3.connect(path)
    try:
        return dict(connection.execute('SELECT id, body FROM secrets').fetchall())
    finally:
        connection.close()


class NamedSecretFromEnvRuleTests(unittest.TestCase):
    def test_the_value_comes_from_the_environment_of_the_fixture(self):
        self.assertEqual(
            serve.named_secret_from_env('TS_HOME_CLIENT_SECRET', {'TS_HOME_CLIENT_SECRET': 'a-value'}),
            ('TS_HOME_CLIENT_SECRET', 'a-value'))

    def test_a_variable_that_is_not_set_is_refused_with_its_name(self):
        with self.assertRaises(ValueError) as raised:
            serve.named_secret_from_env('TS_HOME_CLIENT_SECRET', {})

        message = str(raised.exception)
        self.assertIn('TS_HOME_CLIENT_SECRET', message)
        self.assertIn('not set', message)

    def test_a_variable_that_is_empty_is_refused_with_its_name(self):
        with self.assertRaises(ValueError) as raised:
            serve.named_secret_from_env('TS_HOME_CLIENT_SECRET', {'TS_HOME_CLIENT_SECRET': ''})

        message = str(raised.exception)
        self.assertIn('TS_HOME_CLIENT_SECRET', message)
        self.assertIn('empty', message)

    def test_a_name_of_the_rule_of_a_named_secret_is_refused_with_no_value(self):
        for name in ('ts_home_client_secret', '_LEADING', '1LEADING', 'OPENAI_API_KEY', 'PATH'):
            with self.subTest(name=name):
                with self.assertRaises(ValueError) as raised:
                    serve.named_secret_from_env(name, {name: 'a-value'})
                self.assertNotIn('a-value', str(raised.exception), 'a refusal names no value')

    def test_a_value_with_a_line_break_is_refused(self):
        with self.assertRaises(ValueError):
            serve.named_secret_from_env('TS_HOME_CLIENT_SECRET', {'TS_HOME_CLIENT_SECRET': 'two\nlines'})

    def test_the_fixture_reads_the_variable_of_its_own_environment(self):
        with mock.patch.dict(os.environ, {'TS_HOME_CLIENT_SECRET': 'a-value'}, clear=False):
            self.assertEqual(serve.named_secret_from_env('TS_HOME_CLIENT_SECRET', os.environ),
                             ('TS_HOME_CLIENT_SECRET', 'a-value'))


class NamedSecretFromEnvOptionTests(unittest.TestCase):
    def test_the_option_is_repeatable_and_takes_a_name(self):
        parser = serve.arguments()
        args = parser.parse_args([
            '--horizon', '/opt/horizon', '--state', '/tmp/state',
            '--named-secret-env', 'TS_HOME_CLIENT_SECRET',
            '--named-secret-env', 'TS_HOME_CLIENT_ID',
        ])

        self.assertEqual(args.named_secret_env, ['TS_HOME_CLIENT_SECRET', 'TS_HOME_CLIENT_ID'])

    def test_a_plain_start_refuses_the_option(self):
        args = {name: False for name, _ in serve.PLAIN_START_SETTINGS_OPTIONS}
        args['named_secret_env'] = ['TS_HOME_CLIENT_SECRET']

        self.assertEqual(serve.plain_start_conflicts(type('Args', (), args)), ['--named-secret-env'])

    def test_the_value_reaches_the_secrets_file_and_the_config_names_it(self):
        with mock.patch.dict(os.environ, {'TS_HOME_CLIENT_SECRET': 'a-value'}, clear=False):
            secret = serve.named_secret_from_env('TS_HOME_CLIENT_SECRET', os.environ)
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / 'data' / serve.SECRETS_FILE_NAME
            ids = serve.seed_secret_rows(path, [secret])

            self.assertEqual(rows_of(path), {'named/TS_HOME_CLIENT_SECRET': b'a-value'})
            document = serve.application_document(
                '/tmp/fixture-data', secret_names=serve.named_secret_names(ids))
            self.assertEqual(document['secrets'], {'names': ['TS_HOME_CLIENT_SECRET']})
            self.assertNotIn('a-value', json.dumps(document), 'a setting holds no value')


if __name__ == '__main__':
    unittest.main()
