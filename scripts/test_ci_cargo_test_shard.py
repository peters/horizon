#!/usr/bin/env python3
"""The platform test shards keep one warm cache entry per OS."""

import os
import re
import subprocess
import tempfile
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

    def test_all_runs_libs_then_speech_without_the_plain_ui_pass(self):
        for ref, cache_hit in [("refs/pull/1/merge", "true"), ("refs/heads/main", "false")]:
            with self.subTest(ref=ref, cache_hit=cache_hit):
                result = shard_commands("all", "macOS", ref=ref, cache_hit=cache_hit)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(
                    result.stdout.splitlines(),
                    [
                        "cargo test --locked --workspace --exclude horizon-ui",
                        "cargo test --locked -p horizon-chromecast --features encoder",
                        "cargo test --locked -p horizon-ui --features speech",
                    ],
                )

    def test_all_runs_every_shard_and_fails_when_one_fails(self):
        env = os.environ.copy()
        env.update(HORIZON_CI_SHARD="all", RUNNER_OS="macOS")
        env.pop("HORIZON_CI_SHARD_DRY_RUN", None)
        with tempfile.TemporaryDirectory() as bin_dir:
            log = Path(bin_dir) / "calls"
            fake = Path(bin_dir) / "cargo"
            # The libs workspace run fails; every later command must still run.
            fake.write_text(
                "#!/usr/bin/env bash\n"
                f"echo \"$*\" >> {log}\n"
                "[[ \"$*\" == *'--exclude horizon-ui'* ]] && exit 101\n"
                "exit 0\n"
            )
            fake.chmod(0o755)
            env["PATH"] = f"{bin_dir}:{env['PATH']}"
            result = subprocess.run(["bash", str(SCRIPT)], cwd=ROOT, env=env,
                                    capture_output=True, text=True, check=False)
            calls = log.read_text().splitlines()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Failed CI test shards: libs", result.stderr)
        self.assertEqual(
            calls,
            [
                "test --locked --workspace --exclude horizon-ui",
                "test --locked -p horizon-chromecast --features encoder",
                "test --locked -p horizon-ui --features speech",
            ],
        )

    def test_unknown_shard_fails(self):
        result = shard_commands("every", "Linux")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Unknown CI test shard", result.stderr)


class WorkflowShardTests(unittest.TestCase):
    def test_platform_jobs_share_one_cache_entry_per_os(self):
        workflow = WORKFLOW.read_text()
        jobs = dict(re.findall(r"^  ([\w-]+):\n(.*?)(?=^  [\w-]+:|\Z)", workflow, re.M | re.S))
        rust = jobs["rust-test"]
        windows = jobs["windows-smoke"]
        entries = [
            dict(re.findall(r"^\s+-?\s*(\w+): (.+)$", entry, re.M))
            for entry in re.split(r"^\s+- os: ", rust.split("include:", 1)[1].split("steps:", 1)[0], flags=re.M)[1:]
        ]
        shards = {(entry["name"], entry["shard"]) for entry in entries}
        self.assertEqual(
            shards,
            {("Linux x64", "ui"), ("Linux x64", "libs"), ("Linux x64", "speech"), ("macOS arm64", "all")},
        )
        # Exactly one job per OS saves the entry the others restore.
        savers = {(entry["name"], entry["shard"]) for entry in entries if entry.get("save_cache") == "true"}
        self.assertEqual(savers, {("Linux x64", "speech"), ("macOS arm64", "all")})
        self.assertIn("cache_key: linux-test", rust)
        self.assertIn("cache_key: macos-test", rust)
        self.assertIn("key: ${{ matrix.cache_key }}", rust)
        self.assertNotIn("matrix.shard", rust.split("save-if:", 1)[0].split("key:", 1)[-1])
        self.assertIn("save-if: ${{ github.ref == 'refs/heads/main' && matrix.save_cache == true }}", rust)
        self.assertIn("timeout-minutes: ${{ matrix.timeout_minutes || 35 }}", rust)
        self.assertIn("if: ${{ !cancelled() && startsWith(matrix.os, 'macos-') && matrix.shard == 'all' }}", rust)
        self.assertEqual(windows.count("shard: ui"), 1)
        self.assertEqual(windows.count("shard: libs"), 1)
        self.assertEqual(windows.count("shard: speech"), 1)
        self.assertIn("key: windows-test-build", windows)
        self.assertIn("save-if: ${{ github.ref == 'refs/heads/main' && matrix.shard == 'speech' }}", windows)
        for job in (rust, windows):
            self.assertEqual(job.count("bash scripts/ci-cargo-test-shard.sh"), 1)
            self.assertNotIn("cargo test --locked --workspace\n", job)

    def test_apple_mobile_targets_install_retries_three_times(self):
        workflow = WORKFLOW.read_text()
        start = workflow.index("          for attempt in 1 2 3; do\n")
        retry = workflow[start:workflow.index("\n          done\n", start) + len("\n          done\n")]
        self.assertIn("rustup target add aarch64-apple-ios aarch64-apple-ios-sim", retry)
        for failures, status in [(0, 0), (2, 0), (3, 1)]:
            with self.subTest(failures=failures), tempfile.TemporaryDirectory() as bin_dir:
                calls = Path(bin_dir) / "calls"
                for name, body in [("rustup", f"echo x >> {calls}\n[ $(wc -l < {calls}) -gt {failures} ]\n"),
                                   ("sleep", "exit 0\n")]:
                    tool = Path(bin_dir) / name
                    tool.write_text("#!/usr/bin/env bash\n" + body)
                    tool.chmod(0o755)
                env = dict(os.environ, PATH=f"{bin_dir}:{os.environ['PATH']}")
                result = subprocess.run(["bash", "-eo", "pipefail", "-c", retry], env=env,
                                        capture_output=True, text=True, check=False)
                self.assertEqual(result.returncode, status, result.stdout + result.stderr)
                self.assertEqual(len(calls.read_text().splitlines()), min(failures + 1, 3))
                self.assertEqual("failed three times" in result.stdout, status == 1)


if __name__ == "__main__":
    unittest.main()
