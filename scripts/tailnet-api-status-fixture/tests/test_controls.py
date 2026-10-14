"""Offline control checks; no Docker, bus, socket, native app or credentials."""
import importlib.util
import json
from pathlib import Path
import queue
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def module(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / (name + ".py"))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


class Controls(unittest.TestCase):
    def test_missing_native_prerequisites_refuse_before_artifacts_build_or_docker(self):
        run = module("run")
        from argparse import Namespace
        for missing in ["/usr/bin/xkbcomp", "/usr/bin/Xvfb", "/usr/share/X11/xkb"]:
            with self.subTest(missing=missing), tempfile.TemporaryDirectory() as folder:
                evidence = Path(folder) / "not-created"
                args = Namespace(evidence_dir=evidence, manifest=Path("unused"), target_dir=None,
                                 check_only=False, expect_api_status=False)
                with patch.object(run.platform, "machine", return_value="x86_64"), patch.object(run.sys, "platform", "linux"), \
                     patch.object(Path, "is_file", lambda path: str(path) != missing), \
                     patch.object(Path, "is_dir", lambda path: str(path) != missing), \
                     patch.object(run.os, "access", return_value=True), \
                     patch.object(run, "storage") as storage, patch.object(run, "freeze_artifacts") as freeze, \
                     patch.object(run.subprocess, "check_output") as execute:
                    with self.assertRaisesRegex(RuntimeError, missing):
                        run.start(args)
                    storage.assert_not_called()
                    freeze.assert_not_called()
                    execute.assert_not_called()
                self.assertFalse(evidence.exists())

    def test_unexecutable_native_helper_is_refused_and_xkbcomp_mount_is_readonly(self):
        run = module("run")
        with patch.object(Path, "is_dir", return_value=True), patch.object(Path, "is_file", return_value=True), \
             patch.object(run.os, "access", lambda path, _: str(path) != "/usr/bin/xkbcomp"):
            with self.assertRaisesRegex(RuntimeError, "xkbcomp"):
                run.runtime_mounts()
        with patch.object(Path, "is_dir", return_value=True), patch.object(Path, "is_file", return_value=True), \
             patch.object(run.os, "access", return_value=True):
            self.assertIn((Path("/usr/bin/xkbcomp"), "/usr/bin/xkbcomp", True), run.runtime_mounts())

    def test_private_x11_directory_is_new_owned_and_never_reuses_a_socket_directory(self):
        control = module("control")
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder) / "private-x11"
            control.prepare_x11_directory(directory)
            self.assertTrue(directory.is_dir())
            self.assertEqual(directory.stat().st_mode & 0o777, 0o700)
            with self.assertRaisesRegex(RuntimeError, "must be new"):
                control.prepare_x11_directory(directory)
            linked = Path(folder) / "linked-x11"
            linked.symlink_to(directory, target_is_directory=True)
            with self.assertRaisesRegex(RuntimeError, "must be new"):
                control.prepare_x11_directory(linked)
            self.assertEqual(directory.stat().st_mode & 0o777, 0o700)

    def test_observer_is_a_normal_agent_without_supplied_host_identity(self):
        config = module("control").document("a" * 64)
        agent = config["workspaces"][0]["terminals"][0]
        self.assertEqual(agent["kind"], "codex")
        self.assertEqual(agent["command"], "/usr/bin/python3")
        self.assertNotIn("env", agent)
        self.assertNotIn("HORIZON_BROWSER_ACTOR", json.dumps(config))
        self.assertNotIn("HORIZON_BROWSER_HOST_INSTANCE", json.dumps(config))
        self.assertEqual(config["workspaces"][0]["cwd"], "/fixture/state/repo")

    def test_expect_mode_is_explicit_and_does_not_forge_agent_identity(self):
        config = module("control").document("a" * 64, True)
        agent = config["workspaces"][0]["terminals"][0]
        self.assertIn("--expect-api-status", agent["args"])
        self.assertNotIn("env", agent)
        baseline = module("control").document("a" * 64)
        self.assertNotIn("--expect-api-status", baseline["workspaces"][0]["terminals"][0]["args"])

    def test_baseline_qualifies_legacy_catalog_without_api_observations(self):
        observer = module("observer")
        expected = {"names": {"a": "A", "b": "B", "c": "C"}, "credentials": {}}
        payload = {"tailnets": [{"id": key, "name": name} for key, name in expected["names"].items()]}
        self.assertTrue(observer.validate_inventory(payload, expected, False, True))
        self.assertFalse(observer.validate_inventory(payload, expected, True, True))
        payload["tailnets"].append({"id": "unknown", "name": "Unexpected"})
        with self.assertRaisesRegex(RuntimeError, "identity mismatch"):
            observer.validate_inventory(payload, expected, False, False)

    def test_api_mode_requires_ready_kinds_and_keeps_pending_distinct(self):
        observer = module("observer")
        expected = {"names": {"a": "A"}, "credentials": {"a": {"state": "stored", "kind": "oauth"}}}
        payload = {"tailnets": [{"id": "a", "name": "A"}], "tailnet_api_credentials": {"a": {"status": "pending"}}}
        self.assertFalse(observer.validate_inventory(payload, expected, True, True))
        self.assertTrue(observer.validate_inventory(payload, expected, True, False))
        payload["tailnet_api_credentials"]["a"] = {"status": "ready", "credential": expected["credentials"]["a"]}
        self.assertTrue(observer.validate_inventory(payload, expected, True, True))
        payload["tailnet_api_credentials"]["a"]["credential"] = {"state": "missing"}
        with self.assertRaisesRegex(RuntimeError, "kind mismatch"):
            observer.validate_inventory(payload, expected, True, True)

    def test_baseline_never_dispatches_unshipped_tailnets_cli(self):
        run = module("run")
        from argparse import Namespace
        with patch.object(run, "owned", return_value={"container_id": "owned", "expect_api_status": False}), patch.object(run.subprocess, "run") as execute:
            with self.assertRaisesRegex(RuntimeError, "unshipped"):
                run.action(Namespace(action="cli", ownership=Path("unused")))
            execute.assert_not_called()

    def test_frozen_artifact_mismatch_refuses_before_copy(self):
        run = module("run")
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            binary = root / "input"
            binary.write_bytes(b"synthetic executable")
            manifest = root / "manifest.json"
            manifest.write_text(json.dumps({"horizon": {"path": str(binary), "sha256": "0" * 64}}))
            out = root / "output"
            out.mkdir()
            with self.assertRaisesRegex(RuntimeError, "hash mismatch"):
                run.freeze_artifacts(manifest, out)
            self.assertEqual(list(out.iterdir()), [])

    def test_ownership_does_not_query_another_docker_daemon(self):
        run = module("run")
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "owned.json"
            path.write_text(json.dumps({"daemon_id": "original"}))
            path.chmod(0o600)
            with patch.object(run, "storage") as storage, patch.object(run.subprocess, "check_output") as inspect:
                storage.return_value.docker_id.return_value = "other"
                with self.assertRaisesRegex(RuntimeError, "identity changed"):
                    run.owned(path)
                inspect.assert_not_called()

    def test_public_rpc_evidence_contains_the_response_without_private_ipc(self):
        observer = module("observer")
        with tempfile.TemporaryDirectory() as folder:
            rpc = object.__new__(observer.Rpc)
            rpc.events = queue.Queue()
            rpc.events.put({"jsonrpc": "2.0", "method": "notification"})
            reply = {"jsonrpc": "2.0", "id": 1, "result": {"content": [{"type": "text", "text": "synthetic public status"}]}}
            rpc.events.put(reply)
            rpc.next_id = 0
            rpc.evidence = Path(folder)
            sent = []
            rpc.send = sent.append
            result = rpc.request("tools/call", {"name": "cloud_companions", "arguments": {}})
            self.assertEqual(result, reply)
            self.assertEqual(sent[0]["params"]["name"], "cloud_companions")
            logged = json.loads((rpc.evidence / "rpc.jsonl").read_text())
            self.assertEqual(logged["response"], reply)
            self.assertEqual(logged["method"], "tools/call")

    def test_observer_refuses_missing_host_launch_context(self):
        observer = module("observer")
        with patch.object(Path, "is_file", return_value=True):
            with self.assertRaisesRegex(RuntimeError, "normal Horizon agent"):
                observer.validate_launch({}, Path("/fixture/state/horizon"), Path("unused"), "a" * 64)


if __name__ == "__main__":
    unittest.main()
