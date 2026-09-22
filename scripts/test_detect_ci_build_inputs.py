#!/usr/bin/env python3
"""Tests for scripts/detect-ci-build-inputs.sh."""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "detect-ci-build-inputs.sh"
ZERO_SHA = "0" * 40

GIT_ENV = {
    "GIT_AUTHOR_NAME": "CI Fixture",
    "GIT_AUTHOR_EMAIL": "ci@example.invalid",
    "GIT_COMMITTER_NAME": "CI Fixture",
    "GIT_COMMITTER_EMAIL": "ci@example.invalid",
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_SYSTEM": os.devnull,
}


class DetectionTests(unittest.TestCase):
    def setUp(self):
        self.repo = Path(tempfile.mkdtemp(prefix="detect-ci-inputs-"))
        self.addCleanup(subprocess.run, ["rm", "-rf", str(self.repo)], check=False)
        self.git("init", "-b", "main")
        self.base = self.commit("README.md")

    def git(self, *args):
        result = subprocess.run(
            ["git", *args],
            cwd=self.repo,
            env={**os.environ, **GIT_ENV},
            capture_output=True,
            text=True,
            check=True,
        )
        return result.stdout.strip()

    def commit(self, *paths):
        for path in paths:
            target = self.repo / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(f"{path}\n{len(self.repo.name)}\n")
        self.git("add", "-A")
        self.git("commit", "-m", f"touch {' '.join(paths)}")
        return self.git("rev-parse", "HEAD")

    def detect(self, event="push", base=None, head="HEAD"):
        arguments = [str(SCRIPT), event]
        arguments.append(self.base if base is None else base)
        arguments.append(head)
        result = subprocess.run(arguments, cwd=self.repo, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        return dict(line.split("=", 1) for line in result.stdout.split())

    def assert_gates(self, outputs, *, snap, release):
        self.assertEqual(outputs, {"run_snap_build": snap, "release_inputs": release})

    def test_documentation_only_change_gates_both_lanes_off(self):
        self.commit("docs/architecture/maintainability.md", "README.md")
        self.assert_gates(self.detect(), snap="false", release="false")

    def test_source_and_manifest_changes_gate_the_release_matrix(self):
        for path in ["crates/horizon-ui/src/app/mod.rs", "Cargo.toml", "Cargo.lock",
                     "rust-toolchain.toml", "assets/fonts/regular.ttf"]:
            with self.subTest(path=path):
                self.setUp()
                self.commit(path)
                self.assert_gates(self.detect(), snap="false", release="true")

    def test_packaging_changes_gate_the_snap_build(self):
        for path in ["snap/snapcraft.yaml", "packaging/linux/horizon.desktop",
                     "scripts/build-surge-toolchain.sh", "scripts/stage-surge-artifacts.sh",
                     ".github/workflows/release.yml"]:
            with self.subTest(path=path):
                self.setUp()
                self.commit(path)
                self.assert_gates(self.detect(), snap="true", release="false")

    def test_shared_inputs_gate_both_lanes(self):
        for path in [".github/workflows/ci.yml", "scripts/package-release-asset.sh",
                     "assets/icons/icon-512.png"]:
            with self.subTest(path=path):
                self.setUp()
                self.commit(path)
                self.assert_gates(self.detect(), snap="true", release="true")

    def test_a_range_without_changes_gates_both_lanes_off(self):
        self.assert_gates(self.detect(head=self.base), snap="false", release="false")

    def test_unknown_diff_bases_build_everything(self):
        self.commit("docs/plan.md")
        for base in ["", ZERO_SHA, "0" * 38, "deadbeef" * 5, "refs/heads/missing"]:
            with self.subTest(base=base):
                self.assert_gates(self.detect(base=base), snap="true", release="true")

    def test_workflow_dispatch_builds_everything_from_a_resolvable_base(self):
        self.commit("docs/plan.md")
        self.assert_gates(self.detect(event="workflow_dispatch"), snap="true", release="true")

    def test_wrong_argument_count_fails_without_output(self):
        for arguments in [[], ["push"], ["push", "HEAD"], ["push", "HEAD", "HEAD", "extra"]]:
            with self.subTest(arguments=arguments):
                result = subprocess.run([str(SCRIPT), *arguments], cwd=self.repo,
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.stdout, "")
                self.assertIn("usage:", result.stderr)


if __name__ == "__main__":
    unittest.main()
