"""Real OpenSSH regressions with private, synthetic filesystem fixtures only."""

import base64
import concurrent.futures
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import tempfile
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
                      json.dumps({**original, "version": 2}).encode(),
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
                self.assertFalse((self.runtime / identity.KEY_NAME).exists())
            return real_keygen(*arguments)

        with mock.patch.object(identity, "synchronize", synchronize), mock.patch.object(identity, "keygen", keygen):
            self.store.prepare()
        self.assertTrue(self.store.marker.exists())
        self.assertIn((self.store.directory, True), observed)


if __name__ == "__main__":
    unittest.main()
