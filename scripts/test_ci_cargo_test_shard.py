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

# The opt-in features of the cloud, gateway and encoder lanes build together.
FEATURES = (
    "horizon-cloud/exposure-firewall,horizon-core/one-service-gateway,"
    "horizon-core/cloud-exposure,horizon-chromecast/encoder"
)
# The shipped horizon-cloud build, without an opt-in feature.
DEFAULT_CLOUD = "cargo test --locked -p horizon-cloud"
BUILD = f"cargo test --locked --workspace --exclude horizon-ui --no-run --features {FEATURES}"
WORKSPACE = f"cargo test --locked --workspace --exclude horizon-ui --features {FEATURES}"
EXAMPLE = (
    "cargo test --locked -p horizon-core --example cloud_deploy -- tailnets:: "
    "tests::malformed_selection_arguments_never_create_cloud_paths"
)
PRODUCER = (
    "cargo test --locked -p horizon-cloud-worker --features trusted-tcp-producer "
    "trusted_tcp_producer::"
)
GATE = (
    "cargo test --locked -p horizon-cloud-worker --features trusted-exposure-gate "
    "exposure_gate::"
)


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
        self.assertEqual(libs.stdout.splitlines(), [DEFAULT_CLOUD, BUILD, WORKSPACE, EXAMPLE])

    def test_cloud_gateway_and_encoder_features_build_once_on_each_platform(self):
        for shard, runner_os in [("libs", "Linux"), ("libs", "macOS")]:
            with self.subTest(shard=shard, runner_os=runner_os):
                result = shard_commands(shard, runner_os)
                self.assertEqual(result.returncode, 0, result.stderr)
                lines = result.stdout.splitlines()
                self.assertEqual(lines.count(BUILD), 1)
                self.assertEqual(lines.count(WORKSPACE), 1)
                # A per-lane build of one package would rebuild every crate, so
                # no lane may name these features outside the workspace run.
                self.assertNotIn("-p horizon-core --lib --features", result.stdout)
                self.assertNotIn("-p horizon-cloud --features", result.stdout)
                self.assertNotIn("-p horizon-chromecast --features", result.stdout)

    def test_shipped_horizon_cloud_build_keeps_its_own_run(self):
        # One horizon-cloud test asserts the default-build behaviour that
        # exposure-firewall changes, so the feature build cannot replace it.
        for shard, runner_os in [("libs", "Linux"), ("libs", "macOS")]:
            with self.subTest(shard=shard, runner_os=runner_os):
                result = shard_commands(shard, runner_os)
                self.assertEqual(result.returncode, 0, result.stderr)
                lines = result.stdout.splitlines()
                self.assertEqual(lines.count(DEFAULT_CLOUD), 1)
                self.assertLess(lines.index(DEFAULT_CLOUD), lines.index(BUILD))

    def test_cloud_deploy_example_runs_once_on_each_platform(self):
        command = "cargo test --locked -p horizon-core --example cloud_deploy -- tailnets:: tests::malformed_selection_arguments_never_create_cloud_paths"
        for shard, runner_os in [("libs", "Linux"), ("libs", "macOS")]:
            with self.subTest(shard=shard, runner_os=runner_os):
                result = shard_commands(shard, runner_os)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.splitlines().count(command), 1)

    def test_enabled_producer_runs_only_in_linux_libs(self):
        command = "cargo test --locked -p horizon-cloud-worker --features trusted-tcp-producer trusted_tcp_producer::"
        for shard, runner_os, count in [
            ("libs", "Linux", 1), ("libs", "macOS", 0),
            ("ui", "Linux", 0), ("speech", "Linux", 0), ("speech", "macOS", 0),
        ]:
            with self.subTest(shard=shard, runner_os=runner_os):
                result = shard_commands(shard, runner_os)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.splitlines().count(command), count)

    def test_producer_failure_keeps_the_later_checks_and_fails_linux(self):
        env = os.environ.copy()
        env.update(HORIZON_CI_SHARD="libs", RUNNER_OS="Linux")
        env.pop("HORIZON_CI_SHARD_DRY_RUN", None)
        with tempfile.TemporaryDirectory() as bin_dir:
            log = Path(bin_dir) / "calls"
            fake = Path(bin_dir) / "cargo"
            fake.write_text(
                "#!/usr/bin/env bash\n"
                f"echo \"$*\" >> {log}\n"
                "[[ \"$*\" == *'--features trusted-tcp-producer trusted_tcp_producer::'* ]] && exit 101\n"
                "exit 0\n"
            )
            fake.chmod(0o755)
            env["PATH"] = f"{bin_dir}:{env['PATH']}"
            result = subprocess.run(["bash", str(SCRIPT)], cwd=ROOT, env=env,
                                    capture_output=True, text=True, check=False)
            calls = log.read_text().splitlines()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(calls.count("test --locked -p horizon-cloud-worker --features trusted-tcp-producer trusted_tcp_producer::"), 1)
        self.assertEqual(calls[-2:], [
            "test --locked -p horizon-cloud-worker --features trusted-exposure-gate exposure_gate::",
            "test --locked -p horizon-core --example cloud_deploy -- tailnets:: "
            "tests::malformed_selection_arguments_never_create_cloud_paths",
        ])

    def test_trusted_gate_runs_once_only_in_linux_libs(self):
        command = "cargo test --locked -p horizon-cloud-worker --features trusted-exposure-gate exposure_gate::"
        for shard, runner_os, count in [
            ("libs", "Linux", 1), ("libs", "macOS", 0),
            ("ui", "Linux", 0), ("speech", "Linux", 0), ("speech", "macOS", 0),
        ]:
            with self.subTest(shard=shard, runner_os=runner_os):
                result = shard_commands(shard, runner_os)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.splitlines().count(command), count)

    def test_trusted_gate_failure_preserves_later_checks_and_fails_linux(self):
        env = os.environ.copy()
        env.update(HORIZON_CI_SHARD="libs", RUNNER_OS="Linux")
        env.pop("HORIZON_CI_SHARD_DRY_RUN", None)
        with tempfile.TemporaryDirectory() as bin_dir:
            log = Path(bin_dir) / "calls"
            fake = Path(bin_dir) / "cargo"
            fake.write_text(
                "#!/usr/bin/env bash\n"
                f"echo \"$*\" >> {log}\n"
                "[[ \"$*\" == *'--features trusted-exposure-gate exposure_gate::'* ]] && exit 101\n"
                "exit 0\n"
            )
            fake.chmod(0o755)
            env["PATH"] = f"{bin_dir}:{env['PATH']}"
            result = subprocess.run(["bash", str(SCRIPT)], cwd=ROOT, env=env,
                                    capture_output=True, text=True, check=False)
            calls = log.read_text().splitlines()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(calls.count("test --locked -p horizon-cloud-worker --features trusted-exposure-gate exposure_gate::"), 1)
        self.assertEqual(calls[-1:], [
            "test --locked -p horizon-core --example cloud_deploy -- tailnets:: "
            "tests::malformed_selection_arguments_never_create_cloud_paths",
        ])

    def test_speech_shard_runs_the_speech_tests(self):
        for runner_os in ["Linux", "macOS"]:
            with self.subTest(runner_os=runner_os):
                result = shard_commands("speech", runner_os, ref="refs/pull/1/merge", cache_hit="true")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(
                    result.stdout.splitlines(),
                    ["cargo test --locked -p horizon-ui --features speech"],
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

    def test_macos_speech_shard_never_fills_the_entry_it_cannot_save(self):
        # The libs job saves the macOS entry, so a cold macOS speech job must not
        # pay for a workspace build that nothing keeps.
        cold = shard_commands("speech", "macOS", ref="refs/heads/main", cache_hit="false")
        self.assertEqual(cold.returncode, 0, cold.stderr)
        self.assertEqual(
            cold.stdout.splitlines(),
            ["cargo test --locked -p horizon-ui --features speech"],
        )

    def test_libs_failure_keeps_the_remaining_checks(self):
        for failing_flag in [
            "-p horizon-cloud",
            f"--exclude horizon-ui --features {FEATURES}",
            "--example cloud_deploy",
        ]:
            with self.subTest(failing_flag=failing_flag):
                env = os.environ.copy()
                env.update(HORIZON_CI_SHARD="libs", RUNNER_OS="macOS")
                env.pop("HORIZON_CI_SHARD_DRY_RUN", None)
                with tempfile.TemporaryDirectory() as bin_dir:
                    log = Path(bin_dir) / "calls"
                    fake = Path(bin_dir) / "cargo"
                    # A failing libs command cannot hide the remaining checks.
                    fake.write_text(
                        "#!/usr/bin/env bash\n"
                        f"echo \"$*\" >> {log}\n"
                        f"[[ \"$*\" == *'{failing_flag}'* ]] && exit 101\n"
                        "exit 0\n"
                    )
                    fake.chmod(0o755)
                    env["PATH"] = f"{bin_dir}:{env['PATH']}"
                    result = subprocess.run(["bash", str(SCRIPT)], cwd=ROOT, env=env,
                                            capture_output=True, text=True, check=False)
                    calls = log.read_text().splitlines()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(
                    calls,
                    [
                        DEFAULT_CLOUD.removeprefix("cargo "),
                        BUILD.removeprefix("cargo "),
                        WORKSPACE.removeprefix("cargo "),
                        EXAMPLE.removeprefix("cargo "),
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
        entries = [
            dict(re.findall(r"^\s+-?\s*(\w+): (.+)$", entry, re.M))
            for entry in re.split(r"^\s+- os: ", rust.split("include:", 1)[1].split("steps:", 1)[0], flags=re.M)[1:]
        ]
        shards = {(entry["name"], entry["shard"]) for entry in entries}
        self.assertEqual(
            shards,
            {
                ("Linux x64", "ui"),
                ("Linux x64", "libs"),
                ("Linux x64", "speech"),
                ("macOS arm64", "libs"),
                ("macOS arm64", "speech"),
            },
        )
        # Exactly one job per OS saves the entry the others restore.
        savers = {(entry["name"], entry["shard"]) for entry in entries if entry.get("save_cache") == "true"}
        self.assertEqual(savers, {("Linux x64", "speech"), ("macOS arm64", "libs")})
        self.assertIn("cache_key: linux-test", rust)
        self.assertIn("cache_key: macos-test", rust)
        self.assertIn("key: ${{ matrix.cache_key }}", rust)
        self.assertNotIn("matrix.shard", rust.split("save-if:", 1)[0].split("key:", 1)[-1])
        self.assertIn("save-if: ${{ github.ref == 'refs/heads/main' && matrix.save_cache == true }}", rust)
        self.assertIn("timeout-minutes: ${{ matrix.timeout_minutes || 35 }}", rust)
        self.assertIn("if: ${{ !cancelled() && startsWith(matrix.os, 'macos-') && matrix.shard == 'speech' }}", rust)
        self.assertEqual(rust.count("bash scripts/ci-cargo-test-shard.sh"), 1)
        self.assertNotIn("cargo test --locked --workspace\n", rust)

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
