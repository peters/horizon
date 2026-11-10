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


if __name__ == "__main__":
    unittest.main()
