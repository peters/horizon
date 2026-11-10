"""The device fixture writes the appearance theme that the procedure asks for."""
from __future__ import annotations

from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import serve  # noqa: E402


class ThemeConfigTests(unittest.TestCase):
    def test_a_document_without_a_theme_stays_dark(self):
        document = serve.application_document('/tmp/fixture-data')
        self.assertEqual(document['appearance'], {'theme': 'dark'})

    def test_the_requested_theme_reaches_the_config(self):
        for theme in ('auto', 'dark', 'light'):
            with self.subTest(theme=theme):
                document = serve.application_document('/tmp/fixture-data', theme=theme)
                self.assertEqual(document['appearance'], {'theme': theme})

    def test_the_theme_changes_nothing_else_in_the_document(self):
        dark = serve.application_document('/tmp/fixture-data')
        light = serve.application_document('/tmp/fixture-data', theme='light')
        dark.pop('appearance')
        light.pop('appearance')
        self.assertEqual(dark, light)


if __name__ == '__main__':
    unittest.main()
