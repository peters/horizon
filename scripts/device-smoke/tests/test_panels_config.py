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


def overlaps(first, second):
    """Whether two panel rectangles share a point."""
    (left, top), (width, height) = first['position'], first['size']
    (other_left, other_top), (other_width, other_height) = second['position'], second['size']
    return (left < other_left + other_width and other_left < left + width
            and top < other_top + other_height and other_top < top + height)


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
        # The default board ends at x 1180, so the column starts one step further right.
        self.assertEqual(placed['position'], [1220, 60])
        self.assertEqual(placed['size'], [330, 440])
        self.assertNotIn('position', panel, 'the file of the procedure stays unchanged')
        for default in document['workspaces'][0]['terminals'][:2]:
            self.assertFalse(overlaps(placed, default), default['name'])

    def test_a_panel_without_a_position_clears_a_replaced_board(self):
        # The Firefox disclosure board ends at x 1440, so a fixed column would cover it.
        document = serve.application_document(
            '/tmp/fixture-data', firefox_system_access=True,
            extra_panels=[{'name': 'Second agent', 'kind': 'codex', 'command': '/bin/true'}])
        placed = document['workspaces'][0]['terminals'][-1]
        self.assertEqual(placed['position'], [1480, 60])
        for panel in document['workspaces'][0]['terminals'][:-1]:
            self.assertFalse(overlaps(placed, panel), panel['name'])

    def test_two_panels_without_a_position_share_the_column_and_do_not_overlap(self):
        panels = [{'name': 'One', 'kind': 'codex'}, {'name': 'Two', 'kind': 'codex'}]
        document = serve.application_document('/tmp/fixture-data', extra_panels=panels)
        first, second = document['workspaces'][0]['terminals'][-2:]
        self.assertEqual(first['position'][0], second['position'][0])
        self.assertFalse(overlaps(first, second))

    def test_a_checked_list_is_returned_unchanged(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'panels.json'
            path.write_text(json.dumps([SCRIPTED_AGENT]))
            self.assertEqual(serve.load_panels(path), [SCRIPTED_AGENT])

    def test_a_panel_horizon_would_refuse_stops_the_fixture(self):
        for panel, problem in (
            ({'kind': 'codex'}, 'needs a name that is a string'),
            ({'name': 5}, 'needs a name that is a string'),
            ({'name': 'Agent', 'kind': 3}, 'is not a panel kind'),
            ({'name': 'Agent', 'kind': 'agent'}, 'is not a panel kind'),
            ({'name': 'Agent', 'kind': 'Codex'}, 'is not a panel kind'),
            ({'name': 'Agent', 'args': 'x'}, 'args must be a list of strings'),
            ({'name': 'Agent', 'args': ['-c', 1]}, 'args must be a list of strings'),
            ({'name': 'Agent', 'command': 7}, 'command must be a string'),
            ({'name': 'Agent', 'cwd': []}, 'cwd must be a string'),
            ({'name': 'Agent', 'position': '40,560'}, 'position must be two numbers'),
            ({'name': 'Agent', 'position': [40]}, 'position must be two numbers'),
            ({'name': 'Agent', 'size': [1, True]}, 'size must be two numbers'),
            ({'name': 'Agent', 'resume': 'session'}, 'resume must be fresh, last'),
            ('not a panel', 'must be an object'),
        ):
            with self.subTest(panel=panel), tempfile.TemporaryDirectory() as folder:
                path = Path(folder) / 'panels.json'
                path.write_text(json.dumps([panel]))
                with self.assertRaisesRegex(ValueError, problem):
                    serve.load_panels(path)

    def test_every_panel_kind_of_horizon_is_accepted(self):
        for kind in sorted(serve.PANEL_KINDS):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as folder:
                path = Path(folder) / 'panels.json'
                path.write_text(json.dumps([{'name': 'Panel', 'kind': kind}]))
                self.assertEqual(len(serve.load_panels(path)), 1)

    def test_a_list_is_required(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'panels.json'
            path.write_text(json.dumps({'name': 'one'}))
            with self.assertRaisesRegex(ValueError, 'must be a JSON list'):
                serve.load_panels(path)


if __name__ == '__main__':
    unittest.main()
