"""Offline launch boundaries: synthetic files and owned child processes, no Docker."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parents[1]


def runner():
    spec = importlib.util.spec_from_file_location("launch_fixture_run", HERE / "run.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def record():
    return {"container_name": "horizon-api-native-" + "b" * 32, "container_id": None,
            "daemon_id": "owned-daemon", "labels": {"horizon.task": "1335", "horizon.fixture": "api-status-native",
                                                    "horizon.fixture-owner": "c" * 32},
            "launch_phase": "intent", "cleanup_confirmed": False, "check_only": True,
            "expect_api_status": True, "artifact_marker": "retained-input"}


def actual(value):
    return {"Id": "a" * 64, "Name": "/" + value["container_name"], "Config": {"Labels": value["labels"]},
            "State": {"Running": False, "ExitCode": 0}}


def store(daemon="owned-daemon"):
    return SimpleNamespace(docker_id=lambda: daemon,
                           absent=lambda result, cid: result.returncode != 0 and result.stderr == "Error: No such object: " + cid)


def missing(command, **_):
    return subprocess.CompletedProcess(command, 1, stdout="", stderr="Error: No such object: " + command[-1])


class Launch(unittest.TestCase):
    def test_source_reader_refuses_escape_leaf_parent_links_hardlink_and_fifo(self):
        run = runner()
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            checkout = root / "checkout"
            checkout.mkdir()
            outside = root / "outside"
            outside.write_bytes(b"synthetic private input")
            (checkout / "link.rs").symlink_to(outside)
            (checkout / "parent").symlink_to(root, target_is_directory=True)
            os.link(outside, checkout / "hard.rs")
            os.mkfifo(checkout / "pipe.rs")
            with patch.object(run, "REPO", checkout):
                for name in ["../outside", str(outside), "link.rs", "parent/outside", "hard.rs", "pipe.rs"]:
                    with self.subTest(name=name), self.assertRaisesRegex(RuntimeError, "checkout input"):
                        run.source_bytes(name)
            self.assertEqual(outside.read_bytes(), b"synthetic private input")

    def test_input_inventory_ignores_arbitrary_untracked_files_and_requires_staged_build_roots(self):
        run = runner()
        def git(command, **_):
            return b"" if "--others" in command else b"Cargo.toml\0private-key.json\0"
        with patch.object(run.subprocess, "check_output", side_effect=git), patch.object(run, "source_bytes", return_value=b"source") as read:
            inputs = run.source_inputs()
            self.assertEqual(set(inputs), {"Cargo.toml", *run.FIXTURE_FILES})
            self.assertNotIn("private-key.json", [call.args[0] for call in read.call_args_list])
        with patch.object(run.subprocess, "check_output", return_value=b"crates/untracked.rs\0"), patch.object(run, "source_bytes") as read:
            with self.assertRaisesRegex(RuntimeError, "require staging"):
                run.source_inputs()
            read.assert_not_called()

    def test_frozen_controls_copy_only_declared_files_without_following_links(self):
        run = runner()
        import hashlib
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            checkout = root / "checkout"
            directory = checkout / "scripts/tailnet-api-status-fixture"
            directory.mkdir(parents=True)
            control = directory / "control.py"
            control.write_bytes(b"synthetic declared source")
            outside = root / "outside"
            outside.write_bytes(b"synthetic private input")
            (directory / "unlisted.py").symlink_to(outside)
            destination = root / "frozen/scripts"
            destination.mkdir(mode=0o700, parents=True)
            name = "scripts/tailnet-api-status-fixture/control.py"
            inputs = {name: hashlib.sha256(control.read_bytes()).hexdigest(),
                      "scripts/tailnet-api-status-fixture/unlisted.py": "not-read"}
            with patch.object(run, "REPO", checkout):
                hashes = run.freeze_controls(destination, inputs)
                self.assertEqual(len(hashes), 1)
                control.unlink()
                control.symlink_to(outside)
                with self.assertRaisesRegex(RuntimeError, "checkout input"):
                    run.freeze_controls(root / "refused", {name: inputs[name]})
            self.assertFalse((destination / "tailnet-api-status-fixture/unlisted.py").exists())
            self.assertEqual(outside.read_bytes(), b"synthetic private input")

    def test_create_id_is_verified_and_durable_before_start(self):
        run = runner()
        value = record()
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "ownership.json"
            def docker(command, **options):
                self.assertEqual(options["timeout"], run.DOCKER_TIMEOUT)
                journal = run.private_io().read_json(path, 4096)
                if command[1] == "create":
                    self.assertIsNone(journal["container_id"])
                    self.assertEqual(journal["launch_phase"], "intent")
                else:
                    self.assertEqual(command, ["docker", "start", "a" * 64])
                    self.assertEqual(journal["container_id"], "a" * 64)
                    self.assertEqual(journal["launch_phase"], "created")
                return "a" * 64
            with patch.object(run.subprocess, "check_output", side_effect=docker), patch.object(run, "inspect_owned", return_value=actual(value)):
                run.launch_container(["docker", "create"], path, value)
            self.assertEqual(run.private_io().read_json(path, 4096)["launch_phase"], "started")

    def test_create_timeout_reconciles_only_exact_name_labels_and_never_starts(self):
        run = runner()
        for found, mismatch in [(True, False), (False, False), (True, True)]:
            with self.subTest(found=found, mismatch=mismatch), tempfile.TemporaryDirectory() as folder:
                path = Path(folder) / "ownership.json"
                value = record()
                response = actual(value)
                if mismatch:
                    response["Config"] = {"Labels": {"horizon.fixture-owner": "other"}}
                def inspect(command, **options):
                    self.assertEqual(options["timeout"], run.DOCKER_TIMEOUT)
                    self.assertEqual(command[-1], value["container_name"])
                    return subprocess.CompletedProcess(command, 0, stdout=json.dumps([response]), stderr="") if found else missing(command)
                with patch.object(run, "storage", return_value=store()), patch.object(run.subprocess, "run", side_effect=inspect), \
                     patch.object(run.subprocess, "check_output", side_effect=subprocess.TimeoutExpired("create", 20)) as create:
                    with self.assertRaisesRegex(RuntimeError, "launch uncertain"):
                        run.launch_container(["docker", "create"], path, value)
                    self.assertEqual(create.call_count, 1)
                journal = run.private_io().read_json(path, 4096)
                self.assertTrue(journal["launch_uncertain"])
                self.assertFalse(journal["cleanup_confirmed"])
                self.assertEqual(journal["container_id"], "a" * 64 if found and not mismatch else None)

    def test_start_timeout_preserves_already_recorded_immutable_id(self):
        run = runner()
        value = record()
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "ownership.json"
            with patch.object(run.subprocess, "check_output", side_effect=["a" * 64, subprocess.TimeoutExpired("start", 20)]), \
                 patch.object(run, "inspect_owned", return_value=actual(value)):
                with self.assertRaisesRegex(RuntimeError, "launch uncertain"):
                    run.launch_container(["docker", "create"], path, value)
            journal = run.private_io().read_json(path, 4096)
            self.assertEqual(journal["container_id"], "a" * 64)
            self.assertEqual(journal["launch_phase"], "created")
            self.assertTrue(journal["launch_uncertain"])

    def test_ownership_symlink_is_refused_before_docker(self):
        run = runner()
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            outside = root / "outside"
            outside.write_text(json.dumps(record()))
            path = root / "ownership.json"
            path.symlink_to(outside)
            with patch.object(run, "storage") as storage:
                with self.assertRaisesRegex(RuntimeError, "private fixture file"):
                    run.owned(path)
                storage.assert_not_called()

    def test_logs_are_exclusive_byte_and_time_bounded_owned_subprocesses(self):
        run = runner()
        original = subprocess.Popen
        for script, expected in [("import os; os.write(1,b'x'*128)", "oversized"), ("import time; time.sleep(5)", "timed out")]:
            with self.subTest(expected=expected), tempfile.TemporaryDirectory() as folder:
                path = Path(folder) / "host-log"
                children = []
                def process(command, **options):
                    self.assertEqual(command[:3], ["docker", "logs", "--tail"])
                    child = original([sys.executable, "-c", script], **options)
                    children.append(child)
                    return child
                with patch.object(run, "MAX_LOG_BYTES", 64), patch.object(run, "DOCKER_TIMEOUT", .2), \
                     patch.object(run.subprocess, "Popen", side_effect=process):
                    with self.assertRaisesRegex(RuntimeError, expected):
                        run.collect_logs("synthetic-cid", path)
                self.assertLessEqual(path.stat().st_size, 64)
                self.assertTrue(all(child.poll() is not None for child in children))
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            outside = root / "outside"
            outside.write_bytes(b"synthetic untouched")
            path = root / "log"
            path.symlink_to(outside)
            with patch.object(run.subprocess, "Popen") as spawn:
                with self.assertRaises(FileExistsError):
                    run.collect_logs("synthetic-cid", path)
                spawn.assert_not_called()
            self.assertEqual(outside.read_bytes(), b"synthetic untouched")

    def test_candidate_result_symlink_cannot_qualify_removal(self):
        run = runner()
        value = dict(record(), container_id="a" * 64)
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            state = root / "state"
            state.mkdir(mode=0o700)
            outside = root / "outside"
            outside.write_text('{}')
            (state / "native-result.json").symlink_to(outside)
            path = root / "ownership.json"
            run.private_io().write_exclusive_json(path, value)
            with patch.object(run, "owned", return_value=value), patch.object(run, "collect_logs"), \
                 patch.object(run, "inspect_owned", return_value=actual(value)), patch.object(run.subprocess, "run") as docker:
                with self.assertRaisesRegex(RuntimeError, "private fixture file"):
                    run.cleanup(path)
                docker.assert_not_called()
            self.assertEqual(outside.read_text(), '{}')

    def test_dangling_endpoint_entry_refuses_cleanup_and_retains_container_and_files(self):
        run = runner()
        spec = importlib.util.spec_from_file_location("launch_vnc_proxy", HERE / "vnc_proxy.py")
        proxy = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(proxy)
        value = dict(record(), container_id="a" * 64, check_only=False)
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            state = root / "state"
            state.mkdir(mode=0o700)
            path = root / "ownership.json"
            run.private_io().write_exclusive_json(path, value)
            result = {"state": "passed", "owned_service_empty": True, "owned_processes_stopped": True, "target_expired": True}
            run.private_io().write_exclusive_json(state / "native-result.json", result)
            binding = proxy.binding_for(path, value, "d" * 32)
            run.private_io().write_exclusive_json(state / "vnc-relay-owner.json", binding)
            run.private_io().write_exclusive_json(state / "vnc-proxy-result.json",
                                                  {"owned_relay_stopped": True, "owned_children_stopped": True,
                                                   "container_id": value["container_id"], "relay_nonce": binding["relay_nonce"]})
            endpoint = state / "vnc-address.json"
            endpoint.symlink_to(state / "missing-endpoint-target")
            with patch.dict(sys.modules, {"vnc_proxy": proxy}), patch.object(run, "collect_logs"), \
                 patch.object(run, "owned", return_value=value), patch.object(run, "inspect_owned", return_value=actual(value)), \
                 patch.object(run.subprocess, "run") as docker, patch.object(run, "cleanup_files") as files:
                with self.assertRaisesRegex(RuntimeError, "relay cleanup unresolved"):
                    run.cleanup(path)
                docker.assert_not_called()
                files.assert_not_called()
            self.assertTrue(endpoint.is_symlink())
            self.assertFalse(run.private_io().read_json(path, 4096)["cleanup_confirmed"])
            self.assertNotIn("cleanup_phase", run.private_io().read_json(path, 4096))

    def test_rm_timeout_after_delete_and_partial_file_cleanup_can_retry(self):
        run = runner()
        for partial_files in [False, True]:
            with self.subTest(partial_files=partial_files), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                path = root / "ownership.json"
                value = dict(record(), container_id="a" * 64, cleanup_phase="absent" if partial_files else "qualified")
                state = root / "state"
                state.mkdir(mode=0o700)
                for name in ["private-home", "runtime", "horizon"]:
                    (state / name).mkdir(mode=0o700)
                (state / "keyring-password").write_bytes(b"synthetic-password")
                (state / "keyring-password").chmod(0o600)
                run.private_io().write_exclusive_json(path, value)
                if partial_files:
                    original = run.shutil.rmtree
                    def remove(name, **options):
                        if name == "runtime":
                            raise OSError("synthetic partial deletion")
                        return original(name, **options)
                    with patch.object(run, "container_absent", return_value=True), patch.object(run.shutil, "rmtree", side_effect=remove):
                        with self.assertRaisesRegex(OSError, "partial deletion"):
                            run.cleanup(path)
                    self.assertFalse((state / "private-home").exists())
                else:
                    with patch.object(run, "container_absent", side_effect=[False, True]), patch.object(run, "inspect_owned", return_value=actual(value)), \
                         patch.object(run.subprocess, "run", side_effect=subprocess.TimeoutExpired("rm", 20)):
                        with self.assertRaisesRegex(RuntimeError, "retry exact cleanup"):
                            run.cleanup(path)
                journal = run.private_io().read_json(path, 4096)
                self.assertEqual(journal["cleanup_phase"], "absent")
                self.assertFalse(journal["cleanup_confirmed"])
                with patch.object(run, "container_absent", return_value=True), patch.object(run.subprocess, "run") as docker:
                    run.cleanup(path)
                    docker.assert_not_called()
                complete = run.private_io().read_json(path, 4096)
                self.assertTrue(complete["cleanup_confirmed"])
                self.assertEqual(complete["artifact_marker"], "retained-input")
                self.assertEqual(list(state.iterdir()), [])

    def test_absence_without_qualification_or_changed_daemon_never_removes_files(self):
        run = runner()
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "ownership.json"
            value = dict(record(), container_id="a" * 64)
            run.private_io().write_exclusive_json(path, value)
            with patch.object(run, "storage", return_value=store()), patch.object(run.subprocess, "run", side_effect=missing), \
                 patch.object(run, "cleanup_files") as files:
                with self.assertRaisesRegex(RuntimeError, "unresolved"):
                    run.cleanup(path)
                files.assert_not_called()
            value["cleanup_phase"] = "absent"
            run.private_io().replace_json(path, value)
            with patch.object(run, "storage", return_value=store("changed-daemon")), patch.object(run.subprocess, "run") as docker, \
                 patch.object(run, "cleanup_files") as files:
                with self.assertRaisesRegex(RuntimeError, "absence uncertain"):
                    run.cleanup(path)
                docker.assert_not_called()
                files.assert_not_called()

    def test_cleanup_directory_symlink_is_refused_and_guest_log_inode_is_ignored(self):
        run = runner()
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            state = root / "state"
            state.mkdir(mode=0o700)
            outside = root / "outside"
            outside.mkdir(mode=0o700)
            marker = outside / "retained"
            marker.write_bytes(b"synthetic untouched")
            (state / "private-home").symlink_to(outside, target_is_directory=True)
            with self.assertRaisesRegex(RuntimeError, "cleanup path refused"):
                run.cleanup_files(root / "ownership.json")
            self.assertEqual(marker.read_bytes(), b"synthetic untouched")
            (state / "container.log").symlink_to(marker)
            value = dict(record(), container_id="a" * 64)
            path = root / "ownership.json"
            run.private_io().write_exclusive_json(path, value)
            log_paths = []
            with patch.object(run, "collect_logs", side_effect=lambda _, log: log_paths.append(log)), \
                 patch.object(run, "inspect_owned", return_value={"State": {"Running": True}}):
                with self.assertRaisesRegex(RuntimeError, "close owned Horizon"):
                    run.qualify_cleanup(path, value)
            self.assertEqual(log_paths[0].parent, root)
            self.assertNotEqual(log_paths[0], state / "container.log")
            self.assertEqual(marker.read_bytes(), b"synthetic untouched")


if __name__ == "__main__":
    unittest.main()
