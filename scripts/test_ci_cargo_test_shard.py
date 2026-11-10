#!/usr/bin/env python3
"""The platform test shards run ui and libs on Linux and on macOS."""

import os
import re
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "ci-cargo-test-shard.sh"
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"

UI = "cargo test --locked -p horizon-ui"
# nextest runs the workspace selection and keeps its libtest-json stream for the
# duration gate; libtest keeps the doctests, which nextest cannot run.
STREAM = "/tmp/nextest-libs.json"
NEXTEST = ("cargo nextest run --locked --workspace --exclude horizon-ui "
           f"--message-format libtest-json-plus > {STREAM}")
DOCTESTS = "cargo test --locked --workspace --exclude horizon-ui --doc"
NEXTEST_ARGS = NEXTEST.split(" > ")[0]
DURATION_GATE = f"python3 -B scripts/check-test-durations.py --stream {STREAM}"


def shard_commands(shard, runner_os):
    env = os.environ.copy()
    env["HORIZON_CI_SHARD"] = shard
    env["HORIZON_CI_SHARD_DRY_RUN"] = "1"
    env["RUNNER_OS"] = runner_os
    # The dry run names the stream path, so it must not follow the caller.
    env.pop("RUNNER_TEMP", None)
    return subprocess.run(
        ["bash", str(SCRIPT)],
        cwd=ROOT,
        env=env,
        check=False,
        capture_output=True,
        text=True,
    )


class ShardCommandTests(unittest.TestCase):
    def test_ui_and_libs_select_disjoint_packages(self):
        for runner_os in ["Linux", "macOS"]:
            with self.subTest(runner_os=runner_os):
                ui = shard_commands("ui", runner_os)
                libs = shard_commands("libs", runner_os)
                self.assertEqual(ui.returncode, 0, ui.stderr)
                self.assertEqual(libs.returncode, 0, libs.stderr)
                self.assertEqual(ui.stdout.splitlines(), [UI])
                self.assertEqual(libs.stdout.splitlines(), [NEXTEST, DOCTESTS, DURATION_GATE])

    def test_the_workspace_selection_carries_no_opt_in_feature(self):
        for runner_os in ["Linux", "macOS"]:
            with self.subTest(runner_os=runner_os):
                for shard in ["ui", "libs"]:
                    result = shard_commands(shard, runner_os)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    # No opt-in cargo feature remains in the test matrix, and a
                    # per-lane feature build would rebuild every crate.
                    self.assertNotIn("--features", result.stdout)

    def test_libs_failure_keeps_the_remaining_checks(self):
        cases = [
            ("--exclude horizon-ui --message-format", [NEXTEST_ARGS, DOCTESTS]),
            ("--doc", [NEXTEST_ARGS, DOCTESTS]),
        ]
        for failing_flag, expected in cases:
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
                    [command.removeprefix("cargo ") for command in expected],
                )

    def test_ui_failure_fails_the_shard(self):
        env = os.environ.copy()
        env.update(HORIZON_CI_SHARD="ui", RUNNER_OS="Linux")
        env.pop("HORIZON_CI_SHARD_DRY_RUN", None)
        with tempfile.TemporaryDirectory() as bin_dir:
            fake = Path(bin_dir) / "cargo"
            fake.write_text("#!/usr/bin/env bash\nexit 101\n")
            fake.chmod(0o755)
            env["PATH"] = f"{bin_dir}:{env['PATH']}"
            result = subprocess.run(["bash", str(SCRIPT)], cwd=ROOT, env=env,
                                    capture_output=True, text=True, check=False)
        self.assertNotEqual(result.returncode, 0)

    def test_unknown_shard_fails(self):
        # The removed shard names are unknown, not silently successful.
        for shard in ["every", "speech", "libs-extra"]:
            with self.subTest(shard=shard):
                result = shard_commands(shard, "Linux")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Unknown CI test shard", result.stderr)


