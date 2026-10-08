#!/usr/bin/env python3
"""Meaningful bounds tests for the maintenance demo, without provider access."""
import contextlib
import hashlib
import io
import json
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import worker


class MaintenanceSafety(unittest.TestCase):
    def test_policy_rejects_untrusted_shapes_and_persists_across_initialization(self):
        with tempfile.TemporaryDirectory(prefix="policy-") as temporary:
            root = Path(temporary)
            worker.initial(root)
            valid = {"global_prompt": "Keep repository checks mandatory.",
                     "repo_prompts": {"example/sample-web": "Inspect the synthetic browser recipe."}}
            acknowledged = worker.configure(root, valid)
            self.assertEqual(acknowledged["configRevision"], 2)
            restarted = worker.initial(root)
            self.assertEqual(restarted["configured_revision"], 2)
            self.assertEqual(restarted["global_prompt"], valid["global_prompt"])
            self.assertIsNone(restarted["applied_revision"])
            self.assertEqual(restarted["repos"][0]["prompt"], valid["repo_prompts"]["example/sample-web"])
            cases = [dict(valid, shell="gh api"),
                     {"global_prompt": "x", "repo_prompts": {"unknown/repository": "x"}},
                     {"global_prompt": "x" * 4097, "repo_prompts": {}},
                     {"global_prompt": "x", "repo_prompts": {"example/sample-web": "x" * 2049}},
                     {"global_prompt": 4, "repo_prompts": {}},
                     {"global_prompt": "x\x00", "repo_prompts": {}},
                     {"global_prompt": "x", "repo_prompts": {f"example/{r[0]}": "ø" * 2048 for r in worker.REPOS}}]
            for value in cases:
                with self.assertRaises(ValueError):
                    worker.configure(root, value)
            self.assertEqual(worker.load_policy(root)["revision"], 2)

    def test_run_preserves_configuration_without_external_commands_or_network(self):
        with tempfile.TemporaryDirectory(prefix="safety-") as temporary:
            root = Path(temporary)
            initial = worker.initial(root)
            self.assertEqual(len(initial["repos"]), len(worker.REPOS))
            self.assertEqual(len(initial["repos"]), 21)
            self.assertGreaterEqual(len(initial["prs"]), 34)
            self.assertEqual(sum(not repo["enabled"] for repo in initial["repos"]), len(worker.DISABLED))
            self.assertEqual({repo["status"] for repo in initial["repos"]},
                             {"queued", "blocked", "complete", "idle", "disabled"})
            config_hashes = {repo["repository"]: repo["dependabot_sha256"] for repo in initial["repos"]}
            check_hashes = {path: hashlib.sha256(path.read_bytes()).hexdigest()
                            for path in (root / "repos").glob("*/checks.py")}
            real_run = subprocess.run
            executed = []

            def bounded_check(arguments, **options):
                # A GitHub, cloud, curl or shell call fails this test before execution.
                self.assertEqual(arguments, [sys.executable, "checks.py"])
                cwd = Path(options["cwd"]).resolve()
                self.assertTrue(cwd.is_relative_to(root / "repos"))
                self.assertEqual(options["timeout"], 10)
                executed.append(cwd.name)
                return real_run(arguments, **options)

            def reject_network(*_arguments, **_options):
                self.fail("The synthetic worker attempted a network connection")

            with patch.object(worker.subprocess, "run", side_effect=bounded_check), \
                 patch.object(socket.socket, "connect", side_effect=reject_network), \
                 patch.object(socket.socket, "connect_ex", side_effect=reject_network), \
                 contextlib.redirect_stdout(io.StringIO()):
                worker.run(root, delay=0)

            result = json.loads((root / "status.json").read_text())
            self.assertEqual(result["phase"], "cycle_completed")
            self.assertEqual(result["executed_count"], worker.EXECUTION_LIMIT)
            self.assertEqual(result["completed_count"], initial["completed_count"] + worker.EXECUTION_LIMIT)
            self.assertGreater(result["queued_count"], 0)
            self.assertEqual(result["blocked_count"], initial["blocked_count"])
            self.assertIsNone(result["active"])
            self.assertEqual(executed, ["sample-web", "sample-service", "sample-service", "sample-desktop"])
            for repo in result["repos"]:
                digest = hashlib.sha256(Path(repo["dependabot_path"]).read_bytes()).hexdigest()
                self.assertEqual(digest, config_hashes[repo["repository"]])
                self.assertTrue(repo["instructions"])
                self.assertEqual(repo["instruction_execution"], "read; explicit fixture recipe only")
            for path, digest in check_hashes.items():
                self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), digest)
            service = next(pr for pr in result["prs"] if pr["repository"] == "example/sample-service" and pr["primary"])
            self.assertFalse(service["checks"][0]["passed"])
            self.assertTrue(service["checks"][1]["passed"])
            self.assertEqual(service["head"], "fixture-repair")
            for pr in result["prs"]:
                self.assertTrue(pr["synthetic"])
                self.assertEqual(pr["url"], f"https://github.com/{pr['repository']}/pull/{pr['number']}")
                if not pr["execution_selected"]:
                    self.assertEqual(pr["checks"], [])


if __name__ == "__main__":
    unittest.main()
