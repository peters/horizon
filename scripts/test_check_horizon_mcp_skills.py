#!/usr/bin/env python3
"""Tests for scripts/check-horizon-mcp-skills.py and its shipped skill files."""

import json
import re
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "check-horizon-mcp-skills.py"
BUNDLES = [ROOT / "assets/plugins/codex/skills", ROOT / "assets/plugins/claude-code/skills"]
JSON_BLOCK = re.compile(r"```json\n(.*?)```", re.S)


class CoverageCheckTests(unittest.TestCase):
    def test_the_bundled_skills_pass_the_coverage_check(self):
        result = subprocess.run(
            [sys.executable, "-B", str(SCRIPT)],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("parity, references, and embedding pass", result.stdout)

    def test_every_fenced_json_block_in_a_shipped_skill_parses(self):
        blocks = 0
        for bundle in BUNDLES:
            for path in sorted(bundle.rglob("*.md")):
                text = path.read_text()
                for match in JSON_BLOCK.finditer(text):
                    with self.subTest(file=str(path.relative_to(ROOT)), offset=match.start()):
                        json.loads(match.group(1))
                    blocks += 1
        self.assertGreater(blocks, 0, "the shipped skills hold card documents")

    def test_a_card_document_names_only_the_fields_that_the_block_skill_names(self):
        import importlib.util

        spec = importlib.util.spec_from_file_location("coverage_check", SCRIPT)
        check = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(check)
        skill = "`width`, `finished` and `components`\n"
        example = '```json\n{"operation": "create", "document": {"components": [], "position": [1, 2]}}\n```\n'

        self.assertEqual(check.card_document_errors({"horizon-block": skill}), [])
        self.assertEqual(
            check.card_document_errors({"horizon-block": "`width` and `components`\n"}),
            ["finished: the field of a card document is missing from horizon-block"],
        )
        errors = check.card_document_errors({"horizon-block": skill, "horizon-setup": example})
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("horizon-setup: a card example names the field position", errors[0])


if __name__ == "__main__":
    unittest.main()
