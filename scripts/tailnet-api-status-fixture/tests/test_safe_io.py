"""Offline hostile-entry checks; no Docker, sockets, native app or credentials."""
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import MagicMock, patch
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]


def module(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / (name + ".py"))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


safe_io = module("safe_io")


def private(path, raw=b'{"value":1}'):
    path.write_bytes(raw)
    path.chmod(0o600)
    return path


class SafeIo(unittest.TestCase):
    def test_private_bounded_json_and_exclusive_creation(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "control.json"
            safe_io.write_exclusive_json(path, {"value": 1})
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(safe_io.read_json(path, 4096), {"value": 1})
            with self.assertRaises(FileExistsError):
                safe_io.write_exclusive_json(path, {"value": 2})
            self.assertEqual(safe_io.read_json(path, 4096), {"value": 1})
            with self.assertRaises(FileNotFoundError):
                safe_io.read_json(Path(folder) / "absent", 4096)

    def test_symlink_hardlink_fifo_directory_public_and_oversized_reads_are_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            target = private(root / "target")
            (root / "symlink").symlink_to(target)
            os.link(target, root / "hardlink")
            os.mkfifo(root / "fifo", 0o600)
            (root / "directory").mkdir(mode=0o700)
            private(root / "public").chmod(0o644)
            private(root / "large", b"x" * 4097)
            for name in ["symlink", "hardlink", "fifo", "directory", "public", "large"]:
                with self.subTest(name=name), self.assertRaisesRegex(RuntimeError, "refused"):
                    safe_io.read_bytes(root / name, 4096)
            self.assertEqual(target.read_bytes(), b'{"value":1}')

    def test_fd_checks_refuse_wrong_uid_and_unlinked_file(self):
        with tempfile.TemporaryDirectory() as folder:
            path = private(Path(folder) / "control")
            real = safe_io.os.fstat
            for change in ["uid", "unlink"]:
                def changed(descriptor):
                    metadata = real(descriptor)
                    if stat.S_ISREG(metadata.st_mode):
                        if change == "unlink":
                            path.unlink(missing_ok=True)
                            return real(descriptor)
                        fields = list(metadata)
                        fields[4] += 1
                        return os.stat_result(fields)
                    return metadata
                with self.subTest(change=change), patch.object(safe_io.os, "fstat", side_effect=changed):
                    with self.assertRaisesRegex(RuntimeError, "refused"):
                        safe_io.read_bytes(path, 4096)

    def test_no_follow_open_refuses_leaf_swapped_after_call_begins(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = private(root / "control")
            target = private(root / "target", b"host sentinel")
            original_open = os.open
            def swapped(name, flags, *args, **kwargs):
                if name == "control":
                    path.unlink()
                    path.symlink_to(target)
                return original_open(name, flags, *args, **kwargs)
            with patch.object(safe_io.os, "open", side_effect=swapped):
                with self.assertRaisesRegex(RuntimeError, "refused"):
                    safe_io.read_bytes(path, 4096)
            self.assertEqual(target.read_bytes(), b"host sentinel")

    def test_hardlink_added_between_open_and_fd_validation_is_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = private(root / "control")
            original_open = os.open
            def linked(name, flags, *args, **kwargs):
                descriptor = original_open(name, flags, *args, **kwargs)
                if name == "control":
                    os.link(path, root / "second-link")
                return descriptor
            with patch.object(safe_io.os, "open", side_effect=linked):
                with self.assertRaisesRegex(RuntimeError, "refused"):
                    safe_io.read_bytes(path, 4096)
            self.assertEqual(path.read_bytes(), b'{"value":1}')

    def test_exclusive_write_refuses_leaf_created_at_open_race(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            target = private(root / "target", b"host sentinel")
            original_open = os.open
            def swapped(name, flags, *args, **kwargs):
                if name == "receipt":
                    (root / "receipt").symlink_to(target)
                return original_open(name, flags, *args, **kwargs)
            with patch.object(safe_io.os, "open", side_effect=swapped):
                with self.assertRaises(FileExistsError):
                    safe_io.write_exclusive_json(root / "receipt", {})
            self.assertEqual(target.read_bytes(), b"host sentinel")

    def test_no_follow_traversal_refuses_parent_swap_before_open(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            state, outside = root / "state", root / "outside"
            state.mkdir(mode=0o700)
            outside.mkdir(mode=0o700)
            original_open = os.open
            def swapped(name, flags, *args, **kwargs):
                if name == "state":
                    state.rename(root / "original")
                    state.symlink_to(outside, target_is_directory=True)
                return original_open(name, flags, *args, **kwargs)
            with patch.object(safe_io.os, "open", side_effect=swapped):
                with self.assertRaisesRegex(RuntimeError, "refused"):
                    safe_io.write_exclusive_json(state / "receipt", {"value": 1})
            self.assertEqual(list(outside.iterdir()), [])

    def test_open_directory_remains_anchored_after_parent_swap(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            state, outside, original = root / "state", root / "outside", root / "original"
            state.mkdir(mode=0o700)
            outside.mkdir(mode=0o700)
            with safe_io.PrivateDirectory(state) as directory:
                state.rename(original)
                state.symlink_to(outside, target_is_directory=True)
                directory.write_exclusive_json("receipt", {"value": 1})
                self.assertEqual(directory.read_json("receipt", 4096), {"value": 1})
            self.assertEqual(list(outside.iterdir()), [])
            self.assertEqual(safe_io.read_json(original / "receipt", 4096), {"value": 1})
            with self.assertRaisesRegex(RuntimeError, "refused"):
                safe_io.read_json(state / "receipt", 4096)

    def test_every_existing_output_entry_is_refused_without_target_mutation(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            target = private(root / "target", b"host sentinel")
            (root / "symlink").symlink_to(target)
            os.link(target, root / "hardlink")
            os.mkfifo(root / "fifo", 0o600)
            private(root / "regular", b"previous receipt")
            for name in ["symlink", "hardlink", "fifo", "regular"]:
                with self.subTest(name=name), self.assertRaises(FileExistsError):
                    with safe_io.open_exclusive(root / name) as output:
                        output.write(b"changed")
            self.assertEqual(target.read_bytes(), b"host sentinel")
            self.assertEqual((root / "regular").read_bytes(), b"previous receipt")

    def test_candidate_growth_is_capped_before_json_parsing(self):
        with tempfile.TemporaryDirectory() as folder:
            path = private(Path(folder) / "control", b"{}")
            original_read = os.read
            requested = []
            def growing(descriptor, count):
                requested.append(count)
                path.write_bytes(b"x" * 8192)
                return original_read(descriptor, count)
            with patch.object(safe_io.os, "read", side_effect=growing):
                with self.assertRaisesRegex(RuntimeError, "refused"):
                    safe_io.read_json(path, 4096)
            self.assertEqual(requested, [4097])

    def test_private_parent_and_lexical_escape_are_required(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            private(root / "control")
            root.chmod(0o755)
            for operation in [lambda: safe_io.read_json(root / "control", 4096),
                              lambda: safe_io.write_exclusive_json(root / "receipt", {})]:
                with self.assertRaisesRegex(RuntimeError, "refused"):
                    operation()
            root.chmod(0o700)
            with self.assertRaisesRegex(RuntimeError, "refused"):
                safe_io.read_json(root / ".." / root.name / "control", 4096)
            with safe_io.PrivateDirectory(root) as directory:
                for name in ["../control", "/control", "", ".."]:
                    with self.assertRaisesRegex(RuntimeError, "refused"):
                        directory.write_exclusive_json(name, {})

    def test_json_duplicate_fields_and_invalid_encoding_are_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "control"
            for raw in [b'{"nonce":"old","nonce":"new"}', b"\xff", b"{"]:
                private(path, raw)
                with self.subTest(raw=raw), self.assertRaisesRegex(RuntimeError, "refused"):
                    safe_io.read_json(path, 4096)

    def test_host_journal_replace_uses_new_inode_without_truncating_old_inode(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "ownership.json"
            safe_io.replace_json(path, {"container_id": None})
            with path.open("rb") as prior:
                old_identity = os.fstat(prior.fileno()).st_ino
                safe_io.replace_json(path, {"container_id": "synthetic-cid"})
                self.assertEqual(json.load(prior), {"container_id": None})
            self.assertNotEqual(path.stat().st_ino, old_identity)
            self.assertEqual(safe_io.read_json(path, 4096), {"container_id": "synthetic-cid"})

    def test_host_journal_replace_refuses_symlink_hardlink_fifo_and_changed_destination(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            target = private(root / "target", b"host sentinel")
            (root / "symlink").symlink_to(target)
            os.link(target, root / "hardlink")
            os.mkfifo(root / "fifo", 0o600)
            for name in ["symlink", "hardlink", "fifo"]:
                with self.subTest(name=name), self.assertRaisesRegex(RuntimeError, "refused"):
                    safe_io.replace_json(root / name, {})
            path = private(root / "ownership")
            original_write = safe_io.PrivateDirectory.write_exclusive_json
            def swapped(directory, name, value):
                result = original_write(directory, name, value)
                path.unlink()
                path.symlink_to(target)
                return result
            with patch.object(safe_io.PrivateDirectory, "write_exclusive_json", swapped):
                with self.assertRaisesRegex(RuntimeError, "refused"):
                    safe_io.replace_json(path, {})
            self.assertEqual(target.read_bytes(), b"host sentinel")
            self.assertEqual(list(root.glob(".fixture-write-*")), [])

    def test_owned_endpoint_unlink_refuses_replaced_inode(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            with safe_io.PrivateDirectory(root) as directory:
                identity = directory.write_exclusive_json("address", {"endpoint": "synthetic"})
                (root / "address").unlink()
                private(root / "address", b"replacement")
                with self.assertRaisesRegex(RuntimeError, "refused"):
                    directory.unlink_owned("address", identity)
                self.assertEqual((root / "address").read_bytes(), b"replacement")


class RelayFiles(unittest.TestCase):
    def test_accepted_client_validates_actual_ownership_before_starting_docker_child(self):
        proxy = module("vnc_proxy")
        run = module("run")
        record = {"container_id": "a" * 64, "container_name": "horizon-api-native-" + "b" * 32,
                  "daemon_id": "synthetic-daemon", "labels": {"horizon.fixture-owner": "c" * 32}}
        events, errors = [], []
        client, listener, process = MagicMock(), MagicMock(), MagicMock()
        listener.__enter__.return_value = listener
        listener.getsockname.return_value = ("127.0.0.1", 23456)
        listener.accept.return_value = (client, ("127.0.0.1", 23457))
        process.poll.return_value = 0
        def inspect(command, **options):
            self.assertEqual(command, ["docker", "inspect", record["container_id"]])
            self.assertGreater(options["timeout"], 0)
            events.append("ownership inspected")
            actual = {"Id": record["container_id"], "Name": "/" + record["container_name"],
                      "Config": {"Labels": record["labels"]}}
            return subprocess.CompletedProcess(command, 0, stdout=json.dumps([actual]), stderr="")
        def start(command, **options):
            self.assertEqual(events, ["ownership inspected"])
            self.assertEqual(command[:4], ["docker", "exec", "-i", record["container_id"]])
            self.assertIs(options["stdin"], client)
            self.assertIs(options["stdout"], client)
            events.append("owned child started")
            return process
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            state = root / "state"
            state.mkdir(mode=0o700)
            ownership = root / "ownership.json"
            safe_io.write_exclusive_json(ownership, record)
            with proxy.safe_io.PrivateDirectory(state) as directory, patch.dict(sys.modules, {"run": run}), \
                 patch.object(run, "storage", return_value=SimpleNamespace(docker_id=lambda: record["daemon_id"])), \
                 patch.object(run.subprocess, "run", side_effect=inspect), patch.object(proxy.subprocess, "Popen", side_effect=start) as child, \
                 patch.object(proxy.socket, "socket", return_value=listener), patch.object(proxy.signal, "signal"), \
                 patch.object(proxy.threading, "excepthook", side_effect=lambda error: errors.append(error.exc_value)), \
                 patch.object(proxy, "requested", side_effect=[False, True]):
                proxy.serve_owned(ownership, record, directory)
            self.assertEqual(errors, [])
            self.assertEqual(events, ["ownership inspected", "owned child started"])
            child.assert_called_once()
            client.close.assert_called_once()
            result = proxy.read_control(state / "vnc-proxy-result.json")
            self.assertTrue(result["owned_relay_stopped"])
            self.assertTrue(result["owned_children_stopped"])
            self.assertFalse((state / "vnc-address.json").exists())

    def test_mocked_relay_removes_only_its_endpoint_and_writes_private_result(self):
        proxy = module("vnc_proxy")
        record = {"container_id": "synthetic-cid", "daemon_id": "synthetic-daemon", "labels": {}}
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            listener = MagicMock()
            listener.__enter__.return_value = listener
            listener.getsockname.return_value = ("127.0.0.1", 23456)
            with proxy.safe_io.PrivateDirectory(root) as directory, \
                 patch.object(proxy.socket, "socket", return_value=listener), \
                 patch.object(proxy.signal, "signal"), patch.object(proxy, "requested", return_value=True):
                proxy.serve_owned(root / "ownership.json", record, directory)
            self.assertFalse((root / "vnc-address.json").exists())
            result = proxy.read_control(root / "vnc-proxy-result.json")
            owner = proxy.read_control(root / "vnc-relay-owner.json")
            self.assertTrue(result["owned_relay_stopped"])
            self.assertTrue(result["owned_children_stopped"])
            self.assertEqual(result["container_id"], record["container_id"])
            self.assertEqual(result["relay_nonce"], owner["relay_nonce"])
            self.assertEqual((root / "vnc-proxy-result.json").stat().st_mode & 0o777, 0o600)
            listener.accept.assert_not_called()

    def test_relay_endpoint_and_result_never_overwrite_existing_host_targets(self):
        proxy = module("vnc_proxy")
        record = {"container_id": "synthetic-cid", "daemon_id": "synthetic-daemon", "labels": {}}
        for name in ["vnc-address.json", "vnc-proxy-result.json"]:
            for kind in ["symlink", "hardlink", "fifo", "regular"]:
                with self.subTest(name=name, kind=kind), tempfile.TemporaryDirectory() as folder:
                    root = Path(folder)
                    target = private(root / "host-target", b"host sentinel")
                    path = root / name
                    if kind == "symlink":
                        path.symlink_to(target)
                    elif kind == "hardlink":
                        os.link(target, path)
                    elif kind == "fifo":
                        os.mkfifo(path, 0o600)
                    else:
                        private(path, b"previous receipt")
                    listener = MagicMock()
                    listener.__enter__.return_value = listener
                    listener.getsockname.return_value = ("127.0.0.1", 23456)
                    with proxy.safe_io.PrivateDirectory(root) as directory, \
                         patch.object(proxy.socket, "socket", return_value=listener), \
                         patch.object(proxy.signal, "signal"), patch.object(proxy, "requested", return_value=True):
                        with self.assertRaises(FileExistsError):
                            proxy.serve_owned(root / "ownership.json", record, directory)
                    self.assertEqual(target.read_bytes(), b"host sentinel")
                    if kind == "regular":
                        self.assertEqual(path.read_bytes(), b"previous receipt")
                    listener.accept.assert_not_called()

    def test_relay_control_refuses_non_object_and_stop_symlink(self):
        proxy = module("vnc_proxy")
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = private(root / "control", b"[]")
            with self.assertRaisesRegex(RuntimeError, "shape refused"):
                proxy.read_control(path)
            self.assertFalse(proxy.stop_requested(root, {}))
            (root / "vnc-stop-request.json").symlink_to(path)
            with self.assertRaisesRegex(RuntimeError, "refused"):
                proxy.stop_requested(root, {})


if __name__ == "__main__":
    unittest.main()
