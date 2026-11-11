"""The device fixture seeds secret values into the secrets file of its private home."""
from __future__ import annotations

import json
import os
from pathlib import Path
import sqlite3
import stat
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import serve  # noqa: E402


class NamedSecretRuleTests(unittest.TestCase):
    def test_a_name_and_a_value_are_read_from_one_argument(self):
        self.assertEqual(serve.parse_named_secret('HETZNER_TOKEN=a-value'), ('HETZNER_TOKEN', 'a-value'))
        self.assertEqual(serve.parse_named_secret('A=' + 'k' * 40), ('A', 'k' * 40))
        self.assertEqual(serve.parse_named_secret('KEY_WITH_2=value=with=equals'),
                         ('KEY_WITH_2', 'value=with=equals'))

    def test_a_name_that_is_not_a_variable_name_is_refused(self):
        for text in ('lower=value', '_LEADING=value', '1LEADING=value', 'WITH-DASH=value',
                     'WITH SPACE=value', 'A' * 65 + '=value'):
            with self.subTest(text=text):
                with self.assertRaises(ValueError):
                    serve.parse_named_secret(text)

    def test_the_names_of_horizon_and_of_the_system_are_refused(self):
        for name in ('PATH', 'HOME', 'USER', 'SHELL', 'TERM', 'LANG', 'DISPLAY', 'WAYLAND_DISPLAY',
                     'TMPDIR', 'LD_PRELOAD', 'DYLD_INSERT_LIBRARIES', 'SSH_AUTH_SOCK', 'HORIZON_HOME',
                     'CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN', 'CODEX_HOME', 'ANTHROPIC_API_KEY',
                     'OPENAI_API_KEY', 'XDG_RUNTIME_DIR', 'DBUS_SESSION_BUS_ADDRESS'):
            with self.subTest(name=name):
                with self.assertRaises(ValueError):
                    serve.parse_named_secret(f'{name}=value')

    def test_a_value_that_is_empty_has_a_line_break_or_is_too_large_is_refused(self):
        for text in ('KEY=', 'KEY=two\nlines', 'KEY=two\rlines', 'KEY=' + 'k' * (16 * 1024 + 1)):
            with self.subTest(text=text[:20]):
                with self.assertRaises(ValueError):
                    serve.parse_named_secret(text)

    def test_an_argument_without_a_value_is_refused(self):
        with self.assertRaises(ValueError):
            serve.parse_named_secret('HETZNER_TOKEN')


class SecretRowRuleTests(unittest.TestCase):
    def test_a_credential_id_and_a_value_are_read_from_one_argument(self):
        self.assertEqual(serve.parse_secret_row('credential/hetzner-token=a-value'),
                         ('credential/hetzner-token', 'a-value'))
        self.assertEqual(serve.parse_secret_row('credential/KEY_1=value=with=equals'),
                         ('credential/KEY_1', 'value=with=equals'))

    def test_an_id_that_is_not_a_credential_id_is_refused(self):
        for text in ('hetzner-token=a-value', 'named/TOKEN=a-value', 'credential/=a-value',
                     'credential/WITH SPACE=a-value', 'credential/A' * 1 + 'x' * 65 + '=a-value',
                     'credential/KEY=', 'credential/KEY=' + 'k' * (16 * 1024 + 1)):
            with self.subTest(text=text[:30]):
                with self.assertRaises(ValueError):
                    serve.parse_secret_row(text)


def rows_of(path):
    """The id and the body of every row of a secrets file."""
    connection = sqlite3.connect(path)
    try:
        return dict(connection.execute('SELECT id, body FROM secrets').fetchall())
    finally:
        connection.close()


class SecretSeedTests(unittest.TestCase):
    def test_the_values_go_to_the_secrets_file_and_the_names_to_the_config(self):
        with tempfile.TemporaryDirectory() as root:
            home = Path(root) / 'data'
            path = home / serve.SECRETS_FILE_NAME
            ids = serve.seed_secret_rows(path, [('HETZNER_TOKEN', 'a-value')],
                                         [('credential/grid-user', 'grid-value')])

            self.assertEqual(ids, ['credential/grid-user', 'named/HETZNER_TOKEN'])
            self.assertEqual(rows_of(path),
                             {'credential/grid-user': b'grid-value', 'named/HETZNER_TOKEN': b'a-value'})
            self.assertEqual(stat.S_IMODE(os.stat(path).st_mode), 0o600, 'the file of the values is private')
            self.assertEqual(stat.S_IMODE(os.stat(home).st_mode), 0o700, 'its folder is private')
            self.assertEqual(serve.named_secret_names(ids), ['HETZNER_TOKEN'])

            document = serve.application_document(
                '/tmp/fixture-data', secret_names=serve.named_secret_names(ids))
            self.assertEqual(document['secrets'], {'names': ['HETZNER_TOKEN']})
            self.assertNotIn('a-value', json.dumps(document), 'a setting holds no value')

    def test_a_later_run_of_the_same_state_keeps_the_values_that_it_does_not_name(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / 'data' / serve.SECRETS_FILE_NAME
            serve.seed_secret_rows(path, [('HETZNER_TOKEN', 'first'), ('CLOUD_KEY', 'second')])
            ids = serve.seed_secret_rows(path, [('HETZNER_TOKEN', 'replaced')])

            self.assertEqual(rows_of(path),
                             {'named/HETZNER_TOKEN': b'replaced', 'named/CLOUD_KEY': b'second'})
            self.assertEqual(serve.named_secret_names(ids), ['CLOUD_KEY', 'HETZNER_TOKEN'])

    def test_a_state_without_the_file_holds_no_secret(self):
        with tempfile.TemporaryDirectory() as root:
            ids = serve.read_secret_ids(Path(root) / 'absent' / serve.SECRETS_FILE_NAME)
            self.assertEqual(ids, [])
            document = serve.application_document('/tmp/fixture-data', secret_names=serve.named_secret_names(ids))
            self.assertNotIn('secrets', document)

    def test_the_schema_that_the_fixture_runs_is_the_schema_of_the_store(self):
        # `crates/horizon-core/src/secret_store/schema.rs` includes this file, so the
        # fixture and Horizon write one file and not two.
        schema = serve.SECRETS_SCHEMA_FILE.read_text()
        self.assertIn('CREATE TABLE secrets (', schema)
        self.assertIn('CREATE TABLE meta (', schema)
        self.assertTrue(serve.SECRETS_SCHEMA_FILE.is_file())


if __name__ == '__main__':
    unittest.main()
