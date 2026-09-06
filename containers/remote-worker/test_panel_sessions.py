"""Linux worker tests use only private temporary directories and dedicated sockets."""

from concurrent.futures import ThreadPoolExecutor
import importlib.util
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
        self.repository.mkdir()
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
        for version in (2, True):
            marker = json.loads(original)
            marker["version"] = version
            path.write_text(json.dumps(marker))
            with self.assertRaises(MODULE.SessionError):
                self.service.status(runtime, "owned")
        path.write_text(original)
        path.chmod(0o644)
        with self.assertRaises(MODULE.SessionError):
            self.service.status(runtime, "owned")


if __name__ == "__main__":
    unittest.main()
