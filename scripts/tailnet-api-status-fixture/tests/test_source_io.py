"""Synthetic FD/copy and Cargo-input regressions; no Cargo, Docker or secrets."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def module(name):
    spec = importlib.util.spec_from_file_location("source_test_" + name, ROOT / (name + ".py"))
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


def sha(value):
    return hashlib.sha256(value).hexdigest()


class SourceIo(unittest.TestCase):
    def test_artifacts_are_exclusive_verified_copies_with_separate_large_bound(self):
        run = module("run")
        self.assertGreaterEqual(run.MAX_ARTIFACT_BYTES, 443 * 1024 * 1024)
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            target = root / "frozen"
            target.mkdir(mode=0o700)
            manifest = root / "manifest.json"
            entries = {}
            for key in ["horizon", "cloud_deploy", "horizon_device"]:
                path = root / key
                path.write_bytes(key.encode())
                entries[key] = {"path": str(path), "sha256": sha(key.encode())}
            manifest.write_text(json.dumps(entries))
            frozen = run.freeze_artifacts(manifest, target)
            for key, item in frozen.items():
                path = Path(item["path"])
                self.assertEqual(sha(path.read_bytes()), entries[key]["sha256"])
                self.assertEqual(path.stat().st_mode & 0o777, 0o500)
            with self.assertRaises(FileExistsError):
                run.freeze_artifacts(manifest, target)

    def test_public_input_rejects_leaf_parent_hardlink_fifo_and_oversize(self):
        io = module("safe_io")
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source = root / "source"
            source.write_bytes(b"not copied")
            (root / "leaf").symlink_to(source)
            (root / "parent").symlink_to(root, target_is_directory=True)
            os.link(source, root / "hard")
            os.mkfifo(root / "fifo")
            for path, limit in [(root / "leaf", 100), (root / "parent/source", 100),
                                (root / "hard", 100), (root / "fifo", 100), (source, 1)]:
                with self.subTest(path=path), self.assertRaises(RuntimeError):
                    io.read_public_bytes(path, limit)
            self.assertEqual(source.read_bytes(), b"not copied")

    def test_bad_manifest_hash_refuses_before_creating_target(self):
        io = module("safe_io")
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source, target = root / "source", root / "frozen"
            source.write_bytes(b"synthetic")
            with self.assertRaisesRegex(RuntimeError, "hash mismatch"):
                io.copy_verified(source, target, 100, expected="0" * 64)
            self.assertFalse(target.exists())

    def test_hash_and_copy_open_source_only_once(self):
        io = module("safe_io")
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source, target = root / "source", root / "frozen"
            source.write_bytes(b"same fd")
            real_open = os.open
            opened = []
            def observe(path, flags, *args, **kwargs):
                if path == "source":
                    opened.append(flags)
                return real_open(path, flags, *args, **kwargs)
            with patch.object(io.os, "open", side_effect=observe):
                self.assertEqual(io.copy_verified(source, target, 100, expected=sha(b"same fd")), sha(b"same fd"))
            self.assertEqual(len(opened), 1)
            self.assertTrue(opened[0] & os.O_NOFOLLOW)
            self.assertTrue(opened[0] & os.O_NONBLOCK)
            self.assertEqual(target.read_bytes(), b"same fd")

    def test_path_replaced_after_hash_cannot_copy_replacement_bytes(self):
        io = module("safe_io")
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source, target = root / "source", root / "frozen"
            source.write_bytes(b"manifest bytes")
            real_seek = os.lseek
            def replace(descriptor, offset, how):
                source.rename(root / "original")
                source.write_bytes(b"replacement bytes")
                return real_seek(descriptor, offset, how)
            with patch.object(io.os, "lseek", side_effect=replace), self.assertRaisesRegex(RuntimeError, "changed"):
                io.copy_verified(source, target, 100, expected=sha(b"manifest bytes"))
            self.assertEqual(target.read_bytes(), b"manifest bytes")
            self.assertEqual(source.read_bytes(), b"replacement bytes")

    def test_inplace_mutation_between_hash_and_copy_is_refused(self):
        io = module("safe_io")
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source, target = root / "source", root / "frozen"
            source.write_bytes(b"original")
            real_seek = os.lseek
            def mutate(descriptor, offset, how):
                source.write_bytes(b"modified")
                return real_seek(descriptor, offset, how)
            with patch.object(io.os, "lseek", side_effect=mutate), self.assertRaisesRegex(RuntimeError, "changed"):
                io.copy_verified(source, target, 100, expected=sha(b"original"))

    def test_frozen_fd_corruption_before_verification_is_refused(self):
        io = module("safe_io")
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source, target = root / "source", root / "frozen"
            source.write_bytes(b"original")
            real_read = os.read
            def corrupt(descriptor, limit):
                if target.exists() and os.fstat(descriptor).st_ino == target.stat().st_ino:
                    os.pwrite(descriptor, b"modified", 0)
                return real_read(descriptor, limit)
            with patch.object(io.os, "read", side_effect=corrupt), self.assertRaisesRegex(RuntimeError, "changed"):
                io.copy_verified(source, target, 100, expected=sha(b"original"))

    def test_target_symlink_and_hardlink_are_never_overwritten(self):
        io = module("safe_io")
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source, victim = root / "source", root / "victim"
            source.write_bytes(b"source")
            victim.write_bytes(b"unchanged")
            (root / "linked").symlink_to(victim)
            os.link(victim, root / "hard")
            for target in [root / "linked", root / "hard"]:
                with self.subTest(target=target), self.assertRaises(FileExistsError):
                    io.copy_verified(source, target, 100)
            self.assertEqual(victim.read_bytes(), b"unchanged")

    def test_control_copy_uses_declared_source_hash_and_exact_output(self):
        run = module("run")
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            checkout = root / "checkout"
            name = run.FIXTURE_FILES[0]
            source = checkout / name
            source.parent.mkdir(parents=True)
            source.write_bytes(b"declared source")
            destination = root / "frozen/scripts"
            destination.mkdir(mode=0o700, parents=True)
            with patch.object(run, "REPO", checkout):
                result = run.freeze_controls(destination, {name: sha(b"declared source"), "private-input": "unread"})
            target = destination / name.removeprefix("scripts/")
            self.assertEqual(result, {str(target.relative_to(destination.parent)): sha(b"declared source")})
            self.assertEqual(target.read_bytes(), b"declared source")
            self.assertEqual(target.stat().st_mode & 0o777, 0o400)

    def test_public_read_detects_inplace_change(self):
        io = module("safe_io")
        with tempfile.TemporaryDirectory() as folder:
            source = Path(folder) / "source"
            source.write_bytes(b"old")
            real_read = os.read
            changed = False
            def mutate(descriptor, limit):
                nonlocal changed
                value = real_read(descriptor, limit)
                if not changed:
                    source.write_bytes(b"new")
                    changed = True
                return value
            with patch.object(io.os, "read", side_effect=mutate), self.assertRaisesRegex(RuntimeError, "changed"):
                io.read_public_bytes(source, 100)

    def test_declaration_hash_is_order_independent_but_input_sensitive(self):
        run = module("run")
        self.assertEqual(run.inventory_digest({"a": "1", "b": "2"}), run.inventory_digest({"b": "2", "a": "1"}))
        self.assertNotEqual(run.inventory_digest({"a": "1"}), run.inventory_digest({"a": "2"}))
        self.assertIn("scripts/tailnet-api-status-fixture/tests/test_source_io.py", run.FIXTURE_FILES)


class StartInputs(unittest.TestCase):
    def run_start(self, inventories, mutate_manifest=False):
        from argparse import Namespace
        from types import SimpleNamespace
        run = module("run")
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            manifest = root / "manifest.json"
            original_manifest = b'{"operator":"original accepted declaration"}'
            manifest.write_bytes(original_manifest)
            built = root / "built-test"
            built.write_bytes(b"synthetic cargo executable")
            base = root / "evidence"
            args = Namespace(evidence_dir=base, manifest=manifest, target_dir=None,
                             check_only=True, expect_api_status=False)
            def freeze(_, destination, contents):
                self.assertEqual(contents, original_manifest)
                if mutate_manifest:
                    manifest.write_bytes(b'{}')
                return {"horizon": {"path": str(destination / "horizon"), "sha256": "a" * 64}}
            with patch.object(run, "runtime_mounts", return_value=[]), \
                 patch.object(run, "storage", return_value=SimpleNamespace(build=lambda *_: (built, ["synthetic-build"]),
                                                                             docker_id=lambda: "synthetic-daemon", IMAGE="synthetic-unused-image")), \
                 patch.object(run, "freeze_artifacts", side_effect=freeze), \
                 patch.object(run, "source_inputs", side_effect=inventories), \
                 patch.object(run, "freeze_controls", return_value={}) as controls, \
                 patch.object(run, "launch_container") as launch, \
                 patch.object(run.subprocess, "check_output", return_value="synthetic-source-head"), \
                 patch.object(run.sys, "platform", "linux"), patch.object(run.platform, "machine", return_value="x86_64"):
                if inventories[0] != inventories[1] or inventories[0] != inventories[2]:
                    with self.assertRaisesRegex(RuntimeError, "source changed"):
                        run.start(args)
                    launch.assert_not_called()
                    self.assertEqual(controls.call_count, int(inventories[0] == inventories[1]))
                else:
                    run.start(args)
                    record = launch.call_args.args[2]
                    self.assertEqual(record["test_artifact_sha256"], sha(b"synthetic cargo executable"))
                    frozen_test = base / "artifacts/tailnet-api-store-fixture"
                    self.assertEqual(frozen_test.read_bytes(), built.read_bytes())
                    self.assertIn("type=bind,source=" + str(frozen_test) + ",target=/fixture/bin/tailnet-api-store-fixture,readonly", record["argv"])
                    self.assertEqual(record["manifest_sha256"], sha(original_manifest))
                    self.assertEqual((base / "artifact-manifest.json").read_bytes(), original_manifest)
                    self.assertEqual(set(record["source_inventory_digests"].values()), {run.inventory_digest(inventories[0])})
                    self.assertEqual(record["source_input_declaration_sha256"], run.inventory_digest(record["source_input_declaration"]))
                    self.assertEqual(record["source_input_declaration"]["fixture_files"], run.FIXTURE_FILES)

    def test_changed_build_inputs_prevent_control_freeze_and_launch(self):
        self.run_start([{"Cargo.toml": "before"}, {"Cargo.toml": "after"}, {}])

    def test_changed_inputs_during_control_freeze_prevent_launch(self):
        self.run_start([{"Cargo.toml": "same"}, {"Cargo.toml": "same"}, {"Cargo.toml": "after"}])

    def test_record_and_mount_use_frozen_test_and_original_manifest_bytes(self):
        self.run_start([{"Cargo.toml": "same"}] * 3, mutate_manifest=True)


class CargoInputs(unittest.TestCase):
    def git(self, root, *args):
        subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, timeout=5)

    def checkout(self, root):
        self.git(root, "init", "--quiet")
        (root / ".cargo").mkdir()
        for name, value in {"Cargo.toml": b"workspace", ".cargo/config.toml": b"tracked config",
                            ".gitignore": b".cargo/config\n.cargo/config.toml\nrust-toolchain\nCargo.lock\n"}.items():
            (root / name).write_bytes(value)
            self.git(root, "add", "-f", name)

    def test_ignored_untracked_cargo_overrides_are_refused_without_reading(self):
        run = module("run")
        for name in [".cargo/config", ".cargo/config.toml", "rust-toolchain", "Cargo.lock"]:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                self.checkout(root)
                if name == ".cargo/config.toml":
                    self.git(root, "rm", "--cached", name)
                else:
                    (root / name).write_bytes(b"unrecorded build override")
                with patch.object(run, "REPO", root), patch.object(run, "FIXTURE_FILES", ()), \
                     patch.object(run, "source_bytes") as read:
                    with self.assertRaisesRegex(RuntimeError, "require staging"):
                        run.source_inputs()
                    read.assert_not_called()

    def test_tracked_legacy_config_toolchain_and_toml_are_hashed_not_arbitrary_files(self):
        run = module("run")
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            self.checkout(root)
            for name in [".cargo/config", "rust-toolchain", "rust-toolchain.toml", "Cargo.lock"]:
                (root / name).write_bytes(name.encode())
                self.git(root, "add", "-f", name)
            (root / "unrelated-private.json").write_bytes(b"must never read")
            with patch.object(run, "REPO", root), patch.object(run, "FIXTURE_FILES", ()):
                inputs = run.source_inputs()
            expected = ["Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml", ".cargo/config", ".cargo/config.toml"]
            self.assertEqual(set(inputs), set(expected))
            for name in expected:
                self.assertEqual(inputs[name], sha((root / name).read_bytes()))
            self.assertNotIn("unrelated-private.json", inputs)


if __name__ == "__main__":
    unittest.main()
