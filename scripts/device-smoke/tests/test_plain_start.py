"""A plain start of the fixture, and the secrets file that its home holds.

A run with `--plain-start` gives the binary no argument at all, so an older
release can make its own home, and a run with `--reuse-state` starts this build on
that home: that is the upgrade that a procedure shows. The secrets file of every
run is the file of the Horizon home of the fixture, which is `<private
home>/.horizon/secrets.sqlite`, because that is the path that
`HorizonHome::secrets_path` resolves.
"""
from __future__ import annotations

import argparse
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import sandbox  # noqa: E402
import serve  # noqa: E402

ROOT = Path(__file__).resolve().parents[3]


def args_with(**options):
    """The parsed options of a run, with every option of the fixture off."""
    values = {name: False for name, _ in serve.PLAIN_START_SETTINGS_OPTIONS}
    values['theme'] = None
    values['named_secret'] = None
    values['secret_row'] = None
    values.update(options)
    return argparse.Namespace(**values)


class ApplicationCommand(unittest.TestCase):
    def test_a_plain_start_gives_the_binary_no_argument(self):
        command = serve.application_command(
            Path('/opt/horizon'), Path('/state/data/horizon.json'), False, False, plain_start=True)

        self.assertEqual(command, ['/opt/horizon'])

    def test_a_run_without_a_plain_start_keeps_its_settings_and_its_session(self):
        self.assertEqual(
            serve.application_command(Path('/opt/horizon'), Path('/state/data/horizon.json'), False),
            ['/opt/horizon', '--config', '/state/data/horizon.json', '--ephemeral'])
        self.assertEqual(
            serve.application_command(Path('/opt/horizon'), Path('/state/data/horizon.json'), False, True),
            ['/opt/horizon', '--config', '/state/data/horizon.json'])


class PlainStartRefusals(unittest.TestCase):
    def test_a_plain_start_alone_is_accepted(self):
        self.assertEqual(serve.plain_start_conflicts(args_with()), [])

    def test_every_option_that_writes_a_settings_file_is_refused(self):
        for name, flag in serve.PLAIN_START_SETTINGS_OPTIONS:
            for value in (True, ['a-value'], Path('/tmp/presets.json'), 'dark'):
                with self.subTest(flag=flag, value=value):
                    conflicts = serve.plain_start_conflicts(args_with(**{name: value}))
                    self.assertEqual(conflicts, [flag], f'{flag} writes the settings of the fixture')

    def test_the_sentence_names_each_refused_option(self):
        conflicts = serve.plain_start_conflicts(
            args_with(presets=Path('/tmp/presets.json'), theme='light', saved_session=True))

        self.assertEqual(conflicts, ['--presets', '--theme', '--saved-session'])


class SecretsFileOfTheFixture(unittest.TestCase):
    def test_the_secrets_file_is_the_file_of_the_horizon_home_of_the_fixture(self):
        self.assertEqual(
            serve.horizon_secrets_file(Path('/state/data/home')),
            Path('/state/data/home/.horizon/secrets.sqlite'))

    def test_the_state_folder_masks_the_home_that_the_secrets_file_is_under(self):
        with tempfile.TemporaryDirectory() as parent:
            state = sandbox.prepare_state(Path(parent) / 'run', reuse=False)
            box = sandbox.prepare(state)

            secrets = serve.horizon_secrets_file(box.private_home)
            self.assertEqual(box.private_home, state / 'data' / 'home')
            self.assertEqual(secrets, state / 'data' / 'home' / '.horizon' / 'secrets.sqlite')
            # `HOME` in the namespace is the private home, so the path that the
            # candidate resolves is the path that the fixture writes.
            self.assertIn('--bind', box.namespace)
            index = box.namespace.index(str(box.private_home))
            self.assertEqual(box.namespace[index - 1], '--bind')

    def test_the_fixture_follows_the_path_rules_of_the_crate(self):
        # The file name, the home folder and the path of the secrets file are the
        # rules of `crates/horizon-core/src/horizon_home.rs` and
        # `crates/horizon-browser-control/src/paths.rs`. A rename in either place
        # leaves a fixture that seeds a file that no panel reads, which is the
        # defect that this test keeps out.
        home = (ROOT / 'crates/horizon-core/src/horizon_home.rs').read_text()
        self.assertIn('self.root.join(crate::secret_store::SECRETS_FILE_NAME)', home)
        secrets = (ROOT / 'crates/horizon-core/src/secret_store/mod.rs').read_text()
        self.assertIn(f'pub const SECRETS_FILE_NAME: &str = "{serve.SECRETS_FILE_NAME}";', secrets)
        paths = (ROOT / 'crates/horizon-browser-control/src/paths.rs').read_text()
        self.assertIn(f"PathBuf::from(\"{serve.HORIZON_HOME_FOLDER}\")", paths)
        self.assertIn(f'.join("{serve.HORIZON_HOME_FOLDER}")', paths)


if __name__ == '__main__':
    unittest.main()
