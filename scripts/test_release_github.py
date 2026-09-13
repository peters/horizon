#!/usr/bin/env python3
"""Exercise draft-first GitHub Release orchestration without calling GitHub."""

import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts/release-github.sh"
COMMIT = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
OTHER_COMMIT = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
TAG = "v0.3.0-alpha.1"
STABLE_TAG = "v0.3.0"
REPO = "peters/horizon"
PRERELEASE_ASSETS = [
    "horizon-linux-x64.tar.gz",
    "horizon-osx-arm64.tar.gz",
    "horizon-osx-x64.tar.gz",
    "horizon-windows-x64.exe",
    "SHA256SUMS.txt",
]
STABLE_ASSETS = PRERELEASE_ASSETS + [
    "horizon-installer-linux-x64.bin",
    "horizon-installer-osx-arm64.bin",
    "horizon-installer-osx-x64.bin",
    "horizon-installer-win-x64.exe",
]

FAKE_GH = r'''#!/usr/bin/env python3
import hashlib
import json
import os
from pathlib import Path
import sys

STATE_PATH = Path(os.environ["HORIZON_RELEASE_TEST_STATE"])


def load_state():
    return json.loads(STATE_PATH.read_text())


def save_state(state):
    STATE_PATH.write_text(json.dumps(state, indent=2, sort_keys=True))


def record(state, argv):
    state.setdefault("commands", []).append(list(argv))


def parse_flags(args):
    flags = {}
    positional = []
    index = 0
    while index < len(args):
        arg = args[index]
        if arg == "--repo":
            flags["repo"] = args[index + 1]
            index += 2
        elif arg == "--title":
            flags["title"] = args[index + 1]
            index += 2
        elif arg == "--target":
            flags["target"] = args[index + 1]
            index += 2
        elif arg == "--notes":
            flags["notes"] = args[index + 1]
            index += 2
        elif arg == "--notes-file":
            flags["notes"] = Path(args[index + 1]).read_text()
            index += 2
        elif arg == "--json":
            flags["json"] = args[index + 1]
            index += 2
        elif arg == "--draft":
            flags["draft"] = True
            index += 1
        elif arg.startswith("--draft="):
            flags["draft"] = arg.split("=", 1)[1].lower() == "true"
            index += 1
        elif arg == "--prerelease":
            flags["prerelease"] = True
            index += 1
        elif arg.startswith("--prerelease="):
            flags["prerelease"] = arg.split("=", 1)[1].lower() == "true"
            index += 1
        elif arg == "--verify-tag":
            flags["verify_tag"] = True
            index += 1
        elif arg in {"--yes", "-y"}:
            flags["yes"] = True
            index += 1
        else:
            positional.append(arg)
            index += 1
    return flags, positional


def digest_for(path):
    return "sha256:" + hashlib.sha256(Path(path).read_bytes()).hexdigest()


def require_release(state, tag):
    release = state.get("releases", {}).get(tag)
    if release is None:
        sys.stderr.write("release not found\n")
        sys.exit(1)
    return release


def main(argv):
    state = load_state()
    record(state, argv[1:])
    if argv[1:3] != ["release", "view"]:
        pass
    flags, positional = parse_flags(argv[3:] if argv[1] == "release" else argv[1:])
    if argv[1] != "release":
        sys.stderr.write("unexpected command\n")
        save_state(state)
        sys.exit(99)

    sub = argv[2]
    if sub == "view":
        tag = positional[0]
        release = require_release(state, tag)
        save_state(state)
        json.dump(release, sys.stdout)
        sys.stdout.write("\n")
        return
    if sub == "create":
        tag = positional[0]
        if flags.get("verify_tag") and tag not in state.get("tags", {}):
            sys.stderr.write(f"tag {tag} not found\n")
            save_state(state)
            sys.exit(1)
        if tag in state.setdefault("releases", {}):
            sys.stderr.write("release already exists\n")
            save_state(state)
            sys.exit(1)
        state["releases"][tag] = {
            "tagName": tag,
            "isDraft": bool(flags.get("draft", False)),
            "isPrerelease": bool(flags.get("prerelease", False)),
            "isImmutable": False,
            "body": flags.get("notes", ""),
            "targetCommitish": flags.get("target", ""),
            "assets": [],
        }
        save_state(state)
        return
    if sub == "edit":
        tag = positional[0]
        release = require_release(state, tag)
        if "draft" in flags:
            release["isDraft"] = flags["draft"]
        if "prerelease" in flags:
            release["isPrerelease"] = flags["prerelease"]
        if "notes" in flags:
            release["body"] = flags["notes"]
        save_state(state)
        return
    if sub == "upload":
        tag = positional[0]
        files = positional[1:]
        release = require_release(state, tag)
        fail_after = int(os.environ.get("HORIZON_RELEASE_TEST_FAIL_UPLOAD_AFTER") or "0")
        for path in files:
            name = Path(path).name
            state["upload_attempts"] = state.get("upload_attempts", 0) + 1
            if fail_after and state["upload_attempts"] >= fail_after:
                save_state(state)
                sys.stderr.write("injected upload failure\n")
                sys.exit(2)
            if any(asset["name"] == name for asset in release["assets"]):
                sys.stderr.write(f"{name} already exists\n")
                save_state(state)
                sys.exit(1)
            payload = Path(path).read_bytes()
            release["assets"].append(
                {
                    "name": name,
                    "size": len(payload),
                    "digest": digest_for(path),
                }
            )
        save_state(state)
        return
    if sub == "delete-asset":
        tag = positional[0]
        name = positional[1]
        release = require_release(state, tag)
        release["assets"] = [asset for asset in release["assets"] if asset["name"] != name]
        save_state(state)
        return

    sys.stderr.write(f"unsupported gh release subcommand: {sub}\n")
    save_state(state)
    sys.exit(99)


if __name__ == "__main__":
    main(sys.argv)
'''

