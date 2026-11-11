"""The device fixture seeds named secrets into its private state, and into no setting."""
from __future__ import annotations

import json
import os
from pathlib import Path
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


class NamedSecretSeedTests(unittest.TestCase):
    def test_the_values_go_to_the_private_state_and_the_names_to_the_config(self):
        with tempfile.TemporaryDirectory() as root:
            state = Path(root) / 'named-secrets.json'
            values = serve.seed_named_secrets(state, [('HETZNER_TOKEN', 'a-value')])

            self.assertEqual(values, {'named/HETZNER_TOKEN': 'a-value'})
            stored = json.loads(state.read_text())
            self.assertEqual(stored, {'named/HETZNER_TOKEN': 'a-value'})
            mode = stat.S_IMODE(os.stat(state).st_mode)
            self.assertEqual(mode, 0o600, 'the file of the values is private')
            self.assertEqual(serve.named_secret_names(stored), ['HETZNER_TOKEN'])

            document = serve.application_document(
                '/tmp/fixture-data', secret_names=serve.named_secret_names(stored))
            self.assertEqual(document['secrets'], {'names': ['HETZNER_TOKEN']})
            self.assertNotIn('a-value', json.dumps(document), 'a setting holds no value')

    def test_a_later_run_of_the_same_state_keeps_the_values_that_it_does_not_name(self):
        with tempfile.TemporaryDirectory() as root:
            state = Path(root) / 'named-secrets.json'
            serve.seed_named_secrets(state, [('HETZNER_TOKEN', 'first'), ('CLOUD_KEY', 'second')])
            held = serve.read_named_secrets(state)
            values = serve.seed_named_secrets(state, [('HETZNER_TOKEN', 'replaced')], held)

            self.assertEqual(values, {'named/HETZNER_TOKEN': 'replaced', 'named/CLOUD_KEY': 'second'})
            self.assertEqual(serve.named_secret_names(values), ['CLOUD_KEY', 'HETZNER_TOKEN'])

    def test_a_state_without_the_file_holds_no_named_secret(self):
        with tempfile.TemporaryDirectory() as root:
            held = serve.read_named_secrets(Path(root) / 'absent.json')
            self.assertEqual(held, {})
            document = serve.application_document('/tmp/fixture-data', secret_names=serve.named_secret_names(held))
            self.assertNotIn('secrets', document)

    def test_the_variable_that_names_the_file_is_the_one_horizon_reads(self):
        # `crates/horizon-core/src/named_secrets/test_file.rs` reads this name, and
        # `tests/named_secrets` of the crate names the same text.
        self.assertEqual(serve.NAMED_SECRETS_ENV, 'HORIZON_TEST_NAMED_SECRETS_FILE')


if __name__ == '__main__':
    unittest.main()
