"""Offline control checks; no Docker, bus, socket, native app or credentials."""
import importlib.util
import json
from pathlib import Path
import queue
import signal
import sys
import threading
import tempfile
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[1]


def module(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / (name + ".py"))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def resolved_native_path(path, strict=False):
    if str(path) == "/usr/share/X11/xkb":
        return Path("/usr/share/fixture-xkb-data")
    return path


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
                     patch.object(run.os, "access", return_value=True), patch.object(Path, "resolve", resolved_native_path), \
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
             patch.object(run.os, "access", return_value=True), patch.object(Path, "resolve", resolved_native_path):
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

    def test_resolved_xkb_mount_and_missing_evdev_preflight(self):
        run = module("run")
        from argparse import Namespace
        with patch.object(Path, "is_dir", return_value=True), patch.object(Path, "is_file", return_value=True), \
             patch.object(run.os, "access", return_value=True), patch.object(Path, "resolve", resolved_native_path):
            self.assertIn((Path("/usr/share/fixture-xkb-data"), "/fixture/xkb", True), run.runtime_mounts())
        with tempfile.TemporaryDirectory() as folder:
            evidence = Path(folder) / "not-created"
            with patch.object(run.platform, "machine", return_value="x86_64"), patch.object(run.sys, "platform", "linux"), \
                 patch.object(Path, "is_dir", return_value=True), patch.object(run.os, "access", return_value=True), \
                 patch.object(Path, "resolve", resolved_native_path), \
                 patch.object(Path, "is_file", lambda path: str(path) != "/usr/share/fixture-xkb-data/rules/evdev"), \
                 patch.object(run, "storage") as storage, patch.object(run.subprocess, "check_output") as execute:
                with self.assertRaisesRegex(RuntimeError, "XKB rules refused"):
                    run.start(Namespace(evidence_dir=evidence))
                storage.assert_not_called()
                execute.assert_not_called()
            self.assertFalse(evidence.exists())
        def outside_share(path, strict=False):
            return Path("/tmp/untrusted-xkb") if str(path) == "/usr/share/X11/xkb" else path
        with patch.object(Path, "is_dir", return_value=True), patch.object(Path, "is_file", return_value=True), \
             patch.object(run.os, "access", return_value=True), patch.object(Path, "resolve", outside_share):
            with self.assertRaisesRegex(RuntimeError, "XKB rules refused"):
                run.runtime_mounts()

    def test_protocol_readiness_precedes_secret_fixture_construction(self):
        control = module("control")
        from argparse import Namespace
        args = Namespace(sha256="a" * 64, check_only=False, expect_api_status=False)
        with tempfile.TemporaryDirectory() as folder, patch.object(control, "ROOT", Path(folder)), \
             patch.object(control.argparse.ArgumentParser, "parse_args", return_value=args), \
             patch.object(control.os, "umask"), patch.object(control.signal, "signal"), patch.dict(control.os.environ, {}, clear=True), \
             patch.object(Path, "exists", return_value=False), \
             patch.object(control, "start_native_display", side_effect=RuntimeError("X11 not ready")) as native, \
             patch.object(control, "fixture_class") as fixture:
            with self.assertRaisesRegex(RuntimeError, "X11 not ready"):
                control.main()
            native.assert_called_once()
            fixture.assert_not_called()

    def test_check_only_never_starts_native_display(self):
        control = module("control")
        from argparse import Namespace
        args = Namespace(sha256="a" * 64, check_only=True, expect_api_status=False)
        with tempfile.TemporaryDirectory() as folder, patch.object(control, "ROOT", Path(folder)), \
             patch.object(control.argparse.ArgumentParser, "parse_args", return_value=args), \
             patch.object(control.os, "umask"), patch.object(control.signal, "signal"), patch.dict(control.os.environ, {}, clear=True), \
             patch.object(Path, "exists", return_value=False), patch.object(control, "start_native_display") as native, \
             patch.object(control, "fixture_class", side_effect=RuntimeError("synthetic constructor reached")) as fixture:
            with self.assertRaisesRegex(RuntimeError, "constructor reached"):
                control.main()
            native.assert_not_called()
            fixture.assert_called_once()

    def test_doctor_validates_real_protocol_shape_with_five_second_budget(self):
        control = module("control")
        import subprocess
        ready = {"geometry": {"width": 1600, "height": 1000, "surface_id": "display", "revision": "synthetic-revision"},
                 "capabilities": ["screenshot", "pointer", "keyboard"]}
        process = Mock()
        process.poll.return_value = None
        def doctor(command, **options):
            self.assertEqual(command[0], "/candidate/horizon-device")
            self.assertEqual(command[1:], ["--target", str(target), "doctor"])
            self.assertGreater(options["timeout"], 0)
            self.assertLessEqual(options["timeout"], 5)
            ready["geometry"]["target_id"] = json.loads(target.read_text())["id"]
            options["stdout"].write(json.dumps({"ok": True, "result": ready}).encode())
            return subprocess.CompletedProcess(command, 0)
        with tempfile.TemporaryDirectory() as folder:
            state = Path(folder)
            target = state / "target.json"
            children, logs = [], []
            with patch.object(control, "prepare_x11_directory"), patch.object(control.subprocess, "Popen", return_value=process) as spawn, \
                 patch.object(control.subprocess, "run", side_effect=doctor):
                result = control.start_native_display(state, target, children, logs)
            for log in logs:
                log.close()
            self.assertEqual(result["state"], "ready")
            self.assertEqual(json.loads(target.read_text())["endpoint"], {"kind": "local_x11", "display": ":71"})
            self.assertIn("-xkbdir", spawn.call_args.args[0])
            self.assertIn("/fixture/xkb", spawn.call_args.args[0])
            self.assertEqual(json.loads((state / "x11-readiness.json").read_text())["readiness"], ready)

    def test_doctor_timeout_records_failure_without_a_secret_fixture(self):
        control = module("control")
        import subprocess
        process = Mock()
        process.poll.return_value = None
        with tempfile.TemporaryDirectory() as folder:
            state = Path(folder)
            children, logs = [], []
            with patch.object(control, "prepare_x11_directory"), patch.object(control.subprocess, "Popen", return_value=process), \
                 patch.object(control.subprocess, "run", side_effect=subprocess.TimeoutExpired("synthetic-doctor", 5)):
                with self.assertRaisesRegex(RuntimeError, "readiness timed out"):
                    control.start_native_display(state, state / "target.json", children, logs)
            for log in logs:
                log.close()
            self.assertEqual(json.loads((state / "x11-readiness.json").read_text())["state"], "failed")
            self.assertFalse((state / "keyring-password").exists())

    def test_doctor_rejects_wrong_envelope_target_or_revision(self):
        control = module("control")
        ready = {"geometry": {"target_id": "exact-target", "surface_id": "display", "revision": "synthetic",
                               "width": 1600, "height": 1000}, "capabilities": ["screenshot", "pointer", "keyboard"]}
        self.assertEqual(control.doctor_readiness(json.dumps({"ok": True, "result": ready}), "exact-target"), ready)
        for response in [ready, {"ok": False, "result": ready}, {"ok": True, "result": []}]:
            with self.subTest(response=response), self.assertRaisesRegex(RuntimeError, "envelope refused"):
                control.doctor_readiness(json.dumps(response), "exact-target")
        for changes in [{"target_id": "other"}, {"revision": ""}, {"surface_id": ""}, {"width": 1}]:
            changed = dict(ready, geometry=dict(ready["geometry"], **changes))
            with self.subTest(changes=changes), self.assertRaisesRegex(RuntimeError, "readiness"):
                control.doctor_readiness(json.dumps({"ok": True, "result": changed}), "exact-target")

    def test_exact_owned_relay_stop_is_nonce_bound_and_pollable_without_signals(self):
        proxy = module("vnc_proxy")
        run = module("run")
        import sys
        with tempfile.TemporaryDirectory() as folder:
            ownership = Path(folder) / "ownership.json"
            state = Path(folder) / "relay"
            state.mkdir(mode=0o700)
            record = {"container_id": "exact-cid", "daemon_id": "exact-daemon", "labels": {"fixture-owner": "owned"}}
            binding = proxy.binding_for(ownership, record, "a" * 32)
            proxy.write_exclusive(state / "vnc-relay-owner.json", binding)
            with patch.dict(sys.modules, {"run": run}), patch.object(run, "owned", return_value=record) as owned:
                proxy.request_stop(ownership)
                owned.assert_called_once_with(ownership)
                self.assertTrue(proxy.stop_requested(state, binding))
                changed = dict(record, container_id="other-cid")
                with patch.object(run, "owned", return_value=changed):
                    with self.assertRaisesRegex(RuntimeError, "ownership mismatch"):
                        proxy.request_stop(ownership)
            request = state / "vnc-stop-request.json"
            self.assertEqual(request.stat().st_mode & 0o777, 0o600)
            request.write_text(json.dumps(dict(binding, relay_nonce="b" * 32)))
            with self.assertRaisesRegex(RuntimeError, "another ownership"):
                proxy.stop_requested(state, binding)
            with patch.dict(sys.modules, {"run": run}), patch.object(run, "owned", return_value=record):
                with self.assertRaisesRegex(RuntimeError, "another ownership"):
                    proxy.request_stop(ownership)

    def test_changed_daemon_refuses_relay_stop_before_read_or_write(self):
        proxy = module("vnc_proxy")
        run = module("run")
        import sys
        with patch.dict(sys.modules, {"run": run}), patch.object(run, "owned", side_effect=RuntimeError("identity changed")), \
             patch.object(proxy, "read_control") as read, patch.object(proxy, "write_exclusive") as write:
            with self.assertRaisesRegex(RuntimeError, "identity changed"):
                proxy.request_stop(Path("unused"))
            read.assert_not_called()
            write.assert_not_called()

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
            rpc.failure = threading.Event()
            rpc.rpc_bytes = 0
            rpc.rpc_log = (rpc.evidence / "rpc.jsonl").open("wb")
            self.addCleanup(rpc.rpc_log.close)
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

    def test_noisy_stderr_is_capped_terminated_and_cannot_retain_qualification(self):
        observer = module("observer")
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            evidence = root / "evidence"
            evidence.mkdir(mode=0o700)
            (evidence / "qualification.json").write_text('{"passed":true}')
            binary = root / "noisy"
            binary.write_text(f"#!{sys.executable}\nimport os,signal\nsignal.signal(signal.SIGTERM,signal.SIG_IGN)\nwhile True: os.write(2,b'x'*4096)\n")
            binary.chmod(0o700)
            with patch.object(observer, "MAX_STDERR_BYTES", 1024):
                rpc = observer.Rpc(binary, evidence)
                try:
                    self.assertTrue(rpc.failure.wait(timeout=3))
                    rpc.process.wait(timeout=5)
                    # A response racing the collector cannot restore a passing receipt.
                    (evidence / "qualification.json").write_text('{"passed":true}')
                    with self.assertRaisesRegex(RuntimeError, "stderr exceeded"):
                        rpc.close()
                    with self.assertRaisesRegex(RuntimeError, "stderr exceeded"):
                        rpc.close()
                    self.assertIsNotNone(rpc.process.poll())
                    self.assertEqual(rpc.process.returncode, -signal.SIGKILL)
                    self.assertEqual((evidence / "server.stderr").stat().st_size, 1024)
                    self.assertFalse((evidence / "qualification.json").exists())
                    self.assertTrue(all(not reader.is_alive() for reader in rpc.readers))
                finally:
                    if not rpc.closed:
                        try:
                            rpc.close()
                        except RuntimeError:
                            pass

    def test_rpc_evidence_cap_refuses_response_before_writing_or_qualifying(self):
        observer = module("observer")
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            evidence = root / "evidence"
            evidence.mkdir(mode=0o700)
            binary = root / "candidate"
            binary.write_text(f"#!{sys.executable}\nimport json,sys\nfor line in sys.stdin:\n v=json.loads(line); print(json.dumps({{'id':v['id'],'result':'x'*512}}),flush=True)\n")
            binary.chmod(0o700)
            with patch.object(observer, "MAX_RPC_BYTES", 128):
                rpc = observer.Rpc(binary, evidence)
                with self.assertRaisesRegex(RuntimeError, "reply evidence exceeded"):
                    rpc.request("tools/list", {})
                with self.assertRaisesRegex(RuntimeError, "reply evidence exceeded"):
                    rpc.close()
                self.assertIsNotNone(rpc.process.poll())
                self.assertLessEqual((evidence / "rpc.jsonl").stat().st_size, 128)

    def test_normal_rpc_drains_stderr_and_closes_owned_process_idempotently(self):
        observer = module("observer")
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            evidence = root / "evidence"
            evidence.mkdir(mode=0o700)
            binary = root / "candidate"
            binary.write_text(f"#!{sys.executable}\nimport json,sys\nsys.stderr.write('bounded stderr\\n');sys.stderr.flush()\nfor line in sys.stdin:\n v=json.loads(line); print(json.dumps({{'id':v['id'],'result':{{}}}}),flush=True)\n")
            binary.chmod(0o700)
            rpc = observer.Rpc(binary, evidence)
            try:
                self.assertEqual(rpc.request("tools/list", {})["result"], {})
            finally:
                rpc.close()
            rpc.close()
            self.assertEqual(rpc.process.returncode, 0)
            self.assertEqual((evidence / "server.stderr").read_text(), "bounded stderr\n")

    def test_sequential_clients_exhaust_a_finite_child_and_accept_budget(self):
        proxy = module("vnc_proxy")
        for accepts, children, expected in [(3, 8, 3), (8, 2, 2)]:
            with self.subTest(accepts=accepts, children=children), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                host = root / "relay"
                host.mkdir(mode=0o700)
                ownership = root / "ownership.json"
                record = {"container_id": "owned", "daemon_id": "daemon", "labels": {"owner": "fixture"}}
                clients = []
                listener = Mock()
                listener.__enter__ = Mock(return_value=listener)
                listener.__exit__ = Mock(return_value=False)
                listener.getsockname.return_value = ("127.0.0.1", 1234)
                def accept():
                    if clients:
                        self.assertTrue(clients[-1].finished.wait(timeout=2))
                    client = Mock()
                    client.finished = threading.Event()
                    client.close.side_effect = client.finished.set
                    clients.append(client)
                    return client, ("127.0.0.1", 4321)
                listener.accept.side_effect = accept
                process = Mock()
                process.poll.return_value = 0
                with proxy.safe_io.PrivateDirectory(host) as directory, \
                        patch.object(proxy.socket, "socket", return_value=listener), \
                        patch.object(proxy.signal, "signal"), patch.object(proxy, "validate_ownership", return_value=record), \
                        patch.object(proxy.subprocess, "Popen", return_value=process) as spawn, \
                        patch.object(proxy, "MAX_ACCEPTED_CLIENTS", accepts), patch.object(proxy, "MAX_CHILDREN", children):
                    proxy.serve_owned(ownership, record, directory)
                self.assertEqual(spawn.call_count, expected)
                self.assertEqual(listener.accept.call_count, expected)
                listener.close.assert_called()
                result = proxy.read_control(host / "vnc-proxy-result.json")
                self.assertEqual(result["accepted_clients"], expected)
                self.assertEqual(result["children_started"], expected)
                self.assertTrue(result["budget_exhausted"])
                self.assertTrue(result["owned_children_stopped"])
                self.assertTrue(result["owned_relay_stopped"])
                self.assertFalse((host / "vnc-address.json").exists())


if __name__ == "__main__":
    unittest.main()
