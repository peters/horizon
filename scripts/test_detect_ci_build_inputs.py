#!/usr/bin/env python3
"""Tests for scripts/detect-ci-build-inputs.sh."""

import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "detect-ci-build-inputs.sh"
SELECT = ROOT / "scripts" / "select-release-artifact-platforms.py"
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"
ARTIFACTS_WORKFLOW = ROOT / ".github" / "workflows" / "release-artifacts.yml"
NIGHTLY_WORKFLOW = ROOT / ".github" / "workflows" / "macos-release-builds.yml"
MACOS_PLATFORMS = ["macos-arm64", "macos-x64"]
RUST_JOBS = ["browser-engine", "device", "clippy", "rust-test", "windows-smoke"]
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

    def assert_gates(self, outputs, *, snap, release, macos="false", rust="true"):
        self.assertEqual(outputs, {"run_snap_build": snap, "release_inputs": release,
                                   "macos_release_inputs": macos, "rust_inputs": rust})

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
                     ".github/workflows/release.yml"]:
            with self.subTest(path=path):
                self.setUp()
                self.commit(path)
                self.assert_gates(self.detect(), snap="true", release="false")

    def test_shared_inputs_gate_both_lanes(self):
        for path in [".github/workflows/ci.yml", "assets/icons/icon-512.png"]:
            with self.subTest(path=path):
                self.setUp()
                self.commit(path)
                self.assert_gates(self.detect(), snap="true", release="true")

    def test_artifact_build_definition_builds_macos_at_once(self):
        for path, snap in [(".github/workflows/release-artifacts.yml", "false"),
                           ("scripts/select-release-artifact-platforms.py", "false"),
                           ("scripts/install-ci-video-build-tools.sh", "false"),
                           ("scripts/package-release-asset.sh", "true")]:
            with self.subTest(path=path):
                self.setUp()
                self.commit(path)
                self.assert_gates(self.detect(), snap=snap, release="true", macos="true")

    def test_source_changes_leave_macos_to_the_nightly_build(self):
        for paths in [["crates/horizon-ui/src/app/mod.rs"], ["Cargo.lock", "Cargo.toml"],
                      ["rust-toolchain.toml"], ["assets/fonts/regular.ttf"],
                      [".github/workflows/macos-release-builds.yml"]]:
            with self.subTest(paths=paths):
                self.setUp()
                self.commit(*paths)
                self.assertEqual(self.detect()["macos_release_inputs"], "false")

    def test_macos_build_inputs_with_other_changes_still_build_macos(self):
        self.commit("docs/plan.md", "crates/horizon-ui/src/lib.rs",
                    "scripts/package-release-asset.sh")
        self.assert_gates(self.detect(), snap="true", release="true", macos="true")

    def test_a_range_without_changes_gates_both_lanes_off(self):
        self.assert_gates(self.detect(head=self.base), snap="false", release="false")

    def test_unknown_diff_bases_build_everything(self):
        self.commit("docs/plan.md")
        for base in ["", ZERO_SHA, "0" * 38, "deadbeef" * 5, "refs/heads/missing"]:
            with self.subTest(base=base):
                self.assert_gates(self.detect(base=base), snap="true", release="true",
                                  macos="true")

    def test_workflow_dispatch_builds_everything_from_a_resolvable_base(self):
        self.commit("docs/plan.md")
        self.assert_gates(self.detect(event="workflow_dispatch"), snap="true", release="true",
                          macos="true")

    def test_pull_request_outside_rust_skips_the_rust_jobs(self):
        for paths in [["docs/plan.md"], ["README.md", "AGENTS.md"],
                      [".github/copilot-instructions.md"], [".github/workflows/release.yml"],
                      [".github/dependabot.yml", "docs/testing/procedures/upkeep.md"],
                      [".claude/settings.json"]]:
            with self.subTest(paths=paths):
                self.setUp()
                self.commit(*paths)
                outputs = self.detect(event="pull_request")
                self.assertEqual(outputs["rust_inputs"], "false")

    def test_pull_request_with_any_other_path_runs_the_rust_jobs(self):
        for paths in [["crates/horizon-ui/src/lib.rs"], ["crates/horizon-ui/README.md"],
                      ["Cargo.lock"], [".github/workflows/ci.yml"],
                      ["scripts/ci-cargo-test-shard.sh"], ["examples/cloud.yml"],
                      ["skills/horizon-browser/SKILL.md"], [".horizon/version"],
                      ["docs/architecture/remote-device-testing.schema.json"],
                      ["docs/features/native-smoke.md"], ["docs/plan.md", "crates/x/src/lib.rs"],
                      ["assets/fonts/regular.ttf"], ["rust-toolchain.toml"]]:
            with self.subTest(paths=paths):
                self.setUp()
                self.commit(*paths)
                outputs = self.detect(event="pull_request")
                self.assertEqual(outputs["rust_inputs"], "true")

    def test_moving_a_source_file_into_docs_runs_the_rust_jobs(self):
        source = self.repo / "crates/horizon-core/src/upkeep.rs"
        source.parent.mkdir(parents=True)
        source.write_text("pub fn upkeep() {}\n" * 40)
        self.git("add", "-A")
        self.git("commit", "-m", "add source")
        self.base = self.git("rev-parse", "HEAD")
        (self.repo / "docs").mkdir()
        self.git("mv", "crates/horizon-core/src/upkeep.rs", "docs/upkeep.md")
        self.git("commit", "-m", "move source into docs")
        outputs = self.detect(event="pull_request")
        self.assertEqual(outputs["rust_inputs"], "true")
        self.assertEqual(outputs["release_inputs"], "true")

    def test_paths_git_would_quote_still_gate_the_release_matrix(self):
        for path in ["crates/horizon-ui/src/\u00e9.rs", 'crates/horizon-ui/src/we"ird.rs',
                     "crates/horizon-ui/src/back\\slash.rs"]:
            with self.subTest(path=path):
                self.setUp()
                self.commit(path)
                self.assert_gates(self.detect(), snap="false", release="true")

    def test_pushes_and_unknown_bases_always_run_the_rust_jobs(self):
        self.commit("docs/plan.md")
        self.assert_gates(self.detect(event="push"), snap="false", release="false", rust="true")
        self.assert_gates(self.detect(event="pull_request", base=ZERO_SHA),
                          snap="true", release="true", macos="true", rust="true")

    def test_wrong_argument_count_fails_without_output(self):
        for arguments in [[], ["push"], ["push", "HEAD"], ["push", "HEAD", "HEAD", "extra"]]:
            with self.subTest(arguments=arguments):
                result = subprocess.run([str(SCRIPT), *arguments], cwd=self.repo,
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.stdout, "")
                self.assertIn("usage:", result.stderr)