class WorkflowShardTests(unittest.TestCase):
    def test_platform_jobs_save_one_cache_entry_per_cache_key(self):
        workflow = WORKFLOW.read_text()
        jobs = dict(re.findall(r"^  ([\w-]+):\n(.*?)(?=^  [\w-]+:|\Z)", workflow, re.M | re.S))
        rust = jobs["rust-test"]
        entries = [
            dict(re.findall(r"^\s+-?\s*(\w+): (.+)$", entry, re.M))
            for entry in re.split(r"^\s+- os: ", rust.split("include:", 1)[1].split("steps:", 1)[0], flags=re.M)[1:]
        ]
        self.assertEqual(
            {(entry["name"], entry["shard"]) for entry in entries},
            {
                ("Linux x64", "ui"),
                ("Linux x64", "libs"),
                ("macOS arm64", "libs"),
                ("macOS arm64", "ui"),
            },
        )
        # One key per shard, because cargo resolves features per package
        # selection: a key shared by two selections is a full match that still
        # rebuilds. Compare the parsed values, because `linux-test` is a prefix
        # of `linux-test-libs` and a substring check cannot tell them apart.
        self.assertEqual(
            {(entry["shard"], entry["cache_key"]) for entry in entries},
            {
                ("ui", "linux-test"),
                ("libs", "linux-test-libs"),
                ("libs", "macos-test-libs"),
                ("ui", "macos-test-ui"),
            },
        )
        # Every shard saves its own key, so each key holds one entry.
        self.assertEqual(
            len({entry["cache_key"] for entry in entries}),
            len(entries),
            "every cache key needs exactly one shard",
        )
        self.assertIn("key: ${{ matrix.cache_key }}", rust)
        self.assertNotIn("matrix.shard", rust.split("save-if:", 1)[0].split("key:", 1)[-1])
        self.assertIn("save-if: ${{ github.ref == 'refs/heads/main' }}", rust)
        self.assertNotIn("matrix.save_cache", rust)
        self.assertIn("timeout-minutes: ${{ matrix.timeout_minutes || 35 }}", rust)
        # The shard timeouts of the target design: Linux keeps the default, the
        # macOS libs shard gets cold-cache headroom and the macOS ui shard less.
        self.assertEqual(
            {entry["shard"] + "-" + entry["name"]: entry.get("timeout_minutes") for entry in entries},
            {"ui-Linux x64": None, "libs-Linux x64": None,
             "libs-macOS arm64": "60", "ui-macOS arm64": "45"},
        )
        self.assertEqual(rust.count("bash scripts/ci-cargo-test-shard.sh"), 1)
        self.assertNotIn("cargo test --locked --workspace\n", rust)
        # The per-pull-request skip flag went with the removed lanes.
        self.assertNotIn("HORIZON_CI_SKIP_", rust)
        self.assertNotIn("HORIZON_CI_CACHE_HIT", rust)

    def test_only_the_libs_shards_install_and_run_nextest(self):
        workflow = WORKFLOW.read_text()
        self.assertIn(
            "        if: ${{ matrix.shard == 'libs' }}\n"
            "        uses: taiki-e/install-action@nextest\n",
            workflow,
        )
        self.assertEqual(workflow.count("taiki-e/install-action@nextest"), 1)
        # Only the libs shard's script path may name nextest, so a leg that does
        # not install it cannot run it.
        self.assertIn("nextest run", SCRIPT.read_text())

    def test_the_linux_libs_shard_keeps_its_loopback_ssh_transport(self):
        rust = jobs_of_workflow()["rust-test"]
        condition = "!env.LOCALCI && matrix.os == 'ubuntu-latest' && matrix.shard == 'libs'"
        for step in ["bash scripts/install-ci-ubuntu-dependencies.sh openssh-server\n",
                     "bash scripts/ci-ssh-transport.sh\n"]:
            with self.subTest(step=step.split()[-1]):
                start = rust.index(step)
                self.assertIn(condition, rust[start - rust[:start].rfind("      - "):start])

    def test_only_the_libs_shard_publishes_its_durations_on_main(self):
        rust = jobs_of_workflow()["rust-test"]
        self.assertIn(
            "        if: ${{ !cancelled() && github.ref == 'refs/heads/main' "
            "&& matrix.shard == 'libs' }}\n",
            rust,
        )
        self.assertIn('--stream "$RUNNER_TEMP/nextest-libs.json"', rust)


def jobs_of_workflow():
    workflow = WORKFLOW.read_text()
    return dict(re.findall(r"^  ([\w-]+):\n(.*?)(?=^  [\w-]+:|\Z)", workflow, re.M | re.S))


if __name__ == "__main__":
    unittest.main()
