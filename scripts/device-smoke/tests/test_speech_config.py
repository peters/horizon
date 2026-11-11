"""The device fixture writes the speech section of its private settings."""
from __future__ import annotations

import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import serve  # noqa: E402


class SpeechSectionTests(unittest.TestCase):
    def test_a_run_that_names_no_speech_writes_no_section(self):
        self.assertIsNone(serve.speech_section(None, None, None))
        document = serve.application_document('/tmp/state')
        self.assertNotIn('speech', document, 'the candidate keeps the defaults of its own platform')

    def test_a_recorder_and_a_service_are_one_section(self):
        section = serve.speech_section('rec {file}', None, 'http://127.0.0.1:40913/v1/audio/transcriptions')
        self.assertEqual(section['recorder'], 'rec {file}')
        self.assertEqual(section['transcriber'],
                         {'kind': 'service', 'endpoint': 'http://127.0.0.1:40913/v1/audio/transcriptions'})
        document = serve.application_document('/tmp/state', speech=section)
        self.assertEqual(document['speech'], section)
        # The section is part of the settings document of the fixture, so the
        # candidate imports it at its first start.
        self.assertEqual(json.loads(json.dumps(document))['speech'], section)

    def test_a_recorder_and_a_command_are_one_section(self):
        section = serve.speech_section('rec {file}', 'my-transcriber {file}', None)
        self.assertEqual(section['transcriber'], {'kind': 'command', 'command': 'my-transcriber {file}'})

    def test_the_section_names_no_value_of_a_key(self):
        # The key is a named secret of the private home, so the section carries the
        # name of the secret and never a value. This is the shape that the kind
        # service leaves to the default name of the settings.
        section = serve.speech_section('rec {file}', None, 'http://127.0.0.1:1/v1')
        text = json.dumps(section)
        self.assertNotIn('key_secret', text, 'the fixture leaves the default name of the key')
        self.assertNotIn('SPEECH_API_KEY=', text)

    def test_a_plain_start_refuses_the_options_that_write_the_settings(self):
        class Args:
            def __init__(self, **values):
                for name, _flag in serve.PLAIN_START_SETTINGS_OPTIONS:
                    setattr(self, name, None)
                for name, value in values.items():
                    setattr(self, name, value)

        args = Args(speech_recorder='rec {file}')
        self.assertEqual(serve.plain_start_conflicts(args), ['--speech-recorder'])
        args = Args(speech_service='http://127.0.0.1:1/v1')
        self.assertEqual(serve.plain_start_conflicts(args), ['--speech-service'])
        self.assertEqual(serve.plain_start_conflicts(Args()), [])


if __name__ == '__main__':
    unittest.main()
