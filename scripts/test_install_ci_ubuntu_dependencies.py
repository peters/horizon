#!/usr/bin/env python3
"""Exercise the runner-only installer without sudo, APT, network, or system writes."""

import os
from pathlib import Path
import re
import shlex
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent
INSTALLER = ROOT / "scripts/install-ci-ubuntu-dependencies.sh"
SOURCES = "/etc/apt/sources.list.d/ubuntu.sources"
OPTIONS = [
    "-o", f"Dir::Etc::sourcelist={SOURCES}",
    "-o", "Dir::Etc::sourceparts=/dev/null",
    "-o", "APT::Get::List-Cleanup=0",
]
FAKE_SUDO = r'''#!/bin/bash
printf '%s\0' "$@" >> "$CI_TEST_COMMAND_LOG"
printf '\0' >> "$CI_TEST_COMMAND_LOG"
[[ "$1" == -n && "$2" == apt-get ]] || exit 97
for argument in "$@"; do
  case "$argument" in
    update) exit "$CI_TEST_UPDATE_STATUS" ;;
    install) exit "$CI_TEST_INSTALL_STATUS" ;;
  esac
done
exit 98
'''
SOURCED_RUN = r'''
source "$1"
shift
require_ubuntu_ci_sources() {
  [[ "$#" == 1 && "$1" == /etc/apt/sources.list.d/ubuntu.sources ]] || return 99
  return "$CI_TEST_SOURCE_STATUS"
}
'''


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="horizon-ci-apt-test-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.commands = self.root / "commands"
        fake_sudo = self.root / "sudo"
        fake_sudo.write_text(FAKE_SUDO)
        fake_sudo.chmod(0o700)
        # No host command can be found through PATH, including sudo or apt-get.
        self.env = {
            "PATH": str(self.root),
            "GITHUB_ACTIONS": "true",
            "RUNNER_OS": "Linux",
            "CI_TEST_COMMAND_LOG": str(self.commands),
            "CI_TEST_SOURCE_STATUS": "0",
            "CI_TEST_UPDATE_STATUS": "0",
            "CI_TEST_INSTALL_STATUS": "0",
        }

    def run_installer(self, *packages, cli=False, conditional=False):
        invocation = 'install_ci_ubuntu_dependencies "$@"'
        if conditional:
            invocation = 'if install_ci_ubuntu_dependencies "$@"; then exit 0; else exit "$?"; fi'
        command = ["/bin/bash", str(INSTALLER)] if cli else [
            "/bin/bash", "-c", SOURCED_RUN + invocation, "fixture", str(INSTALLER)
        ]
        return subprocess.run(command + list(packages), env=self.env, capture_output=True, timeout=5)

    def calls(self):
        if not self.commands.exists():
            return []
        return [
            record.decode().split("\0")
            for record in self.commands.read_bytes().split(b"\0\0") if record
        ]

    def test_update_and_install_share_exact_source_options_and_literal_packages(self):
        packages = ["libasound2-dev", "pkg-config"]
        result = self.run_installer(*packages)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls(), [
            ["-n", "apt-get", *OPTIONS, "-o", "APT::Update::Error-Mode=any", "update"],
            ["-n", "apt-get", *OPTIONS, "install", "-y", "--", *packages],
        ])

    def test_update_failure_preserves_exit_status_and_never_installs(self):
        self.env["CI_TEST_UPDATE_STATUS"] = "100"
        result = self.run_installer("pkg-config")
        self.assertEqual(result.returncode, 100)
        self.assertEqual(self.calls(), [
            ["-n", "apt-get", *OPTIONS, "-o", "APT::Update::Error-Mode=any", "update"]
        ])

    def test_install_failure_is_not_hidden_or_retried(self):
        self.env["CI_TEST_INSTALL_STATUS"] = "42"
        result = self.run_installer("pkg-config")
        self.assertEqual(result.returncode, 42)
        self.assertEqual(len(self.calls()), 2)

    def test_conditional_function_context_preserves_failures_without_errexit(self):
        for update, install, expected, count in [(100, 0, 100, 1), (0, 42, 42, 2)]:
            with self.subTest(update=update, install=install):
                self.commands.unlink(missing_ok=True)
                self.env.update(CI_TEST_UPDATE_STATUS=str(update), CI_TEST_INSTALL_STATUS=str(install))
                result = self.run_installer("pkg-config", conditional=True)
                self.assertEqual(result.returncode, expected)
                self.assertEqual(len(self.calls()), count)

    def test_normal_cli_propagates_failures_when_the_runner_source_is_available(self):
        source = Path(SOURCES)
        if not source.is_file() or not os.access(source, os.R_OK) or source.stat().st_size == 0:
            self.skipTest("normal CLI requires the supported Ubuntu runner source layout")
        for update, install, expected, count in [(100, 0, 100, 1), (0, 42, 42, 2)]:
            with self.subTest(update=update, install=install):
                self.commands.unlink(missing_ok=True)
                self.env.update(CI_TEST_UPDATE_STATUS=str(update), CI_TEST_INSTALL_STATUS=str(install))
                result = self.run_installer("pkg-config", cli=True)
                self.assertEqual(result.returncode, expected)
                self.assertEqual(len(self.calls()), count)

    def test_missing_source_refuses_before_sudo(self):
        self.env["CI_TEST_SOURCE_STATUS"] = "1"
        result = self.run_installer("pkg-config")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(self.calls(), [])

    def test_real_source_check_requires_a_nonempty_readable_regular_file(self):
        empty = self.root / "empty.sources"
        empty.touch()
        source = self.root / "ubuntu.sources"
        content = "Types: deb\nURIs: https://example.invalid/ubuntu\n"
        source.write_text(content)
        for path, expected in [(self.root / "missing", 1), (self.root, 1), (empty, 1), (source, 0)]:
            with self.subTest(path=path.name):
                result = subprocess.run([
                    "/bin/bash", "-c", 'source "$1"; require_ubuntu_ci_sources "$2"',
                    "fixture", str(INSTALLER), str(path),
                ], env=self.env, capture_output=True, timeout=5)
                self.assertEqual(result.returncode, expected, result.stderr)
        source.chmod(0o000)
        expected = 0 if os.access(source, os.R_OK) else 1
        result = subprocess.run([
            "/bin/bash", "-c", 'source "$1"; require_ubuntu_ci_sources "$2"',
            "fixture", str(INSTALLER), str(source),
        ], env=self.env, capture_output=True, timeout=5)
        self.assertEqual(result.returncode, expected, result.stderr)
        source.chmod(0o600)
        self.assertEqual(source.read_text(), content)
        self.assertEqual(self.calls(), [])

    def test_normal_cli_rejects_nonrunner_and_nonlinux_contexts(self):
        for actions, operating_system in [("", "Linux"), ("false", "Linux"), ("true", "macOS")]:
            with self.subTest(actions=actions, operating_system=operating_system):
                self.env.update(GITHUB_ACTIONS=actions, RUNNER_OS=operating_system)
                result = self.run_installer("pkg-config", cli=True)
                self.assertEqual(result.returncode, 2)
                self.assertIn(b"only for Linux GitHub Actions runners", result.stderr)
                self.assertEqual(self.calls(), [])

    def test_normal_cli_rejects_options_paths_and_shell_text_before_sudo(self):
        for packages in [(), ("--allow-unauthenticated",), ("-o", "Dir::Etc::sourceparts=/tmp"),
                         ("./local.deb",), ("liba;echo bad",), ("liba\nlibb",), ("liba libb",),
                         ("bash-",), ("bash+",), ("lib.a",), ("lib.*",), (".",), ("g++",)]:
            with self.subTest(packages=packages):
                result = self.run_installer(*packages, cli=True)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(self.calls(), [])