class WorkflowGateTests(unittest.TestCase):
    def test_rust_jobs_wait_for_the_change_detection_and_run_unless_told_not_to(self):
        workflow = WORKFLOW.read_text()
        jobs = jobs_of(workflow)
        self.assertIn("rust_inputs: ${{ steps.changes.outputs.rust_inputs }}", jobs["changes"])
        self.assertIn("detect-ci-build-inputs.sh", jobs["changes"])
        for name in RUST_JOBS:
            with self.subTest(job=name):
                self.assertIn("    needs: changes\n", jobs[name])
                # !cancelled(): a failed detection leaves the output empty and still runs the job.
                self.assertIn(
                    "    if: ${{ !cancelled() && needs.changes.outputs.rust_inputs != 'false' }}\n",
                    jobs[name],
                )
        self.assertNotIn("needs.repo-checks.outputs", workflow)

    def test_ci_builds_macos_only_for_artifact_build_inputs_or_a_dispatch(self):
        job = jobs_of(WORKFLOW.read_text())["rust-artifacts"]
        self.assertIn("    uses: ./.github/workflows/release-artifacts.yml\n", job)
        self.assertIn("needs.changes.outputs.release_inputs == 'true'", job)
        platforms = job.split("      platforms: >-\n", 1)[1]
        self.assertEqual(platforms.splitlines()[0].strip(), "linux-x64 windows-x64")
        self.assertIn(
            "${{ (github.event_name == 'workflow_dispatch' || "
            "needs.changes.outputs.macos_release_inputs == 'true') "
            "&& 'macos-arm64 macos-x64' || '' }}",
            platforms.splitlines()[1],
        )
        self.assertIn(
            "macos_release_inputs: ${{ steps.changes.outputs.macos_release_inputs }}",
            jobs_of(WORKFLOW.read_text())["changes"],
        )

    def test_nightly_builds_macos_on_a_schedule_and_on_dispatch(self):
        workflow = NIGHTLY_WORKFLOW.read_text()
        self.assertRegex(workflow, r"(?m)^  schedule:\n    #[^\n]*\n    - cron: '17 2 \* \* \*'$")
        self.assertRegex(workflow, r"(?m)^  workflow_dispatch:$")
        job = jobs_of(workflow)["rust-artifacts"]
        self.assertIn("    uses: ./.github/workflows/release-artifacts.yml\n", job)
        self.assertIn("    if: ${{ !cancelled() && needs.changes.outputs.build != 'false' }}\n", job)
        self.assertIn(f"      platforms: {' '.join(MACOS_PLATFORMS)}\n", job)
        self.assertIn("    if: github.event_name == 'schedule'\n", jobs_of(workflow)["changes"])

    def test_artifact_builds_keep_their_cache_keys_and_main_only_saves(self):
        workflow = ARTIFACTS_WORKFLOW.read_text()
        job = jobs_of(workflow)["rust-artifacts"]
        self.assertIn("          save-if: ${{ github.ref == 'refs/heads/main' }}\n", job)
        self.assertIn("          key: ${{ matrix.cache_key }}\n", job)
        # rust-cache hashes CARGO_* and RUST* into its key; a different set
        # would start every artifact cache cold.
        self.assertEqual(top_env(workflow), top_env(WORKFLOW.read_text()))


    def test_artifact_workflow_passes_the_selection_to_its_matrix(self):
        workflow = ARTIFACTS_WORKFLOW.read_text()
        self.assertRegex(workflow, r"(?m)^  workflow_call:\n    inputs:\n      platforms:\n")
        jobs = jobs_of(workflow.split("\njobs:\n", 1)[1])
        self.assertIn("PLATFORMS: ${{ inputs.platforms }}", jobs["platforms"])
        self.assertIn('select-release-artifact-platforms.py "$PLATFORMS" >> "$GITHUB_OUTPUT"',
                      jobs["platforms"])
        self.assertIn("include: ${{ steps.select.outputs.include }}", jobs["platforms"])
        self.assertIn("    needs: platforms\n", jobs["rust-artifacts"])
        self.assertIn("include: ${{ fromJSON(needs.platforms.outputs.include || '[]') }}",
                      jobs["rust-artifacts"])


