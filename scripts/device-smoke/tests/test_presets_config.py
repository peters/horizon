"""A procedure can give the device fixture the New Workspace presets it tests."""
from __future__ import annotations

from pathlib import Path
import json
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import serve  # noqa: E402

PI_AND_VARIANT = [
    {'name': 'Pi', 'alias': 'pi', 'kind': 'pi', 'command': None, 'args': [], 'resume': 'fresh'},
    {
        'name': 'Pi DeepSeek',
        'alias': 'pids',
        'kind': 'pi',
        'command': None,
        'args': ['--model', 'deepseek'],
        'resume': 'fresh',
    },
]


class PresetsConfigTests(unittest.TestCase):
    def test_an_omitted_list_leaves_the_menu_of_the_fixture_to_horizon(self):
        document = serve.application_document('/tmp/fixture-data')
        self.assertNotIn('presets', document)
        self.assertNotIn('presets', serve.application_document('/tmp/fixture-data', presets=None))

    def test_the_list_becomes_the_presets_of_the_fixture(self):
        document = serve.application_document('/tmp/fixture-data', presets=PI_AND_VARIANT)
        self.assertEqual(document['presets'], PI_AND_VARIANT)
        # Two presets of one agent stay two entries: the caret in New cloud needs both.
        self.assertEqual([preset['kind'] for preset in document['presets']], ['pi', 'pi'])

    def test_a_resume_mode_horizon_rejects_stops_the_fixture_before_it_starts(self):
        # Horizon starts with its own defaults when its config does not load, so the
        # fixture must not hand it a file whose presets it refuses.
        for resume in ('default', 'session', {'session': 'abc'}, {'session': {}}):
            with self.subTest(resume=resume), tempfile.TemporaryDirectory() as folder:
                path = Path(folder) / 'presets.json'
                path.write_text(json.dumps([{**PI_AND_VARIANT[0], 'resume': resume}]))
                with self.assertRaises(ValueError):
                    serve.load_presets(path)

    def test_the_resume_modes_horizon_reads_are_accepted(self):
        # `fresh` and `last` are the renamed unit variants; `session` keeps a session id
        # and needs the map form.
        for resume in ('fresh', 'last', {'session': {'session_id': 'abc'}}):
            with self.subTest(resume=resume), tempfile.TemporaryDirectory() as folder:
                path = Path(folder) / 'presets.json'
                path.write_text(json.dumps([{**PI_AND_VARIANT[0], 'resume': resume}]))
                self.assertEqual(len(serve.load_presets(path)), 1)

    def test_a_checked_list_is_returned_unchanged(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'presets.json'
            path.write_text(json.dumps(PI_AND_VARIANT))
            self.assertEqual(serve.load_presets(path), PI_AND_VARIANT)

    def test_a_preset_without_a_name_or_kind_stops_the_fixture(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'presets.json'
            path.write_text(json.dumps([{'name': 'Pi'}]))
            with self.assertRaisesRegex(ValueError, 'needs a name and a kind'):
                serve.load_presets(path)


if __name__ == '__main__':
    unittest.main()
