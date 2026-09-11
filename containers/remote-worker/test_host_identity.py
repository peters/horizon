"""Real OpenSSH regressions with private, synthetic filesystem fixtures only."""

import base64
import concurrent.futures
from contextlib import contextmanager
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location("host_identity", Path(__file__).with_name("host-identity.py"))
identity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(identity)


class HostIdentityTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="horizon-host-identity-")
        self.root = Path(self.temporary.name)
        self.root.chmod(0o700)
        self.workspace = self.root / "workspace"
        self.runtime = self.root / "runtime"
        self.workspace.mkdir(mode=0o700)
        self.runtime.mkdir(mode=0o700)
        self.client = self.root / "client"
        identity.keygen("-q", "-t", "ed25519", "-N", "", "-C", "", "-f", self.client)
        self.access = self.client.with_suffix(".pub")
        self.access.chmod(0o600)
        self.store = self.new_store()

    def tearDown(self):
        self.temporary.cleanup()

    def new_store(self, runtime=None):
        return identity.HostIdentity(self.workspace, self.access, runtime or self.runtime)

    def snapshot(self):
        return {str(path.relative_to(self.store.parent)): path.read_bytes()
                for path in self.store.parent.rglob("*") if path.is_file() and not path.is_symlink()}

    def assert_rejected_without_generation(self, store=None):
        with mock.patch.object(identity, "WAIT_SECONDS", 0), mock.patch.object(identity, "keygen") as utility:
            with self.assertRaises((identity.IdentityError, OSError, ValueError)):
                (store or self.store).prepare()
            utility.assert_not_called()

    def bootstrap_environment(self):
        return {identity.BOOTSTRAP_ENV: "1", "HORIZON_CLOUD_PROTOCOL_VERSION": "1", "RUNPOD_POD_ID": "pod_synthetic",
                "HORIZON_WORKFLOW_ID": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                "HORIZON_JOB_ID": "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"}

    def test_image_does_not_populate_workspace_before_identity_validation(self):
        directory = Path(__file__).parent
        dockerfile = (directory / "Dockerfile").read_text()
        entrypoint = (directory / "entrypoint.sh").read_text()
        self.assertEqual("WORKDIR /", [line for line in dockerfile.splitlines()
                                      if line.startswith("WORKDIR ")][-1])
        self.assertNotIn("/workspace/horizon", dockerfile)
        prepare = entrypoint.index("\n/usr/local/bin/horizon-worker-host-identity\n")
        create = entrypoint.index("\nmkdir -p /workspace/horizon\n")
        enter = entrypoint.index("\ncd /workspace/horizon\n")
        self.assertLess(prepare, create)
        self.assertLess(create, enter)

    def test_bootstrap_record_is_emitted_only_after_retained_key_materialization(self):
        output = io.StringIO()
        observed = []

        def write(line):
            observed.append((self.store.marker.exists(), (self.runtime / identity.KEY_NAME).exists()))
            return output.write(line)

        writer = mock.Mock(write=write, flush=output.flush)
        public = identity.prepare_for_startup(self.store, self.bootstrap_environment(), writer)
        self.assertEqual([(True, True)], observed)
        line = output.getvalue()
        self.assertLessEqual(len(line.encode()), identity.BOOTSTRAP_LIMIT)
        self.assertTrue(line.startswith(identity.BOOTSTRAP_PREFIX))
        record = json.loads(line[len(identity.BOOTSTRAP_PREFIX):])
        self.assertEqual({**identity.bootstrap_context(self.bootstrap_environment()),
                          "access_digest": self.store.claim.read_text(), "host_public_key": public}, record)
        self.assertNotIn("PRIVATE KEY", line)
        self.assertEqual(public, identity.public_key((self.runtime / (identity.KEY_NAME + ".pub")).read_bytes()))
        before = self.snapshot()
        reopened = io.StringIO()
        self.assertEqual(public, identity.prepare_for_startup(self.new_store(), self.bootstrap_environment(), reopened))
        self.assertEqual(line, reopened.getvalue())
        self.assertEqual(before, self.snapshot())

    def test_invalid_explicit_bootstrap_context_fails_before_identity_preparation(self):
        for field in self.bootstrap_environment():
            for value in (None, "", "invalid/context", "x" * 192):
                environment = self.bootstrap_environment()
                if value is None and field != identity.BOOTSTRAP_ENV:
                    del environment[field]
                else:
                    environment[field] = value
                with self.subTest(field=field, value=value), mock.patch.object(self.store, "prepare") as prepare:
                    with self.assertRaises((identity.IdentityError, ValueError)):
                        identity.prepare_for_startup(self.store, environment, io.StringIO())
                    prepare.assert_not_called()
        output = io.StringIO()
        public = identity.prepare_for_startup(self.store, {"RUNPOD_POD_ID": "unrelated"}, output)
        self.assertEqual("", output.getvalue())
        self.assertEqual(public, self.store.prepare())

    def test_failed_preparation_or_bootstrap_output_never_replaces_retained_identity(self):
        environment = self.bootstrap_environment()
        output = io.StringIO()
        with mock.patch.object(self.store, "prepare", side_effect=identity.IdentityError):
            with self.assertRaises(identity.IdentityError):
                identity.prepare_for_startup(self.store, environment, output)
        self.assertEqual("", output.getvalue())
        self.store.prepare()
        before = self.snapshot()
        for writer in (mock.Mock(write=mock.Mock(side_effect=BrokenPipeError)), mock.Mock(write=lambda _: 0),
                       mock.Mock(write=len, flush=mock.Mock(side_effect=OSError))):
            with self.assertRaises((identity.IdentityError, OSError)):
                identity.prepare_for_startup(self.new_store(), environment, writer)
            self.assertEqual(before, self.snapshot())

    def test_same_key_after_reopen_and_loss_of_runtime_filesystem(self):
        public = self.store.prepare()
        before = self.snapshot()
        self.assertEqual(public, self.new_store().prepare())
        shutil.rmtree(self.runtime)
        self.runtime.mkdir(mode=0o700)
        self.assertEqual(public, self.new_store().prepare())
        self.assertEqual(before, self.snapshot())
        runtime_key = self.runtime / identity.KEY_NAME
        self.assertEqual(public, identity.public_key(identity.keygen("-y", "-P", "", "-f", runtime_key)))
        for path in (self.store.claim, self.store.marker, runtime_key):
            self.assertEqual(0o600, path.stat().st_mode & 0o777)
        self.assertEqual(0o700, self.store.directory.stat().st_mode & 0o777)
        self.assertEqual(0o700, self.store.panels.stat().st_mode & 0o777)
        self.assertEqual(identity.STATE_VERSION, json.loads(self.store.marker.read_bytes())["version"])

    def test_retained_panel_records_survive_runtime_filesystem_replacement(self):
        public = self.store.prepare()
        record = self.store.panels / "synthetic-record"
        record.write_bytes(b"retained-task-claim\0")
        record.chmod(0o600)
        before = self.snapshot()
        shutil.rmtree(self.runtime)
        self.runtime.mkdir(mode=0o700)
        self.assertEqual(public, self.new_store().prepare())
        self.assertEqual(before, self.snapshot())

    def test_missing_retained_panel_root_is_not_recreated(self):
        self.store.prepare()
        self.store.panels.rmdir()
        before = self.snapshot()
        self.assert_rejected_without_generation()
        self.assertFalse(self.store.panels.exists())
        self.assertEqual(before, self.snapshot())

    def test_legacy_readiness_is_not_automatically_migrated(self):
        self.store.prepare()
        marker = json.loads(self.store.marker.read_bytes())
        self.store.marker.write_bytes(json.dumps({**marker, "version": 1}).encode())
        self.store.panels.rmdir()
        before = self.snapshot()
        self.assert_rejected_without_generation()
        self.assertFalse(self.store.panels.exists())
        self.assertEqual(before, self.snapshot())

    def test_insecure_or_linked_panel_root_is_rejected_without_identity_replacement(self):
        self.store.prepare()
        before = self.snapshot()
        self.store.panels.chmod(0o755)
        self.assert_rejected_without_generation()
        self.store.panels.chmod(0o700)
        retained = self.store.panels.with_name("retained-panels")
        self.store.panels.rename(retained)
        self.store.panels.symlink_to(retained)
        self.assert_rejected_without_generation()
        self.assertEqual(before, self.snapshot())

    def test_preexisting_unclaimed_panel_state_is_not_adopted(self):
        self.store.parent.mkdir(mode=0o700)
        self.store.panels.mkdir(mode=0o700)
        record = self.store.panels / "unclaimed"
        record.write_bytes(b"must-not-adopt-or-remove")
        self.assert_rejected_without_generation()
        self.assertEqual(b"must-not-adopt-or-remove", record.read_bytes())
        self.assertFalse(self.store.marker.exists())

    def test_separate_workspaces_never_share_host_identity(self):
        first = self.store.prepare()
        other_workspace = self.root / "other-workspace"
        other_runtime = self.root / "other-runtime"
        other_workspace.mkdir(mode=0o700)
        other_runtime.mkdir(mode=0o700)
        other = identity.HostIdentity(other_workspace, self.access, other_runtime)
        self.assertNotEqual(first, other.prepare())

    def test_concurrent_initialization_publishes_one_host_key(self):
        destinations = [self.root / f"runtime-{index}" for index in range(8)]
        for path in destinations:
            path.mkdir(mode=0o700)
        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
            results = list(pool.map(lambda path: self.new_store(path).prepare(), destinations))
        self.assertEqual(1, len(set(results)))
        self.assertEqual(1, len(list(self.store.directory.glob(identity.KEY_NAME))))

    def test_follower_wait_covers_both_key_operations_before_readiness(self):
        public = self.store.prepare()
        before = self.snapshot()
        real_read = identity.read_file
        marker_attempts = []

        def read_file(path, private=True):
            if path == self.store.marker:
                marker_attempts.append(path)
                if len(marker_attempts) < 3:
                    raise FileNotFoundError
            return real_read(path, private)

        elapsed = [0, 0, 2 * identity.KEYGEN_TIMEOUT_SECONDS]
        with mock.patch.object(identity, "read_file", read_file), \
                mock.patch.object(identity.time, "monotonic", side_effect=elapsed), \
                mock.patch.object(identity.time, "sleep") as sleep:
            self.assertEqual(public, self.new_store().prepare())
        self.assertEqual(2, sleep.call_count)
        self.assertEqual(before, self.snapshot())

    def test_exhausted_follower_deadline_does_not_replace_identity(self):
        self.store.prepare()
        self.store.marker.unlink()
        before = self.snapshot()
        with mock.patch.object(identity.time, "monotonic", side_effect=[0, identity.WAIT_SECONDS]), \
                mock.patch.object(identity.time, "sleep") as sleep, \
                mock.patch.object(identity, "keygen") as utility:
            with self.assertRaises(identity.IdentityError):
                self.new_store().prepare()
        utility.assert_not_called()
        sleep.assert_not_called()
        self.assertEqual(before, self.snapshot())

    def test_different_client_cannot_adopt_retained_volume(self):
        self.store.prepare()
        before = self.snapshot()
        self.access.write_bytes(b"ssh-ed25519 " + base64.b64encode(identity.ED25519_PREFIX + b"x" * 32))
        self.assert_rejected_without_generation()
        self.assertEqual(before, self.snapshot())

    def test_incomplete_claim_never_generates_a_replacement(self):
        self.store.prepare()
        before_claim = self.store.claim.read_bytes()
        shutil.rmtree(self.store.directory)
        self.assert_rejected_without_generation()
        self.assertEqual(before_claim, self.store.claim.read_bytes())
        self.assertFalse(self.store.directory.exists())

    def test_missing_marker_or_key_never_generates_replacement(self):
        self.store.prepare()
        for path in (self.store.marker, self.store.directory / identity.KEY_NAME,
                     self.store.directory / (identity.KEY_NAME + ".pub")):
            value = path.read_bytes()
            path.unlink()
            self.assert_rejected_without_generation()
            path.write_bytes(value)
            path.chmod(0o600)

    def test_missing_claim_is_not_recreated_for_an_existing_store(self):
        self.store.prepare()
        self.store.claim.unlink()
        before = self.snapshot()
        self.assert_rejected_without_generation()
        self.assertEqual(before, self.snapshot())

    def test_corrupt_or_future_marker_leaves_original_keys_unchanged(self):
        self.store.prepare()
        original = json.loads(self.store.marker.read_bytes())
        key = (self.store.directory / identity.KEY_NAME).read_bytes()
        for value in (b"broken", b"[]", b"x" * (identity.LIMIT + 1),
                      b'{"version": 2, ' + self.store.marker.read_bytes()[1:],
                      json.dumps({**original, "version": identity.STATE_VERSION + 1}).encode(),
                      json.dumps({**original, "version": True}).encode(),
                      json.dumps({**original, "access_digest": "wrong"}).encode(),
                      json.dumps({**original, "extra": 1}).encode()):
            self.store.marker.write_bytes(value)
            self.assert_rejected_without_generation()
            self.assertEqual(key, (self.store.directory / identity.KEY_NAME).read_bytes())

    def test_nonregular_claim_fails_without_blocking_or_creating_keys(self):
        self.store.parent.mkdir(mode=0o700)
        os.mkfifo(self.store.claim, 0o600)
        self.assert_rejected_without_generation()
        self.assertFalse(self.store.directory.exists())

    def test_cli_diagnostics_never_include_private_error_details(self):
        diagnostics = io.StringIO()
        with mock.patch.object(identity.sys, "argv", ["host-identity"]), \
                mock.patch.object(identity.sys, "stderr", diagnostics), \
                mock.patch.object(identity.HostIdentity, "prepare", side_effect=OSError("synthetic-private-detail")):
            self.assertEqual(64, identity.main())
        self.assertEqual("horizon-worker: retained SSH host identity is unavailable\n", diagnostics.getvalue())

    def test_insecure_files_and_symlinks_are_rejected(self):
        self.store.prepare()
        for path in (self.store.claim, self.store.marker, self.store.directory / identity.KEY_NAME):
            path.chmod(0o644)
            self.assert_rejected_without_generation()
            path.chmod(0o600)
            retained = path.with_name(path.name + ".retained")
            path.rename(retained)
            path.symlink_to(retained)
            self.assert_rejected_without_generation()
            path.unlink()
            retained.rename(path)

    def test_unsafe_workspace_and_linked_state_fail_before_key_generation(self):
        self.workspace.chmod(0o777)
        self.assert_rejected_without_generation()
        self.assertFalse(self.store.parent.exists())
        self.workspace.chmod(0o700)
        elsewhere = self.root / "elsewhere"
        elsewhere.mkdir(mode=0o700)
        self.store.parent.symlink_to(elsewhere)
        self.assert_rejected_without_generation()
        self.assertEqual([], list(elsewhere.iterdir()))

    def test_existing_runtime_key_is_not_silently_migrated_or_overwritten(self):
        path = self.runtime / identity.KEY_NAME
        path.write_bytes(b"legacy-private-key")
        path.chmod(0o600)
        self.assert_rejected_without_generation()
        self.assertEqual(b"legacy-private-key", path.read_bytes())
        self.assertFalse(self.store.marker.exists())

    def test_changed_runtime_key_is_not_overwritten(self):
        self.store.prepare()
        path = self.runtime / identity.KEY_NAME
        path.write_bytes(b"different-private-key")
        before = self.snapshot()
        with self.assertRaises(identity.IdentityError):
            self.store.prepare()
        self.assertEqual(before, self.snapshot())
        self.assertEqual(b"different-private-key", path.read_bytes())

    def test_claim_is_synced_before_generation_and_ready_before_materialization(self):
        real_keygen = identity.keygen
        observed = []
        real_sync = identity.synchronize

        def synchronize(path, directory=False):
            observed.append((path, directory))
            real_sync(path, directory)

        def keygen(*arguments):
            if "-q" in arguments:
                self.assertTrue(self.store.claim.exists())
                self.assertIn((self.store.parent, True), observed)
                self.assertTrue(self.store.panels.is_dir())
                self.assertIn((self.store.panels, True), observed)
                self.assertFalse((self.runtime / identity.KEY_NAME).exists())
            return real_keygen(*arguments)

        with mock.patch.object(identity, "synchronize", synchronize), mock.patch.object(identity, "keygen", keygen):
            self.store.prepare()
        self.assertTrue(self.store.marker.exists())
        self.assertIn((self.store.directory, True), observed)


    @contextmanager
    def fresh_mount(self):
        # Only authority/mount topology are simulated; chmod, fsync, directory
        # enumeration and path replacement operate on disposable real files.
        self.workspace.chmod(0o777)
        actual_fstat, actual_trusted, uid = os.fstat, identity.trusted_directory, os.geteuid()

        def fstat(descriptor):
            info = actual_fstat(descriptor)
            return SimpleNamespace(st_dev=info.st_dev, st_ino=info.st_ino,
                                   st_uid=0, st_gid=info.st_gid, st_mode=info.st_mode)

        def trusted(path, private=False):
            with mock.patch.object(identity.os, "geteuid", return_value=uid):
                actual_trusted(path, private)

        def mount(descriptor):
            return 2 if os.readlink(f"/proc/self/fd/{descriptor}") == str(self.workspace) else 1

        with mock.patch.object(identity, "WORKSPACE", self.workspace), \
                mock.patch.object(identity.os, "geteuid", return_value=0), \
                mock.patch.object(identity.os, "fstat", side_effect=fstat), \
                mock.patch.object(identity, "trusted_directory", side_effect=trusted), \
                mock.patch.object(identity, "mount_id", side_effect=mount):
            yield

    def test_fresh_mount_is_private_and_synced_before_identity_preparation(self):
        actual_sync, observed = os.fsync, []

        def sync(descriptor):
            self.assertEqual(0o700, self.workspace.stat().st_mode & 0o777)
            self.assertEqual([], list(self.workspace.iterdir()))
            observed.append("synced")
            actual_sync(descriptor)

        def prepare():
            self.assertEqual(["synced"], observed)
            raise identity.IdentityError()

        with self.fresh_mount(), mock.patch.object(identity.os, "fsync", side_effect=sync), \
                mock.patch.object(self.store, "prepare", side_effect=prepare) as start:
            with self.assertRaises(identity.IdentityError):
                identity.prepare_for_startup(self.store, self.bootstrap_environment(), io.StringIO())
            start.assert_called_once()
        self.assertFalse(self.store.parent.exists())

    def test_missing_or_invalid_bootstrap_never_prepares_unsafe_mount(self):
        with self.fresh_mount(), mock.patch.object(identity.os, "fchmod") as chmod:
            for environment in ({}, {identity.BOOTSTRAP_ENV: "invalid"}):
                with self.assertRaises(identity.IdentityError):
                    identity.prepare_for_startup(self.store, environment, io.StringIO())
            chmod.assert_not_called()
        self.assertFalse(self.store.parent.exists())

    def test_existing_protected_mount_and_nonworkspace_store_are_unchanged(self):
        with self.fresh_mount(), mock.patch.object(identity.os, "fchmod") as chmod:
            self.workspace.chmod(0o700)
            (self.workspace / "retained").write_bytes(b"synthetic")
            identity.prepare_fresh_workspace()
            chmod.assert_not_called()
        with mock.patch.object(identity, "prepare_fresh_workspace") as prepare:
            identity.prepare_for_startup(self.store, self.bootstrap_environment(), io.StringIO())
            prepare.assert_not_called()

    def test_nonempty_unsafe_mount_never_repairs_or_deletes_entries(self):
        for filename in ("retained", ".hidden"):
            path = self.workspace / filename
            path.write_bytes(b"synthetic")
            with self.fresh_mount(), mock.patch.object(identity.os, "fchmod") as chmod:
                with self.assertRaises(identity.IdentityError):
                    identity.prepare_fresh_workspace()
                chmod.assert_not_called()
            self.assertEqual(b"synthetic", path.read_bytes())
            path.unlink()

    def test_noop_chmod_and_sync_failure_refuse_without_creating_state(self):
        with self.fresh_mount(), mock.patch.object(identity.os, "fchmod"):
            with self.assertRaises(identity.IdentityError):
                identity.prepare_fresh_workspace()
        with self.fresh_mount(), mock.patch.object(identity.os, "fsync", side_effect=OSError):
            with self.assertRaises(OSError):
                identity.prepare_fresh_workspace()
        self.assertEqual([], list(self.workspace.iterdir()))

    def test_mount_id_parser_is_bounded_and_unambiguous(self):
        with mock.patch("builtins.open", return_value=io.BytesIO(b"mnt_id:\t123\n")):
            self.assertEqual(123, identity.mount_id(1))
        for value in (b"", b"mnt_id: x\n", b"mnt_id: 1\nmnt_id: 2\n", b"x" * 4097):
            with mock.patch("builtins.open", return_value=io.BytesIO(value)):
                with self.assertRaises(identity.IdentityError):
                    identity.mount_id(1)

    def test_post_chmod_mode_or_mount_change_is_rejected(self):
        actual_chmod = os.fchmod
        with self.fresh_mount(), mock.patch.object(identity.os, "fchmod",
                side_effect=lambda descriptor, mode: actual_chmod(descriptor, 0o755)):
            with self.assertRaises(identity.IdentityError):
                identity.prepare_fresh_workspace()
        with self.fresh_mount():
            mount = identity.mount_id.side_effect
            def changed_mount(descriptor):
                value = mount(descriptor)
                return 3 if value == 2 and self.workspace.stat().st_mode & 0o777 == 0o700 else value
            with mock.patch.object(identity, "mount_id", side_effect=changed_mount):
                with self.assertRaises(identity.IdentityError):
                    identity.prepare_fresh_workspace()
        self.assertFalse(self.store.parent.exists())

    def test_insertion_or_path_replacement_after_chmod_refuses_without_cleanup(self):
        actual_chmod = os.fchmod
        for replace in (False, True):
            def race(descriptor, mode):
                actual_chmod(descriptor, mode)
                if replace:
                    self.workspace.rename(self.root / "original")
                    self.workspace.mkdir(mode=0o700)
                else:
                    (self.workspace / ".concurrent").write_bytes(b"synthetic")
            with self.fresh_mount(), mock.patch.object(identity.os, "fchmod", side_effect=race):
                with self.assertRaises(identity.IdentityError):
                    identity.prepare_fresh_workspace()
            if not replace:
                self.assertEqual(b"synthetic", (self.workspace / ".concurrent").read_bytes())
                (self.workspace / ".concurrent").unlink()
            else:
                self.assertTrue((self.root / "original").is_dir())
        self.assertFalse(self.store.parent.exists())

    def test_wrong_owner_nonmount_unsafe_parent_and_symlink_refuse(self):
        with self.fresh_mount():
            for target, value in (("geteuid", 1), ("fstat", SimpleNamespace(
                    st_dev=1, st_ino=1, st_uid=1, st_gid=1, st_mode=0o40777))):
                with mock.patch.object(identity.os, target, return_value=value), \
                        mock.patch.object(identity.os, "fchmod") as chmod:
                    with self.assertRaises(identity.IdentityError):
                        identity.prepare_fresh_workspace()
                    chmod.assert_not_called()
            with mock.patch.object(identity, "mount_id", return_value=1):
                with self.assertRaises(identity.IdentityError):
                    identity.prepare_fresh_workspace()
            self.root.chmod(0o777)
            with self.assertRaises(identity.IdentityError):
                identity.prepare_fresh_workspace()
            self.root.chmod(0o700)
            self.workspace.rmdir()
            self.workspace.symlink_to(self.runtime, target_is_directory=True)
            with self.assertRaises((identity.IdentityError, OSError)):
                identity.prepare_fresh_workspace()
        self.assertEqual([], list(self.runtime.iterdir()))


if __name__ == "__main__":
    unittest.main()