FAKE_GIT = r'''#!/bin/bash
printf 'unexpected git invocation: %s\n' "$*" >&2
exit 99
'''


class ReleaseGithubTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="horizon-release-github-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.state_path = self.root / "state.json"
        self.assets = self.root / "assets"
        self.assets.mkdir()
        self.state_path.write_text(json.dumps({
            "releases": {},
            "tags": {TAG: COMMIT, STABLE_TAG: COMMIT},
            "commands": [],
            "upload_attempts": 0,
        }))
        gh = self.root / "gh"
        gh.write_text(FAKE_GH)
        gh.chmod(gh.stat().st_mode | stat.S_IEXEC)
        git = self.root / "git"
        git.write_text(FAKE_GIT)
        git.chmod(git.stat().st_mode | stat.S_IEXEC)
        self.env = os.environ.copy()
        self.env["PATH"] = str(self.root) + os.pathsep + self.env.get("PATH", "")
        self.env["HORIZON_RELEASE_TEST_STATE"] = str(self.state_path)
        self.env.pop("HORIZON_RELEASE_TEST_FAIL_UPLOAD_AFTER", None)

    def state(self):
        return json.loads(self.state_path.read_text())

    def commands(self):
        return self.state()["commands"]

    def release(self, tag=TAG):
        return self.state()["releases"][tag]

    def write_assets(self, names, directory=None):
        directory = directory or self.assets
        for name in names:
            path = directory / name
            path.write_bytes(f"{name}:{COMMIT}\n".encode())
        return directory

    def run_script(self, *args, expect=0):
        result = subprocess.run(
            ["bash", str(SCRIPT), *args],
            env=self.env,
            capture_output=True,
            text=True,
            timeout=10,
        )
        if result.returncode != expect:
            self.fail(
                f"exit {result.returncode} != {expect}\n"
                f"stdout:\n{result.stdout}\n"
                f"stderr:\n{result.stderr}"
            )
        return result

    def ensure_pending(self, tag=TAG, commit=COMMIT, prerelease="true", expect=0):
        return self.run_script(
            "ensure-pending",
            "--repo", REPO,
            "--tag", tag,
            "--commit", commit,
            "--prerelease", prerelease,
            expect=expect,
        )

    def upload_assets(self, tag=TAG, commit=COMMIT, directory=None, expect=0):
        directory = directory or self.assets
        return self.run_script(
            "upload-assets",
            "--repo", REPO,
            "--tag", tag,
            "--commit", commit,
            "--dir", str(directory),
            expect=expect,
        )

    def publish(self, tag=TAG, commit=COMMIT, prerelease="true", expect=0):
        return self.run_script(
            "publish",
            "--repo", REPO,
            "--tag", tag,
            "--commit", commit,
            "--prerelease", prerelease,
            expect=expect,
        )

    def asset_names(self, tag=TAG):
        return [asset["name"] for asset in self.release(tag)["assets"]]

    def test_ensure_pending_creates_draft_for_existing_tag(self):
        result = self.ensure_pending()
        release = self.release()
        self.assertTrue(release["isDraft"])
        self.assertTrue(release["isPrerelease"])
        self.assertIn(COMMIT, release["body"])
        self.assertEqual(release["targetCommitish"], COMMIT)
        self.assertIn("Created draft GitHub Release", result.stdout)
        self.assertTrue(any(command[:2] == ["release", "create"] for command in self.commands()))
        self.assertFalse(any("--draft=false" in command for command in self.commands()))

    def test_ensure_pending_requires_existing_tag(self):
        result = self.ensure_pending(tag="v0.9.9-alpha.1", expect=1)
        self.assertIn("not found", result.stderr)
        self.assertNotIn("v0.9.9-alpha.1", self.state()["releases"])

    def test_ensure_pending_converts_incomplete_published_release_to_draft(self):
        self.ensure_pending()
        state = self.state()
        state["releases"][TAG]["isDraft"] = False
        self.state_path.write_text(json.dumps(state, indent=2))
        result = self.ensure_pending()
        self.assertTrue(self.release()["isDraft"])
        self.assertIn("back to a draft", result.stdout)
        self.assertTrue(
            any(command[:2] == ["release", "edit"] and "--draft" in command for command in self.commands())
        )

    def test_ensure_pending_leaves_complete_published_release_public(self):
        self.write_assets(PRERELEASE_ASSETS)
        self.ensure_pending()
        self.upload_assets()
        self.publish()
        self.assertFalse(self.release()["isDraft"])
        result = self.ensure_pending()
        self.assertFalse(self.release()["isDraft"])
        self.assertIn("already published", result.stdout)

    def test_ensure_pending_rejects_a_different_recorded_commit(self):
        self.ensure_pending()
        result = self.ensure_pending(commit=OTHER_COMMIT, expect=1)
        self.assertIn(COMMIT, result.stderr)
        self.assertIn(OTHER_COMMIT, result.stderr)
        self.assertTrue(self.release()["isDraft"])

    def test_upload_skips_matching_digest_and_replaces_changed_bytes(self):
        self.write_assets(PRERELEASE_ASSETS)
        self.ensure_pending()
        self.upload_assets()
        first_uploads = sum(1 for command in self.commands() if command[:2] == ["release", "upload"])
        self.upload_assets()
        second_uploads = sum(1 for command in self.commands() if command[:2] == ["release", "upload"])
        self.assertEqual(first_uploads, len(PRERELEASE_ASSETS))
        self.assertEqual(second_uploads, first_uploads)

        changed = self.assets / "horizon-linux-x64.tar.gz"
        changed.write_bytes(b"rebuilt-bytes\n")
        result = self.upload_assets()
        self.assertIn("Replacing horizon-linux-x64.tar.gz", result.stdout)
        self.assertTrue(any(command[:2] == ["release", "delete-asset"] for command in self.commands()))
        linux = next(asset for asset in self.release()["assets"] if asset["name"] == "horizon-linux-x64.tar.gz")
        self.assertEqual(linux["digest"], "sha256:" + hashlib.sha256(b"rebuilt-bytes\n").hexdigest())
        self.assertTrue(self.release()["isDraft"])

    def test_failure_injection_keeps_release_pending_then_resume_publishes(self):
        self.write_assets(PRERELEASE_ASSETS)
        self.ensure_pending()
        self.env["HORIZON_RELEASE_TEST_FAIL_UPLOAD_AFTER"] = "2"
        failed = self.upload_assets(expect=2)
        self.assertIn("injected upload failure", failed.stderr)
        self.assertTrue(self.release()["isDraft"])
        self.assertEqual(self.asset_names(), ["horizon-linux-x64.tar.gz"])
        self.assertFalse(any("--draft=false" in command for command in self.commands()))

        refused = self.publish(expect=1)
        self.assertIn("Refusing to publish", refused.stderr)
        self.assertTrue(self.release()["isDraft"])

        del self.env["HORIZON_RELEASE_TEST_FAIL_UPLOAD_AFTER"]
        resumed = self.upload_assets()
        self.assertIn("Skipping horizon-linux-x64.tar.gz", resumed.stdout)
        self.assertCountEqual(self.asset_names(), PRERELEASE_ASSETS)
        published = self.publish()
        self.assertFalse(self.release()["isDraft"])
        self.assertIn("Published GitHub Release", published.stdout)
        self.assertTrue(any("--draft=false" in command for command in self.commands()))
        self.assertFalse(any(command[:1] == ["tag"] for command in self.commands()))

    def test_publish_requires_stable_installers_before_going_public(self):
        self.write_assets(PRERELEASE_ASSETS)
        self.ensure_pending(tag=STABLE_TAG, prerelease="false")
        self.upload_assets(tag=STABLE_TAG)
        result = self.publish(tag=STABLE_TAG, prerelease="false", expect=1)
        self.assertIn("horizon-installer-linux-x64.bin", result.stderr)
        self.assertTrue(self.release(STABLE_TAG)["isDraft"])

        self.write_assets(STABLE_ASSETS)
        self.upload_assets(tag=STABLE_TAG)
        self.publish(tag=STABLE_TAG, prerelease="false")
        self.assertFalse(self.release(STABLE_TAG)["isDraft"])
        self.assertCountEqual(self.asset_names(STABLE_TAG), STABLE_ASSETS)

    def test_script_does_not_invoke_git(self):
        self.write_assets(PRERELEASE_ASSETS)
        self.ensure_pending()
        self.upload_assets()
        self.publish()
        git_log = self.root / "git.log"
        self.assertFalse(git_log.exists())


if __name__ == "__main__":
    unittest.main()
