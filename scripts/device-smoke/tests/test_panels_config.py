"""A procedure can give the device fixture extra workspace panels it needs."""
from __future__ import annotations

from pathlib import Path
import json
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import serve  # noqa: E402

SCRIPTED_AGENT = {
    'name': 'Cloud plan',
    'kind': 'codex',
    'command': '/usr/bin/python3',
    'args': ['/tmp/plan.py'],
    'position': [40, 560],
    'size': [560, 380],
}


class PanelsConfigTests(unittest.TestCase):
    def test_an_omitted_list_keeps_the_default_board(self):
        document = serve.application_document('/tmp/fixture-data')
        self.assertEqual([panel['name'] for panel in document['workspaces'][0]['terminals']],
                         ['Live render heartbeat', 'Device input test'])
        self.assertEqual(
            serve.application_document('/tmp/fixture-data', extra_panels=[]),
            document)

    def test_an_extra_panel_is_added_beside_the_default_panels(self):
        document = serve.application_document('/tmp/fixture-data', extra_panels=[SCRIPTED_AGENT])
        names = [panel['name'] for panel in document['workspaces'][0]['terminals']]
        self.assertEqual(names, ['Live render heartbeat', 'Device input test', 'Cloud plan'])
        self.assertEqual(document['workspaces'][0]['terminals'][2]['kind'], 'codex')

    def test_a_panel_without_a_position_is_placed_out_of_the_way(self):
        panel = {'name': 'Second agent', 'kind': 'codex', 'command': '/bin/true'}
        document = serve.application_document('/tmp/fixture-data', extra_panels=[panel])
        placed = document['workspaces'][0]['terminals'][2]
        self.assertEqual(placed['position'], [1240, 60])
        self.assertEqual(placed['size'], [330, 440])
        self.assertNotIn('position', panel, 'the file of the procedure stays unchanged')

    def test_a_checked_list_is_returned_unchanged(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'panels.json'
            path.write_text(json.dumps([SCRIPTED_AGENT]))
            self.assertEqual(serve.load_panels(path), [SCRIPTED_AGENT])

    def test_a_panel_the_fixture_cannot_place_stops_it_before_it_starts(self):
        for panel, problem in (
            ({'kind': 'codex'}, 'needs a name'),
            ({'name': 'Agent', 'kind': 3}, 'kind must be a string'),
            ({'name': 'Agent', 'args': 'x'}, 'args must be a list'),
            ('not a panel', 'needs a name'),
        ):
            with self.subTest(panel=panel), tempfile.TemporaryDirectory() as folder:
                path = Path(folder) / 'panels.json'
                path.write_text(json.dumps([panel]))
                with self.assertRaisesRegex(ValueError, problem):
                    serve.load_panels(path)

    def test_a_list_is_required(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'panels.json'
            path.write_text(json.dumps({'name': 'one'}))
            with self.assertRaisesRegex(ValueError, 'must be a JSON list'):
                serve.load_panels(path)


if __name__ == '__main__':
    unittest.main()
