#!/usr/bin/env python3
"""The platform test shards keep one warm cache entry per OS."""

import os
import re
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "ci-cargo-test-shard.sh"
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"


def shard_commands(shard, runner_os, *, ref="", cache_hit=""):
    env = os.environ.copy()
    env["HORIZON_CI_SHARD"] = shard
    env["HORIZON_CI_SHARD_DRY_RUN"] = "1"
    env["RUNNER_OS"] = runner_os
    env.pop("GITHUB_REF", None)
    env.pop("HORIZON_CI_CACHE_HIT", None)
    if ref:
        env["GITHUB_REF"] = ref
    if cache_hit:
        env["HORIZON_CI_CACHE_HIT"] = cache_hit
    result = subprocess.run(
        ["bash", str(SCRIPT)],
        cwd=ROOT,
        env=env,
        check=False,
        capture_output=True,
        text=True,
    )
    return result


class ShardCommandTests(unittest.TestCase):
    def test_ui_and_libs_select_disjoint_packages(self):
        ui = shard_commands("ui", "Linux")
        libs = shard_commands("libs", "macOS")
        self.assertEqual(ui.returncode, 0, ui.stderr)
        self.assertEqual(libs.returncode, 0, libs.stderr)
        self.assertEqual(ui.stdout.splitlines(), ["cargo test --locked -p horizon-ui"])
        self.assertEqual(
            libs.stdout.splitlines(),
            [
                "cargo test --locked --workspace --exclude horizon-ui",
                "cargo test --locked -p horizon-chromecast --features encoder",
            ],
        )

    def test_speech_tests_run_on_unix_and_only_build_on_windows(self):
        linux = shard_commands("speech", "Linux", ref="refs/pull/1/merge", cache_hit="true")
        windows = shard_commands("speech", "Windows", cache_hit="true")
        self.assertEqual(linux.returncode, 0, linux.stderr)
        self.assertEqual(windows.returncode, 0, windows.stderr)
        self.assertEqual(
            linux.stdout.splitlines(),
            ["cargo test --locked -p horizon-ui --features speech"],
        )
        self.assertEqual(
            windows.stdout.splitlines(),
            ["cargo test --locked -p horizon-ui --features speech --no-run"],
        )

    def test_cold_main_speech_shard_builds_the_whole_workspace_first(self):
        cold = shard_commands("speech", "Linux", ref="refs/heads/main", cache_hit="false")
        warm = shard_commands("speech", "Linux", ref="refs/heads/main", cache_hit="true")
        self.assertEqual(cold.returncode, 0, cold.stderr)
        self.assertEqual(warm.returncode, 0, warm.stderr)
        self.assertEqual(
            cold.stdout.splitlines(),
            [
                "cargo test --locked --workspace --no-run",
                "cargo test --locked -p horizon-ui --features speech",
            ],
        )
        self.assertEqual(
            warm.stdout.splitlines(),
            ["cargo test --locked -p horizon-ui --features speech"],
        )

    def test_unknown_shard_fails(self):
        result = shard_commands("all", "Linux")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Unknown CI test shard", result.stderr)


class WorkflowShardTests(unittest.TestCase):
    def test_platform_jobs_share_one_cache_entry_per_os(self):
        workflow = WORKFLOW.read_text()
        jobs = dict(re.findall(r"^  ([\w-]+):\n(.*?)(?=^  [\w-]+:|\Z)", workflow, re.M | re.S))
        save_if = "save-if: ${{ github.ref == 'refs/heads/main' && matrix.shard == 'speech' }}"
        rust = jobs["rust-test"]
        windows = jobs["windows-smoke"]
        self.assertEqual(rust.count("shard: ui"), 2)
        self.assertEqual(rust.count("shard: libs"), 2)
        self.assertEqual(rust.count("shard: speech"), 2)
        self.assertIn("cache_key: linux-test", rust)
        self.assertIn("cache_key: macos-test", rust)
        self.assertIn("key: ${{ matrix.cache_key }}", rust)
        self.assertNotIn("matrix.shard", rust.split("save-if:", 1)[0].split("key:", 1)[-1])
        self.assertIn(save_if, rust)
        self.assertEqual(windows.count("shard: ui"), 1)
        self.assertEqual(windows.count("shard: libs"), 1)
        self.assertEqual(windows.count("shard: speech"), 1)
        self.assertIn("key: windows-test-build", windows)
        self.assertIn(save_if, windows)
        for job in (rust, windows):
            self.assertEqual(job.count("bash scripts/ci-cargo-test-shard.sh"), 1)
            self.assertNotIn("cargo test --locked --workspace\n", job)


if __name__ == "__main__":
    unittest.main()
