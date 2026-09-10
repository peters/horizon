"""Linux worker tests use only private temporary directories and dedicated sockets."""

from concurrent.futures import ThreadPoolExecutor
import importlib.util
import io
import json
import os
from pathlib import Path
import pty
import signal
import sys
import tempfile
import time
import unittest
from unittest import mock
import uuid

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("panel_session", HERE / "panel-session.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class PanelSessionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        result = MODULE.subprocess.run(["tmux", "-V"], capture_output=True, check=True, text=True, timeout=3)
        if result.stdout.strip() != "tmux 3.7c":
            raise RuntimeError("tests require the pinned worker tmux; see the remote-worker README")

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="hps-", dir="/tmp")
        self.root = Path(self.temporary.name)
        self.repository = self.root / "repo"
        self.repository.mkdir(mode=0o700)
        (self.repository / "nested space").mkdir()
        self.runtimes = [str(uuid.uuid4()), str(uuid.uuid4())]
        self.service = MODULE.PanelSessions(self.repository, self.root / "state", self.root / "sockets",
                                            HERE / "tmux.conf", "/usr/bin/env")

    def tearDown(self):
        for runtime in self.runtimes:
            self.service.tmux(runtime, "kill-server", check=False)
        self.temporary.cleanup()

    def wait_for(self, predicate):
        deadline = time.monotonic() + 4
        while time.monotonic() < deadline:
            result = predicate()
            if result:
                return result
            time.sleep(0.02)
        self.fail("task-owned fixture did not settle")

    def command(self, text):
        return [sys.executable, "-c", text]

    def tick_command(self, filename="ticks"):
        return self.command("import time\nfrom pathlib import Path\np=Path(" + repr(filename) + ")\n"
                            "while True:\n with p.open('a') as f: f.write('tick\\n')\n time.sleep(0.03)")

    def attach_and_disconnect(self, runtime, panel):
        child, master = pty.fork()
        if child == 0:
            os.environ["TERM"] = "xterm-256color"
            try:
                self.service.attach(runtime, panel)
            finally:
                os._exit(1)
        try:
            self.wait_for(lambda: str(child).encode() in self.service.tmux(
                runtime, "list-clients", "-F", "#{client_pid}", check=False).stdout.splitlines())
        finally:
            os.close(master)
            deadline = time.monotonic() + 2
            reaped = False
            while time.monotonic() < deadline:
                if os.waitpid(child, os.WNOHANG)[0] == child:
                    reaped = True
                    break
                time.sleep(0.01)
            if not reaped:
                os.kill(child, signal.SIGTERM)
                os.waitpid(child, 0)
                self.fail("task-owned attachment did not exit after PTY loss")

    def test_live_task_continues_after_last_client_disconnect_and_reuses_pid(self):
        runtime = self.runtimes[0]
        command = self.tick_command()
        first = self.service.start(runtime, "panel_a", "nested space", command)
        ticks = self.repository / "nested space" / "ticks"
        self.wait_for(ticks.exists)
        for _ in range(2):
            self.attach_and_disconnect(runtime, "panel_a")
        before = len(ticks.read_text().splitlines())
        self.wait_for(lambda: len(ticks.read_text().splitlines()) > before + 2)
        again = self.service.start(runtime, "panel_a", "nested space", command)
        self.assertEqual(first["pid"], again["pid"])
        self.assertEqual(again["state"], "running")
        self.assertEqual(self.service.tmux(runtime, "list-clients", check=False).stdout, b"")

    def request(self, operation, panel="structured", **intent):
        return {"version": 1, "operation": operation, "runtime": self.runtimes[0], "panel": panel, **intent}

    def execute(self, request):
        return MODULE.execute_request(self.service, io.BytesIO(json.dumps(request).encode()))

    def test_structured_start_and_status_reuse_the_same_task_after_disconnect(self):
        request = self.request("start", directory="nested space", argv=self.tick_command())
        first = self.execute(request)
        ticks = self.repository / "nested space" / "ticks"
        self.wait_for(ticks.exists)
        self.attach_and_disconnect(self.runtimes[0], "structured")
        before = len(ticks.read_text().splitlines())
        self.wait_for(lambda: len(ticks.read_text().splitlines()) > before + 2)
        self.assertEqual(self.execute(self.request("status"))["pid"], first["pid"])
        self.assertEqual(self.execute(request)["pid"], first["pid"])

    def test_structured_payload_keeps_literal_argv_and_does_not_persist_task_content(self):
        literals = ["", "$(touch injected);", "'quoted'", "line\nbreak", "\u00e6\u00f8\u00e5", "\\;", "#{l:..}"]
        command = self.command("import json,sys; from pathlib import Path; Path('argv').write_text(json.dumps(sys.argv[1:]))") + literals
        self.execute(self.request("start", directory=".", argv=command))
        self.wait_for(lambda: (self.repository / "argv").exists() and (self.repository / "argv").read_text() == json.dumps(literals))
        self.assertEqual(json.loads((self.repository / "argv").read_text()), literals)
        self.assertFalse((self.repository / "injected").exists())
        self.assertNotIn("touch injected", self.service.marker_path(self.runtimes[0], "structured").read_text())

    def test_structured_status_never_creates_an_absent_task(self):
        with self.assertRaises(FileNotFoundError):
            self.execute(self.request("status"))
        self.assertFalse(self.service.state.exists())
        self.assertFalse(self.service.sockets.exists())

    def test_structured_verification_retains_exact_task_after_disconnect(self):
        argv = self.tick_command()
        first = self.execute(self.request("start", directory="nested space", argv=argv))
        marker = self.service.marker_path(self.runtimes[0], "structured")
        original = marker.read_bytes(), marker.stat().st_mtime_ns
        ticks = self.repository / "nested space" / "ticks"
        self.wait_for(ticks.exists)
        self.attach_and_disconnect(self.runtimes[0], "structured")
        before = len(ticks.read_text().splitlines())
        with mock.patch.object(self.service, "start") as start, mock.patch.object(self.service, "attach") as attach:
            verified = self.execute(self.request("verify", directory="nested space", argv=argv))
            self.assertEqual(verified, first)
            start.assert_not_called()
            attach.assert_not_called()
        self.wait_for(lambda: len(ticks.read_text().splitlines()) > before + 2)
        self.assertEqual((marker.read_bytes(), marker.stat().st_mtime_ns), original)

    def test_structured_verification_survives_running_task_directory_rename(self):
        argv = self.tick_command()
        first = self.execute(self.request("start", directory="nested space", argv=argv))
        original = self.repository / "nested space"
        self.wait_for(lambda: (original / "ticks").exists())
        renamed = self.repository / "renamed"
        original.rename(renamed)
        marker = self.service.marker_path(self.runtimes[0], "structured")
        retained = marker.read_bytes(), marker.stat().st_mtime_ns
        before = len((renamed / "ticks").read_text().splitlines())
        self.assertEqual(self.execute(self.request("verify", directory="nested space", argv=argv)), first)
        self.wait_for(lambda: len((renamed / "ticks").read_text().splitlines()) > before + 2)
        self.assertEqual((marker.read_bytes(), marker.stat().st_mtime_ns), retained)
        with self.assertRaises(MODULE.SessionError):
            self.execute(self.request("verify", directory="renamed", argv=argv))
        with self.assertRaises(FileNotFoundError):
            self.execute(self.request("start", panel="new", directory="nested space", argv=argv))
        self.assertFalse(self.service.marker_path(self.runtimes[0], "new").exists())

    def test_structured_verification_survives_completed_task_repository_removal(self):
        argv = self.command("raise SystemExit(42)")
        first = self.execute(self.request("start", directory="nested space", argv=argv))
        self.wait_for(lambda: self.execute(self.request("status"))["state"] == "exited")
        (self.repository / "nested space").rmdir()
        self.repository.rmdir()
        marker = self.service.marker_path(self.runtimes[0], "structured")
        retained = marker.read_bytes(), marker.stat().st_mtime_ns
        self.assertEqual(self.execute(self.request("verify", directory="nested space", argv=argv)),
                         {"state":"exited", "panel":"structured", "pid":first["pid"], "exit_status":42})
        self.assertEqual((marker.read_bytes(), marker.stat().st_mtime_ns), retained)
        self.assertFalse(self.repository.exists())

    def test_structured_verification_rejects_changed_directory_or_literal_arguments(self):
        literals = ["", "$(touch injected);", "'quoted'", "line\nbreak", "æøå", "\\;", "#{l:..}"]
        argv = self.tick_command() + literals
        first = self.execute(self.request("start", directory=".", argv=argv))
        marker = self.service.marker_path(self.runtimes[0], "structured")
        original = marker.read_bytes(), marker.stat().st_mtime_ns
        self.assertEqual(self.execute(self.request("verify", directory=".", argv=argv)), first)
        for directory, changed in [("nested space", argv), (".", argv[:-1]), (".", argv + ["extra"]),
                                   (".", argv[:-1] + ["different"])]:
            with self.subTest(directory=directory, argument_count=len(changed)):
                with mock.patch.object(self.service, "start") as start, self.assertRaises(MODULE.SessionError):
                    self.execute(self.request("verify", directory=directory, argv=changed))
                start.assert_not_called()
        self.assertFalse((self.repository / "injected").exists())
        self.assertEqual(self.execute(self.request("status"))["pid"], first["pid"])
        self.assertEqual((marker.read_bytes(), marker.stat().st_mtime_ns), original)

    def test_structured_verification_never_creates_an_absent_task(self):
        with mock.patch.object(self.service, "start") as start, self.assertRaises(FileNotFoundError):
            self.execute(self.request("verify", directory=".", argv=self.tick_command()))
        start.assert_not_called()
        self.assertFalse(self.service.state.exists())
        self.assertFalse(self.service.sockets.exists())

    def test_structured_verification_retains_completed_task_without_reexecution(self):
        argv = self.command("from pathlib import Path; Path('runs').open('a').write('once\\n'); raise SystemExit(42)")
        self.execute(self.request("start", directory=".", argv=argv))
        def completed():
            status = self.execute(self.request("status"))
            return status if status["state"] == "exited" else None

        finished = self.wait_for(completed)
        for _ in range(2):
            self.assertEqual(self.execute(self.request("verify", directory=".", argv=argv)), finished)
        self.assertEqual(finished["exit_status"], 42)
        self.assertEqual((self.repository / "runs").read_text(), "once\n")

    def test_structured_verification_of_lost_server_never_starts_replacement(self):
        argv = self.tick_command()
        self.execute(self.request("start", directory=".", argv=argv))
        marker = self.service.marker_path(self.runtimes[0], "structured").read_bytes()
        self.service.tmux(self.runtimes[0], "kill-server")
        verified = self.execute(self.request("verify", directory=".", argv=argv))
        self.assertEqual(verified, {"state": "unavailable", "panel": "structured"})
        self.assertNotEqual(self.service.tmux(self.runtimes[0], "list-sessions", check=False).returncode, 0)
        self.assertEqual(self.service.marker_path(self.runtimes[0], "structured").read_bytes(), marker)

    def test_structured_verification_rejects_incomplete_or_invalid_intent_before_inspection(self):
        valid = self.request("verify", directory=".", argv=self.tick_command())
        cases = [self.request("verify"), {**valid, "argv": "private task"},
                 {**valid, "directory": None}, {**valid, "unexpected": "private task"}]
        for request in cases:
            with mock.patch.object(self.service, "status") as status, mock.patch.object(self.service, "start") as start:
                with self.assertRaises(MODULE.SessionError) as caught:
                    self.execute(request)
                status.assert_not_called()
                start.assert_not_called()
                self.assertNotIn("private task", str(caught.exception))
        self.assertFalse(self.service.state.exists())
        self.assertFalse(self.service.sockets.exists())

    def test_structured_request_rejects_unknown_fields_versions_and_types_before_launch(self):
        valid = self.request("start", directory=".", argv=self.tick_command())
        cases = [None, [], {}, {**valid, "version": True}, {**valid, "version": 2},
                 {**valid, "runtime": 7}, {**valid, "panel": []}, {**valid, "directory": None},
                 {**valid, "argv": "sh"}, {**valid, "argv": ["sh", None]}, {**valid, "extra": "secret"},
                 {**valid, "operation": "attach"}, {**valid, "operation": "status"},
                 {**valid, "argv": []}, {**valid, "argv": ["-invalid"]},
                 {**valid, "argv": ["sh", "\0"]}, {**valid, "directory": ".."},
                 {**valid, "runtime": "invalid"}, {**valid, "panel": "panel;other"}]
        for request in cases:
            with self.subTest(request=request), self.assertRaises(MODULE.SessionError):
                self.execute(request)
        self.assertFalse(self.service.state.exists())
        self.assertFalse(self.service.sockets.exists())

    def test_structured_request_rejects_malformed_duplicate_and_oversized_input(self):
        valid = json.dumps(self.request("start", directory=".", argv=self.tick_command())).encode()
        duplicate = valid[:-1] + b',"version":1}'
        cases = [b"", b"secret payload", b"\xff", b"[]", b'{} {}',
                 duplicate, b"[" * 2000 + b"]" * 2000,
                 b" " * (MODULE.MAX_REQUEST_BYTES + 1)]
        for raw in cases:
            with mock.patch.object(self.service, "start") as start, mock.patch.object(self.service, "status") as status:
                with self.subTest(length=len(raw)), self.assertRaises(MODULE.SessionError) as caught:
                    MODULE.execute_request(self.service, io.BytesIO(raw))
                start.assert_not_called()
                status.assert_not_called()
            self.assertNotIn("secret payload", str(caught.exception))
        self.assertFalse(self.service.state.exists())

    def test_structured_input_read_is_bounded(self):
        stream = mock.Mock()
        stream.read.return_value = b" " * (MODULE.MAX_REQUEST_BYTES + 1)
        with self.assertRaises(MODULE.SessionError):
            MODULE.execute_request(self.service, stream)
        stream.read.assert_called_once_with(MODULE.MAX_REQUEST_BYTES + 1)

    def test_structured_cli_errors_do_not_echo_stdin_and_legacy_help_remains_available(self):
        result = MODULE.subprocess.run([sys.executable, str(HERE / "panel-session.py"), "request"],
                                       input=b"synthetic-private-request", capture_output=True, timeout=3, check=False)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, b"")
        self.assertNotIn(b"synthetic-private-request", result.stderr)
        self.assertNotIn(b"Traceback", result.stderr)
        for operation in ("start", "status", "attach", "request"):
            result = MODULE.subprocess.run([sys.executable, str(HERE / "panel-session.py"), operation, "--help"],
                                           capture_output=True, timeout=3, check=False)
            self.assertEqual(result.returncode, 0)

    def test_exited_task_is_retained_and_not_reexecuted_on_start_or_attach(self):
        runtime = self.runtimes[0]
        command = self.command("from pathlib import Path; p=Path('runs'); p.open('a').write('once\\n'); raise SystemExit(42)")
        first = self.service.start(runtime, "finished", ".", command)
        self.wait_for(lambda: self.service.status(runtime, "finished")["exit_status"] == 42)
        self.attach_and_disconnect(runtime, "finished")
        again = self.service.start(runtime, "finished", ".", command)
        self.assertEqual(first["pid"], again["pid"])
        self.assertEqual(again["exit_status"], 42)
        self.assertEqual((self.repository / "runs").read_text(), "once\n")

    def test_claim_without_creation_stays_unavailable_and_never_restarts(self):
        runtime = self.runtimes[0]
        command = self.tick_command()
        _, intent = self.service.launch_intent(".", command)
        marker = {"version": 1, "runtime": runtime, "panel": "interrupted", "nonce": uuid.uuid4().hex, "intent": intent}
        self.assertTrue(self.service.publish_marker(runtime, "interrupted", marker))
        self.assertEqual(self.service.start(runtime, "interrupted", ".", command)["state"], "unavailable")
        with self.assertRaises(MODULE.SessionError):
            self.service.attach(runtime, "interrupted")
        self.assertFalse((self.repository / "ticks").exists())

    def test_signal_terminated_task_is_retained_without_reporting_success(self):
        runtime = self.runtimes[0]
        command = self.command("import os,signal; os.kill(os.getpid(), signal.SIGTERM)")
        self.service.start(runtime, "signalled", ".", command)
        self.wait_for(lambda: self.service.status(runtime, "signalled")["state"] == "exited")
        result = self.service.start(runtime, "signalled", ".", command)
        self.assertEqual(result["state"], "exited")
        self.assertNotEqual(result["exit_status"], 0)

    def test_status_works_with_a_minimal_non_utf8_worker_environment(self):
        environment = {"PATH": os.environ["PATH"], "LC_ALL": "C", "TERM": "xterm-256color", "SHELL": "/bin/bash"}
        with mock.patch.dict(os.environ, environment, clear=True):
            status = self.service.start(self.runtimes[0], "minimal", ".", self.tick_command())
            self.assertEqual(status["state"], "running")
            self.assertEqual(self.service.status(self.runtimes[0], "minimal")["pid"], status["pid"])

    def test_concurrent_starts_have_one_claim_and_one_task(self):
        runtime = self.runtimes[0]
        command = self.command("from pathlib import Path; import time; Path('runs').open('a').write('once\\n'); time.sleep(30)")
        with ThreadPoolExecutor(max_workers=2) as executor:
            results = list(executor.map(lambda _: self.service.start(runtime, "race", ".", command), range(2)))
        self.wait_for(lambda: (self.repository / "runs").exists() and (self.repository / "runs").read_text() == "once\n")
        self.assertEqual((self.repository / "runs").read_text(), "once\n")
        self.assertTrue(all(result["state"] in ("running", "unavailable") for result in results))
        self.assertEqual(len(self.service.tmux(runtime, "list-sessions").stdout.splitlines()), 1)

    def test_argument_boundaries_are_literal_and_changed_intent_is_rejected(self):
        runtime = self.runtimes[0]
        literal = "$(touch injected); 'quoted' and spaces"
        command = self.command("import json,sys; from pathlib import Path; Path('argv').write_text(json.dumps(sys.argv[1:]))") + [literal, ""]
        self.service.start(runtime, "literal", ".", command)
        self.wait_for(lambda: (self.repository / "argv").exists() and (self.repository / "argv").read_text() == json.dumps([literal, ""]))
        self.assertEqual(json.loads((self.repository / "argv").read_text()), [literal, ""])
        self.assertFalse((self.repository / "injected").exists())
        marker = self.service.marker_path(runtime, "literal").read_text()
        self.assertNotIn(literal, marker)
        with self.assertRaises(MODULE.SessionError):
            self.service.start(runtime, "literal", ".", self.tick_command())

    def test_invalid_ids_and_escaping_directories_have_no_launch_side_effect(self):
        runtime = self.runtimes[0]
        outside = self.root / "outside"
        outside.mkdir()
        (self.repository / "escape").symlink_to(outside)
        cases = [("bad", "panel", "."), (runtime, "panel:other", "."),
                 (runtime, "panel", ".."), (runtime, "panel", "/tmp"), (runtime, "panel", "escape")]
        for runtime_id, panel, directory in cases:
            with self.assertRaises(MODULE.SessionError):
                self.service.start(runtime_id, panel, directory, self.tick_command())
        self.assertFalse(self.service.state.exists())

    def test_two_runtimes_and_panels_do_not_share_ownership_or_processes(self):
        first = self.service.start(self.runtimes[0], "same", ".", self.tick_command("a"))
        second = self.service.start(self.runtimes[1], "same", ".", self.tick_command("b"))
        third = self.service.start(self.runtimes[0], "other", ".", self.tick_command("c"))
        self.assertEqual(len({first["pid"], second["pid"], third["pid"]}), 3)
        self.service.tmux(self.runtimes[0], "set-environment", "-t", "=horizon-panel-same", "HORIZON_PANEL_INSTANCE", "wrong")
        with self.assertRaises(MODULE.SessionError):
            self.service.status(self.runtimes[0], "same")
        self.assertEqual(self.service.status(self.runtimes[1], "same")["state"], "running")

    def test_tmux_command_separators_remain_literal_arguments(self):
        runtime = self.runtimes[0]
        directory = self.repository / "trailing;"
        directory.mkdir()
        literals = ["payload;", ";", "display-message", "-p", "unexpected", "\\;", "\\\\;", "#comment", "#{l:literal}", "line\nbreak", ""]
        command = self.command("import json,sys; from pathlib import Path; Path('argv').write_text(json.dumps(sys.argv[1:]))") + literals
        self.service.start(runtime, "separators", "trailing;", command)
        self.wait_for(lambda: (directory / "argv").exists() and (directory / "argv").read_text() == json.dumps(literals))
        self.assertEqual(json.loads((directory / "argv").read_text()), literals)
        self.assertEqual(len(self.service.tmux(runtime, "list-sessions").stdout.splitlines()), 1)

    def test_tmux_directory_formats_cannot_escape_the_repository(self):
        runtime = self.runtimes[0]
        directory = self.repository / "#{l:..}"
        directory.mkdir()
        command = self.command("from pathlib import Path; Path('cwd').write_text(str(Path.cwd()))")
        self.service.start(runtime, "format", "#{l:..}", command)
        self.wait_for(lambda: ((directory / "cwd").exists() and (directory / "cwd").read_text() == str(directory))
                      or (self.root / "cwd").exists())
        self.assertFalse((self.root / "cwd").exists())
        self.assertEqual((directory / "cwd").read_text(), str(directory))

    def test_server_loss_between_verification_and_attach_cannot_run_startup_config(self):
        runtime = self.runtimes[0]
        self.service.start(runtime, "lost", ".", self.tick_command())
        observed = self.service.status(runtime, "lost")
        config = self.root / "unexpected.conf"
        config.write_text("new-session -d -s horizon-panel-lost /bin/sleep 30\n")
        self.service.config = config

        def lose_server(*_):
            self.service.tmux(runtime, "kill-server")
            return observed

        def execute(_, arguments):
            result = MODULE.subprocess.run(arguments, stdin=MODULE.subprocess.DEVNULL,
                                           capture_output=True, timeout=3, check=False)
            self.assertNotEqual(result.returncode, 0)

        with mock.patch.object(self.service, "status", side_effect=lose_server), mock.patch.object(MODULE.os, "execvp", side_effect=execute):
            self.service.attach(runtime, "lost")
        self.assertNotEqual(self.service.tmux(runtime, "has-session", "-t", "=horizon-panel-lost", check=False).returncode, 0)

    def test_verified_server_loss_does_not_authorize_a_replacement_task(self):
        runtime = self.runtimes[0]
        command = self.tick_command()
        self.service.start(runtime, "lost", ".", command)
        self.service.tmux(runtime, "kill-server")
        self.assertEqual(self.service.start(runtime, "lost", ".", command)["state"], "unavailable")

    def test_retained_markers_prevent_replay_after_runtime_filesystem_replacement(self):
        runtime = self.runtimes[0]
        self.service.require_existing_state = True
        MODULE.private_directory(self.service.state)
        running = self.tick_command()
        completed = self.command("from pathlib import Path; Path('runs').open('a').write('once\\n'); raise SystemExit(42)")
        self.service.start(runtime, "running", ".", running)
        self.service.start(runtime, "completed", ".", completed)
        self.wait_for(lambda: (self.repository / "ticks").exists())
        self.wait_for(lambda: self.service.status(runtime, "completed")["state"] == "exited")
        markers = {panel: self.service.marker_path(runtime, panel).read_bytes() for panel in ("running", "completed")}
        self.service.tmux(runtime, "kill-server")
        self.service = MODULE.PanelSessions(self.repository, self.service.state, self.root / "replacement-sockets",
                                            HERE / "tmux.conf", "/usr/bin/env", require_existing_state=True)
        ticks = (self.repository / "ticks").read_bytes()
        for panel, command in (("running", running), ("completed", completed)):
            expected = {"state": "unavailable", "panel": panel}
            self.assertEqual(expected, self.service.status(runtime, panel))
            self.assertEqual(expected, self.service.start(runtime, panel, ".", command))
            self.assertEqual(expected, self.service.verify(runtime, panel, ".", command))
            with self.assertRaises(MODULE.SessionError):
                self.service.attach(runtime, panel)
            self.assertEqual(markers[panel], self.service.marker_path(runtime, panel).read_bytes())
        self.assertEqual("once\n", (self.repository / "runs").read_text())
        self.assertEqual(ticks, (self.repository / "ticks").read_bytes())
        self.assertFalse(self.service.sockets.exists())

    def test_required_retained_root_is_never_recreated_by_start(self):
        self.service.require_existing_state = True
        for operation in (self.service.start, self.service.verify):
            with self.assertRaises(FileNotFoundError):
                operation(self.runtimes[0], "absent", ".", self.tick_command())
        self.assertFalse(self.service.state.exists())
        self.assertFalse(self.service.sockets.exists())

    def test_linked_or_insecure_retained_parent_fails_before_task_creation(self):
        parent = self.root / "retained-parent"
        MODULE.private_directory(parent)
        self.service.state = parent / "panels"
        self.service.require_existing_state = True
        MODULE.private_directory(self.service.state)
        parent.chmod(0o755)
        with self.assertRaises(MODULE.SessionError):
            self.service.start(self.runtimes[0], "untrusted", ".", self.tick_command())
        parent.chmod(0o700)
        retained = self.root / "original-parent"
        parent.rename(retained)
        parent.symlink_to(retained)
        with self.assertRaises(MODULE.SessionError):
            self.service.start(self.runtimes[0], "untrusted", ".", self.tick_command())
        self.assertEqual([], list((retained / "panels").iterdir()))
        self.assertFalse(self.service.sockets.exists())

    def test_foreign_owned_private_state_is_rejected(self):
        self.service.start(self.runtimes[0], "owned", ".", self.tick_command())
        before = self.service.marker_path(self.runtimes[0], "owned").read_bytes()
        with mock.patch.object(MODULE.os, "geteuid", return_value=os.geteuid() + 1):
            with self.assertRaises(MODULE.SessionError):
                self.service.status(self.runtimes[0], "owned")
        self.assertEqual(before, self.service.marker_path(self.runtimes[0], "owned").read_bytes())

    def test_unowned_sessions_and_future_or_insecure_markers_fail_closed(self):
        runtime = self.runtimes[0]
        MODULE.private_directory(self.service.sockets)
        self.service.tmux(runtime, "new-session", "-d", "-s", "horizon-panel-unowned", "/bin/sleep", "30")
        with self.assertRaises(MODULE.SessionError):
            self.service.start(runtime, "unowned", ".", self.tick_command())
        self.assertFalse(self.service.state.exists())
        self.service.start(runtime, "owned", ".", self.tick_command())
        path = self.service.marker_path(runtime, "owned")
        original = path.read_text()
        for version in (2, 3, True):
            marker = json.loads(original)
            marker["version"] = version
            path.write_text(json.dumps(marker))
            with self.assertRaises(MODULE.SessionError):
                self.service.status(runtime, "owned")
        path.write_text(original)
        path.chmod(0o644)
        with self.assertRaises(MODULE.SessionError):
            self.service.status(runtime, "owned")

    def prepared_request(self, panel="prepared", directory=".", argv=None):
        selection = {"version": 1, "destination": "published", "intake": {
            "version": 1, "workspace_local_id": "fixture-workspace", "workflow_id": self.runtimes[1],
            "job_id": self.runtimes[0], "runtime_generation": 1, "worker_resource_id": "fixture-worker",
            "client_key_sha256": "a" * 64,
            "source": {"repository": "fixture/repository", "commit": "a" * 40, "branch": None},
            "pack": {"sha256": "b" * 64, "encoded_bytes": 32},
            "overlay": {"sha256": "c" * 64, "encoded_bytes": 1}}}
        return self.request("start-prepared", panel, directory=directory, argv=argv or self.tick_command(), repository=selection)

    def prepared_helper(self, operation, selection):
        # This seam tests panel admission, not Rust canonicalization or full setup.
        same = selection == self.prepared_request()["repository"]
        root = None
        if operation == "setup-checkout":
            info = self.repository.stat()
            root = {"path": str(self.repository), "device": info.st_dev, "inode": info.st_ino}
        self.assertIn(operation, ("setup-binding", "setup-checkout"))
        return {"version": 1, "binding_sha256": ("a" if same else "b") * 64,
                "runtime": selection["intake"]["job_id"], "root": root, "reason": None}

    def test_prepared_panels_share_evolving_commits_index_and_working_tree(self):
        environment = {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null"}
        def git(*arguments):
            return MODULE.subprocess.run(["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", *arguments],
                cwd=self.repository, env=environment, capture_output=True, timeout=5, check=True).stdout
        git("init", "--template=", "-q")
        content = self.repository / "tracked"
        content.write_text("base")
        git("add", "tracked")
        git("commit", "-qm", "fixture")
        old_head = git("rev-parse", "HEAD")
        with mock.patch.object(MODULE, "repository_selection", side_effect=self.prepared_helper):
            first = self.execute(self.prepared_request("first"))
            self.wait_for(lambda: (self.repository / "ticks").exists())
            content.write_text("new commit")
            git("commit", "-qam", "development")
            self.assertNotEqual(git("rev-parse", "HEAD"), old_head)
            content.write_text("staged")
            git("add", "tracked")
            content.write_text("working")
            second = self.execute(self.prepared_request("second", argv=self.command(
                "from pathlib import Path; import time; Path('seen').write_text(Path('tracked').read_text()); time.sleep(30)")))
            self.wait_for(lambda: (self.repository / "seen").exists())
            self.assertEqual((self.repository / "seen").read_text(), "working")
            self.assertEqual(git("show", ":tracked"), b"staged")
            self.assertNotEqual(first["pid"], second["pid"])
            self.assertEqual(self.service.status(self.runtimes[0], "first")["pid"], first["pid"])
            markers = [self.service.read_marker(self.runtimes[0], panel) for panel in ("first", "second")]
            self.assertEqual([marker["repository"] for marker in markers], ["a" * 64] * 2)

    def test_prepared_retained_task_survives_repository_removal_without_inspection(self):
        request = self.prepared_request(directory="nested space", argv=self.command("raise SystemExit(42)"))
        with mock.patch.object(MODULE, "repository_selection", side_effect=self.prepared_helper) as helper:
            first = self.execute(request)
            self.wait_for(lambda: self.service.status(self.runtimes[0], "prepared")["state"] == "exited")
            marker = self.service.marker_path(self.runtimes[0], "prepared")
            before = marker.read_bytes(), marker.stat().st_mtime_ns
            (self.repository / "nested space").rmdir()
            self.repository.rmdir()
            helper.reset_mock()
            self.assertEqual(self.execute(request)["pid"], first["pid"])
            self.assertEqual(self.service.verify(self.runtimes[0], "prepared", "nested space", request["argv"])["exit_status"], 42)
            self.attach_and_disconnect(self.runtimes[0], "prepared")
            self.assertEqual([call.args[0] for call in helper.call_args_list], ["setup-binding"])
            self.assertEqual((marker.read_bytes(), marker.stat().st_mtime_ns), before)

    def test_prepared_binding_and_legacy_marker_mismatches_never_launch(self):
        request = self.prepared_request()
        with mock.patch.object(MODULE, "repository_selection", side_effect=self.prepared_helper) as helper:
            self.execute(request)
            request["repository"]["destination"] = "different"
            with self.assertRaises(MODULE.SessionError):
                self.execute(request)
            with self.assertRaises(MODULE.SessionError):
                self.service.start(self.runtimes[0], "prepared", ".", request["argv"])
            self.service.start(self.runtimes[0], "legacy", ".", request["argv"])
            helper.reset_mock()
            with self.assertRaises(MODULE.SessionError):
                self.execute(self.prepared_request("legacy"))
            self.assertEqual([call.args[0] for call in helper.call_args_list], ["setup-binding"])
            self.assertEqual(len(self.service.tmux(self.runtimes[0], "list-sessions").stdout.splitlines()), 2)

    def test_prepared_uncertain_claim_and_server_loss_never_replay(self):
        request = self.prepared_request()
        with mock.patch.object(MODULE, "repository_selection", side_effect=self.prepared_helper) as helper:
            # Simulate interruption after the same irreversible marker publication.
            intent = self.service.intent_digest(".", request["argv"])
            self.service.publish_marker(self.runtimes[0], "prepared", {"version": 2, "runtime": self.runtimes[0],
                "panel": "prepared", "nonce": uuid.uuid4().hex, "intent": intent, "repository": "a" * 64})
            self.assertEqual(self.execute(request)["state"], "unavailable")
            self.assertFalse((self.repository / "ticks").exists())
            running = self.prepared_request("running")
            self.execute(running)
            self.wait_for(lambda: (self.repository / "ticks").exists())
            self.service.tmux(self.runtimes[0], "kill-server")
            helper.reset_mock()
            before = (self.repository / "ticks").read_bytes()
            self.assertEqual(self.execute(running)["state"], "unavailable")
            self.assertEqual((self.repository / "ticks").read_bytes(), before)
            self.assertEqual([call.args[0] for call in helper.call_args_list], ["setup-binding"])

    def test_prepared_lost_start_response_retains_the_one_actual_task(self):
        request = self.prepared_request()
        with mock.patch.object(MODULE, "repository_selection", side_effect=self.prepared_helper) as helper:
            with mock.patch.object(self.service, "status", side_effect=MODULE.SessionError("lost response")), self.assertRaises(MODULE.SessionError):
                self.execute(request)
            self.wait_for(lambda: (self.repository / "ticks").exists())
            first = self.service.status(self.runtimes[0], "prepared")
            helper.reset_mock()
            self.assertEqual(self.execute(request)["pid"], first["pid"])
            self.assertEqual([call.args[0] for call in helper.call_args_list], ["setup-binding"])

    def test_prepared_concurrent_starts_claim_once(self):
        command = self.command("from pathlib import Path; import time; Path('runs').open('a').write('once\\n'); time.sleep(30)")
        request = self.prepared_request(argv=command)
        with mock.patch.object(MODULE, "repository_selection", side_effect=self.prepared_helper):
            with ThreadPoolExecutor(max_workers=2) as executor:
                results = list(executor.map(lambda _: self.execute(request), range(2)))
        self.wait_for(lambda: (self.repository / "runs").exists() and (self.repository / "runs").read_text() == "once\n")
        self.assertTrue(all(result["state"] in ("running", "unavailable") for result in results))
        self.assertEqual(len(self.service.tmux(self.runtimes[0], "list-sessions").stdout.splitlines()), 1)

    def test_prepared_failure_identity_drift_and_escape_have_no_marker(self):
        outside = self.root / "outside"
        outside.mkdir()
        (self.repository / "escape").symlink_to(outside)
        for fault in ("failure", "runtime", "binding", "inode", "mode", "escape"):
            request = self.prepared_request(directory="escape" if fault == "escape" else ".")
            def helper(operation, selection):
                response = self.prepared_helper(operation, selection)
                if operation == "setup-checkout":
                    if fault == "failure":
                        raise MODULE.SessionError("unconfirmed setup")
                    if fault in ("runtime", "binding"):
                        response["runtime" if fault == "runtime" else "binding_sha256"] = "different"
                    if fault == "inode":
                        response["root"]["inode"] += 1
                    if fault == "mode":
                        self.repository.chmod(0o755)
                return response
            try:
                with mock.patch.object(MODULE, "repository_selection", side_effect=helper), self.assertRaises(MODULE.SessionError):
                    self.execute(request)
                self.assertFalse(self.service.state.exists())
                self.assertFalse(self.service.sockets.exists())
            finally:
                self.repository.chmod(0o700)

    def test_fixed_repository_helper_uses_literal_bounded_commands_and_rejects_bad_responses(self):
        selected = self.prepared_request()["repository"]
        reply = self.prepared_helper("setup-binding", selected)
        completed = MODULE.subprocess.CompletedProcess([], 0, json.dumps(reply).encode() + b"\n", b"")
        with mock.patch.object(MODULE.subprocess, "run", return_value=completed) as run:
            self.assertEqual(MODULE.repository_selection("setup-binding", selected), reply)
            self.assertEqual(run.call_args.args[0], ["/usr/local/bin/horizon-repository", "setup-binding"])
            self.assertEqual(run.call_args.kwargs["env"], {"PATH": "/usr/bin:/bin", "LC_ALL": "C"})
            self.assertEqual(json.loads(run.call_args.kwargs["input"]), selected)
            for status, stdout, stderr in [(1, completed.stdout, b""), (0, completed.stdout, b"error"),
                    (0, b"x" * (16 * 1024 + 1), b""), (0, b"{}\n", b"")]:
                run.return_value = MODULE.subprocess.CompletedProcess([], status, stdout, stderr)
                with self.assertRaises(MODULE.SessionError):
                    MODULE.repository_selection("setup-binding", selected)
            run.reset_mock()
            with self.assertRaises(MODULE.SessionError):
                MODULE.repository_selection("setup", selected)
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