class WorkflowTests(unittest.TestCase):
    def test_only_the_five_ubuntu_dependency_sites_use_the_helper_with_unchanged_packages(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        jobs = dict(re.findall(r"^  ([\w-]+):\n(.*?)(?=^  [\w-]+:|\Z)", workflow, re.M | re.S))
        prefix = "bash scripts/install-ci-ubuntu-dependencies.sh "
        expected = {
            "remote-panel-sessions": ["bison", "libevent-dev", "libncurses-dev", "pkg-config"],
            "clippy": ["libasound2-dev", "pkg-config"],
            "clippy-strict": ["libasound2-dev", "pkg-config"],
            "clippy-pedantic": ["libasound2-dev", "pkg-config"],
            "rust-test": ["libasound2-dev", "pkg-config"],
        }
        self.assertEqual(workflow.count(prefix), len(expected))
        self.assertNotIn("apt-get", workflow)
        for name, packages in expected.items():
            with self.subTest(job=name):
                commands = re.findall(r"run: " + re.escape(prefix) + r"([^\n]+)", jobs[name])
                self.assertEqual([shlex.split(command) for command in commands], [packages])
                if name == "rust-test":
                    self.assertIn("- if: matrix.os == 'ubuntu-latest'\n        run: " + prefix, jobs[name])
                else:
                    self.assertIn("runs-on: ubuntu-latest", jobs[name])
        self.assertIn(
            "python3 -B scripts/test_install_ci_ubuntu_dependencies.py -v", jobs["maintainability"]
        )


if __name__ == "__main__":
    unittest.main()