def top_env(workflow):
    block = re.search(r"(?ms)^env:\n(.*?)^\S", workflow).group(1)
    return dict(re.findall(r"(?m)^  ([A-Z_]+): (.*)$", block))


def jobs_of(workflow):
    return dict(re.findall(r"^  ([\w-]+):\n(.*?)(?=^  [\w-]+:|\Z)", workflow, re.M | re.S))


class PlatformSelectionTests(unittest.TestCase):
    def select(self, *arguments):
        return subprocess.run([sys.executable, "-B", str(SELECT), *arguments],
                              capture_output=True, text=True)

    def include(self, selection):
        result = self.select(selection)
        self.assertEqual(result.returncode, 0, result.stderr)
        key, value = result.stdout.strip().split("=", 1)
        self.assertEqual(key, "include")
        return json.loads(value)

    def test_selects_the_named_platforms_with_empty_gaps(self):
        include = self.include("linux-x64 windows-x64\n ")
        self.assertEqual([entry["name"] for entry in include], ["Linux x64", "Windows x64"])
        include = self.include("  macos-arm64   macos-x64 ")
        self.assertEqual([entry["os"] for entry in include], ["macos-latest", "macos-15-intel"])

    def test_cache_keys_and_assets_stay_the_same(self):
        include = self.include("linux-x64 macos-arm64 macos-x64 windows-x64")
        self.assertEqual(
            [(e["cache_key"], e["release_asset"], e["binary_path"], e["asset_kind"])
             for e in include],
            [("linux-artifacts", "horizon-linux-x64.tar.gz", "target/release/horizon", "tar"),
             ("macos-artifacts", "horizon-osx-arm64.tar.gz", "target/release/horizon", "tar"),
             ("macos-intel-artifacts", "horizon-osx-x64.tar.gz", "target/release/horizon", "tar"),
             ("windows-artifacts", "horizon-windows-x64.exe", "target/release/horizon.exe",
              "file")],
        )

    def test_callers_name_only_known_platforms(self):
        ci = jobs_of(WORKFLOW.read_text())["rust-artifacts"]
        nightly = jobs_of(NIGHTLY_WORKFLOW.read_text())["rust-artifacts"]
        named = set(re.findall(r"\b(?:linux|macos|windows)-(?:x64|arm64)\b", ci + nightly))
        self.assertEqual(len(self.include(" ".join(sorted(named)))), 4)
        self.assertEqual(
            [e["id"] for e in self.include(" ".join(MACOS_PLATFORMS))], MACOS_PLATFORMS
        )

    def test_an_empty_or_unknown_selection_fails(self):
        for arguments in [[""], [" \n "], ["linux-x64 macos-arm"], ["linux"], [], ["a", "b"]]:
            with self.subTest(arguments=arguments):
                result = self.select(*arguments)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
