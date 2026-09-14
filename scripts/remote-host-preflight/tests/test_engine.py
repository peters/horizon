"""Deterministic tests for the read-only Linux host preflight."""
from harness import *  # noqa: F401,F403
class EngineFailures(Harness):
    def test_no_engine_found(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture.pop("podman_info", None)
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("no usable container engine", by_id["container_engine"]["detail"])

    def test_docker_daemon_access_denied(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = docker_daemon_down(
            "Got permission denied while trying to connect to the Docker daemon "
            "socket at unix:///var/run/docker.sock")
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("docker present but probe failed", by_id["container_engine"]["detail"])
        self.assertIn("permission denied", by_id["container_engine"]["detail"].lower())

    def test_podman_skipped_when_local_service_is_not_running(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_info"] = podman_ok()
        code, report, executor = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("podman local service is not running", by_id["container_engine"]["detail"])
        self.assertIn(["podman", "--version"], executor.seen)
        self.assertFalse(any(argv and argv[0] == "podman" and "info" in argv
                             for argv in executor.seen))

    def test_podman_uses_uid_runtime_dir_when_xdg_unset(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = {
            "stdout": "/run/user/1000/podman/podman.sock\n"}
        fixture["podman_info"] = podman_ok()
        with mock.patch.object(os, "getuid", return_value=1000):
            saved = os.environ.pop("XDG_RUNTIME_DIR", None)
            try:
                code, report, executor = self.run_main(fixture)
            finally:
                if saved is not None:
                    os.environ["XDG_RUNTIME_DIR"] = saved
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["value"], "podman 4.9.0")
        self.assertIn(
            ["podman", "--remote=true", "--url",
             "unix:///run/user/1000/podman/podman.sock",
             "info", "--format", "{{.Version.Version}}"],
            executor.seen)

    def test_unreadable_xdg_socket_does_not_hide_system_socket(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = {"stdout": "/run/podman/podman.sock\n"}
        fixture["podman_info"] = podman_ok()
        code, report, executor = self.run_main(fixture)
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["value"], "podman 4.9.0")
        self.assertIn(
            ["podman", "--remote=true", "--url", "unix:///run/podman/podman.sock",
             "info", "--format", "{{.Version.Version}}"],
            executor.seen)

    def test_podman_socket_unreadable_is_not_absent(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = {"exit_code": 4, "stderr": "unreadable\n"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("unreadable", by_id["container_engine"]["detail"])
        self.assertNotIn("not running", by_id["container_engine"]["detail"])

    def test_podman_missing_binary_is_tool_not_present(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        code, report, executor = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("podman: tool not present", by_id["container_engine"]["detail"])
        self.assertFalse(any(argv and argv[0] == "podman" and "info" in argv
                             for argv in executor.seen))

    def test_podman_nonzero_info_preserves_stderr(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = podman_socket_ok()
        fixture["podman_info"] = {
            "exit_code": 1, "stdout": "", "stderr": "permission denied"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("permission denied", by_id["container_engine"]["detail"])
        self.assertNotIn("malformed version", by_id["container_engine"]["detail"])

    def test_podman_empty_version_is_unusable(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = podman_socket_ok()
        fixture["podman_info"] = {"stdout": "\n"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")

    def test_podman_multiline_version_is_unusable(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = podman_socket_ok()
        fixture["podman_info"] = {"stdout": "4.9.0\nWARN: extra line\n"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")

    def test_docker_non_string_version_is_unusable(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = {"stdout": json.dumps(
            {"Server": {"Version": ["26.1.4"], "OSType": "linux"}})}
        fixture.pop("podman_info", None)
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")

    def test_bearer_authorization_line_is_fully_redacted(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = docker_daemon_down(
            "error: Authorization: Bearer supersecrettok value")
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        detail = by_id["container_engine"]["detail"]
        self.assertNotIn("supersecrettok", detail)
        self.assertNotIn("Bearer", detail)
        self.assertIn("<redacted>", detail)

    def test_authorization_equals_bearer_is_fully_redacted(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = docker_daemon_down(
            "error: Authorization=Bearer supersecrettoken")
        _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn("supersecrettoken", text)
        self.assertNotIn("Bearer", text)
        self.assertIn("<redacted>", text)

    def test_disk_selects_longest_mount_ancestor(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["disk"] = {"stdout":
            "Filesystem     1024-blocks      Used Available Capacity Mounted on\n"
            "/dev/root        1000000000  800000000   200000000     80% /\n"
            "/dev/data        2000000000 1900000000    41943040    98% /mnt/workspaces\n"}
        fixture["workspace_dir"] = workspace_dir_ok("/mnt/workspaces")
        code, report, _ = self.run_main(fixture, workspace="/mnt/workspaces/job")
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        # /mnt/workspaces (40 GiB free) is the workspace's real filesystem,
        # not / (200 GB free).
        self.assertEqual(by_id["disk_capacity"]["value"], 41943040)
        self.assertIn("/mnt/workspaces", by_id["disk_capacity"]["detail"])

    def test_docker_malformed_json_falls_through_to_unsupported(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = {"stdout": "this is not json {{{"}
        fixture.pop("docker_info")
        fixture.pop("podman_info", None)
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")

    def test_remote_docker_endpoint_is_rejected(self):
        fixture = dict(DEFAULT_FIXTURE)
        with mock.patch.dict(os.environ, {"DOCKER_HOST": "tcp://remote-daemon:2376"}):
            code, report, executor = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("endpoint is remote", by_id["container_engine"]["detail"])
        self.assertIn("tcp://", by_id["container_engine"]["detail"])
        self.assertNotIn(list(preflight.PROBE_ARGS["docker_version"]), executor.seen)
        self.assertNotIn(list(preflight.PROBE_ARGS["docker_info"]), executor.seen)

    def test_empty_unix_endpoint_is_rejected(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_context"] = docker_context_ok(host="unix://")
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("remote", by_id["container_engine"]["detail"])

    def test_root_unix_endpoint_is_rejected(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_context"] = docker_context_ok(host="unix:///")
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("remote", by_id["container_engine"]["detail"])

    def test_explicit_docker_context_beats_docker_host(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_context"] = docker_context_ok(host="tcp://remote-daemon:2376")
        with mock.patch.dict(os.environ, {
                "DOCKER_HOST": "unix:///var/run/docker.sock",
                "DOCKER_CONTEXT": "remote"}):
            code, report, executor = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("context Host=", by_id["container_engine"]["detail"])
        self.assertTrue(any("context" in argv for argv in executor.seen))

    def test_local_docker_host_skips_context_inspect(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_context"] = {
            "exit_code": 1, "stdout": "", "stderr": "no context support"}
        with mock.patch.dict(os.environ, {"DOCKER_HOST": "unix:///var/run/docker.sock"}):
            code, report, executor = self.run_main(fixture)
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "supported")
        self.assertFalse(any("context" in argv for argv in executor.seen))

    def test_local_unix_docker_endpoint_is_accepted(self):
        fixture = dict(DEFAULT_FIXTURE)
        with mock.patch.dict(os.environ, {"DOCKER_HOST": "unix:///var/run/docker.sock"}):
            code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "supported")

    def test_rejected_docker_is_not_listed_as_also_found(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = {"stdout": json.dumps(
            {"Server": {"Version": "26.1.4", "OSType": "windows"}})}
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = podman_socket_ok()
        fixture["podman_info"] = podman_ok()
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["value"], "podman 4.9.0")
        self.assertNotIn("also found", by_id["container_engine"]["detail"])

    def test_non_linux_docker_server_os_is_rejected(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = {"stdout": json.dumps(
            {"Client": {"Version": "26.1.4"},
             "Server": {"Version": "26.1.4", "OSType": "windows"}})}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("not linux", by_id["container_engine"]["detail"])

    def test_remote_docker_context_is_rejected(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_context"] = docker_context_ok(host="tcp://remote-daemon:2376")
        code, report, executor = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("context Host=", by_id["container_engine"]["detail"])
        self.assertIn("tcp://", by_id["container_engine"]["detail"])
        self.assertNotIn(list(preflight.PROBE_ARGS["docker_version"]), executor.seen)
        self.assertNotIn(list(preflight.PROBE_ARGS["docker_info"]), executor.seen)

    def test_missing_docker_server_ostype_is_rejected(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = {"stdout": json.dumps(
            {"Client": {"Version": "26.1.4"}, "Server": {"Version": "26.1.4"}})}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("OSType missing", by_id["container_engine"]["detail"])

    def test_docker_version_os_field_is_accepted(self):
        # Live `docker version --format json` on Engine 29 uses Server.Os,
        # not Server.OSType.
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = {"stdout": json.dumps(
            {"Client": {"Version": "29.7.2"},
             "Server": {"Version": "29.7.2", "Os": "linux"}})}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "supported")
        self.assertEqual(by_id["container_engine"]["value"], "docker 29.7.2")

    def test_podman_container_connection_is_rejected(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_info"] = podman_ok()
        with mock.patch.dict(os.environ, {"CONTAINER_CONNECTION": "remote-worker"}):
            code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("CONTAINER_CONNECTION", by_id["container_engine"]["detail"])

    def test_podman_named_connection_is_rejected(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_info"] = podman_ok()
        with mock.patch.dict(os.environ, {"PODMAN_CONNECTION": "remote-worker"}):
            code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("PODMAN_CONNECTION", by_id["container_engine"]["detail"])

    def test_storage_driver_unverified_when_info_fails(self):
        cases = {
            "timeout": {"docker_info": {"timeout": True}},
            "nonzero": {"docker_info": {"exit_code": 1, "stdout": "", "stderr": "denied"}},
            "no_line": {"docker_info": {"stdout": ""}},
            "multiline": {"docker_info": {"stdout": "overlay2\nWARNING: extra\n"}},
        }
        for label, overrides in cases.items():
            with self.subTest(label=label):
                fixture = dict(DEFAULT_FIXTURE)
                fixture.update(overrides)
                code, report, _ = self.run_main(fixture)
                self.assertEqual(code, 0, label)
                by_id = {check["id"]: check for check in report["checks"]}
                self.assertEqual(by_id["container_engine"]["status"], "supported", label)
                self.assertEqual(by_id["container_storage_driver"]["status"], "unverified", label)

    def test_engine_version_strings_are_redacted(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = {"stdout": json.dumps(
            {"Server": {"Version": JWT, "OSType": "linux"}})}
        _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn(JWT, text)
        self.assertIn("<redacted>", text)

    def test_uname_truncated_is_error_not_rejection(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["os"] = {"stdout": "Linux\n"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["os_linux"]["status"], "error")
        self.assertIn("truncated", by_id["os_linux"]["detail"])

    def test_uname_timeout_includes_value_none(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["os"] = {"timeout": True}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["os_linux"]["status"], "error")
        self.assertIsNone(by_id["os_linux"].get("value"))

    def test_workspace_is_resolved_once_for_disk_and_storage(self):
        _, _, executor = self.run_main(dict(DEFAULT_FIXTURE))
        prefix = list(preflight.PROBE_ARGS["workspace_dir"])
        calls = [argv for argv in executor.seen if argv[:len(prefix)] == prefix]
        self.assertEqual(len(calls), 1)

    def test_workspace_helper_disables_bytecode(self):
        self.assertIn("-B", preflight.PROBE_ARGS["workspace_dir"])
        self.assertIn("-B", preflight.PROBE_ARGS["podman_socket"])

    def test_bounded_communicate_kills_runaway_output(self):
        proc = self.real_popen(
            [sys.executable, "-B", "-c", "import sys; sys.stdout.write('x'*200000)"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        _stdout, _stderr, overflow = preflight.bounded_communicate(proc, 5, 1024)
        self.assertIsNotNone(overflow)
        self.assertIn("exceeded", overflow)
        self.assertIsNotNone(proc.poll())

    def test_default_executor_times_out_before_hanging(self):
        pidfile = os.path.join(self.tmp.name, "probe.pid")
        script = (
            "import os,time\n"
            "open(%r,'w').write(str(os.getpid()))\n"
            "time.sleep(5)\n"
        ) % pidfile
        with mock.patch.object(subprocess, "Popen", self.real_popen):
            with self.assertRaises(subprocess.TimeoutExpired):
                preflight.default_executor(
                    [sys.executable, "-B", "-c", script], 0.2)
        with open(pidfile, encoding="utf-8") as handle:
            probe_pid = int(handle.read().strip())
        deadline = time.monotonic() + 2
        alive = True
        while time.monotonic() < deadline:
            try:
                os.kill(probe_pid, 0)
            except OSError:
                alive = False
                break
            time.sleep(0.05)
        self.assertFalse(alive)

    def test_non_linux_uses_direct_executor(self):
        import executor as execmod
        with mock.patch.object(execmod.sys, "platform", "darwin"):
            with mock.patch.object(execmod, "_watchdog_execute_probe") as watch:
                with mock.patch.object(
                        execmod, "_execute_probe",
                        return_value={"exit_code": 0, "stdout": "", "stderr": ""}) as direct:
                    result = execmod.default_executor(["true"], 1.0)
        watch.assert_not_called()
        direct.assert_called_once()
        self.assertEqual(result["exit_code"], 0)

    def test_setsid_failure_does_not_run_probe(self):
        with mock.patch.object(os, "setsid", side_effect=OSError("denied")):
            with mock.patch.object(subprocess, "Popen", self.real_popen):
                with self.assertRaises(OSError) as caught:
                    preflight.default_executor(
                        [sys.executable, "-B", "-c", "print(1)"], 1.0)
        self.assertIn("process session", str(caught.exception))

    def test_default_executor_missing_tool_is_file_not_found(self):
        with mock.patch.object(subprocess, "Popen", self.real_popen):
            with self.assertRaises(FileNotFoundError):
                preflight.default_executor(
                    ["/nonexistent-horizon-preflight-probe"], 1.0)

    def test_timeout_kills_child_that_inherited_stdout(self):
        pidfile = os.path.join(self.tmp.name, "orphan.pid")
        script = (
            "import os,time\n"
            "child=os.fork()\n"
            "if child==0:\n"
            "    open(%r,'w').write(str(os.getpid()))\n"
            "    time.sleep(30)\n"
            "    os._exit(0)\n"
            "os._exit(0)\n"
        ) % pidfile
        with mock.patch.object(subprocess, "Popen", self.real_popen):
            with self.assertRaises(subprocess.TimeoutExpired):
                preflight.default_executor(
                    [sys.executable, "-B", "-c", script], 0.4)
        deadline = time.monotonic() + 2
        grandchild = None
        while time.monotonic() < deadline:
            if os.path.exists(pidfile):
                with open(pidfile, encoding="utf-8") as handle:
                    text = handle.read().strip()
                if text.isdigit():
                    grandchild = int(text)
                    break
            time.sleep(0.05)
        self.assertIsNotNone(grandchild)
        alive = True
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            try:
                os.kill(grandchild, 0)
            except OSError:
                alive = False
                break
            time.sleep(0.05)
        self.assertFalse(alive)

    def test_timeout_kills_child_that_called_setsid(self):
        pidfile = os.path.join(self.tmp.name, "session-child.pid")
        script = (
            "import os,time\n"
            "child=os.fork()\n"
            "if child==0:\n"
            "    os.setsid()\n"
            "    open(%r,'w').write(str(os.getpid()))\n"
            "    time.sleep(30)\n"
            "    os._exit(0)\n"
            "time.sleep(5)\n"
        ) % pidfile
        with mock.patch.object(subprocess, "Popen", self.real_popen):
            with self.assertRaises(subprocess.TimeoutExpired):
                preflight.default_executor(
                    [sys.executable, "-B", "-c", script], 0.5)
        deadline = time.monotonic() + 2
        grandchild = None
        while time.monotonic() < deadline:
            if os.path.exists(pidfile):
                with open(pidfile, encoding="utf-8") as handle:
                    text = handle.read().strip()
                if text.isdigit():
                    grandchild = int(text)
                    break
            time.sleep(0.05)
        self.assertIsNotNone(grandchild)
        alive = True
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            try:
                os.kill(grandchild, 0)
            except OSError:
                alive = False
                break
            time.sleep(0.05)
        self.assertFalse(alive)

    def test_timeout_kills_orphan_after_probe_exits(self):
        pidfile = os.path.join(self.tmp.name, "orphan-session.pid")
        script = (
            "import os,time\n"
            "child=os.fork()\n"
            "if child==0:\n"
            "    os.setsid()\n"
            "    open(%r,'w').write(str(os.getpid()))\n"
            "    time.sleep(30)\n"
            "    os._exit(0)\n"
            "os._exit(0)\n"
        ) % pidfile
        with mock.patch.object(subprocess, "Popen", self.real_popen):
            with self.assertRaises(subprocess.TimeoutExpired):
                preflight.default_executor(
                    [sys.executable, "-B", "-c", script], 0.5)
        deadline = time.monotonic() + 2
        grandchild = None
        while time.monotonic() < deadline:
            if os.path.exists(pidfile):
                with open(pidfile, encoding="utf-8") as handle:
                    text = handle.read().strip()
                if text.isdigit():
                    grandchild = int(text)
                    break
            time.sleep(0.05)
        self.assertIsNotNone(grandchild)
        alive = True
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            try:
                os.kill(grandchild, 0)
            except OSError:
                alive = False
                break
            time.sleep(0.05)
        self.assertFalse(alive)

    def test_success_kills_daemonized_child(self):
        pidfile = os.path.join(self.tmp.name, "daemon-child.pid")
        script = (
            "import os,sys,time\n"
            "child=os.fork()\n"
            "if child==0:\n"
            "    os.setsid()\n"
            "    open(%r,'w').write(str(os.getpid()))\n"
            "    try:\n"
            "        os.close(1)\n"
            "        os.close(2)\n"
            "    except OSError:\n"
            "        pass\n"
            "    time.sleep(30)\n"
            "    os._exit(0)\n"
            "sys.stdout.write('ok\\n')\n"
        ) % pidfile
        with mock.patch.object(subprocess, "Popen", self.real_popen):
            result = preflight.default_executor(
                [sys.executable, "-B", "-c", script], 2.0)
        self.assertEqual(result.get("exit_code"), 0)
        deadline = time.monotonic() + 2
        grandchild = None
        while time.monotonic() < deadline:
            if os.path.exists(pidfile):
                with open(pidfile, encoding="utf-8") as handle:
                    text = handle.read().strip()
                if text.isdigit():
                    grandchild = int(text)
                    break
            time.sleep(0.05)
        self.assertIsNotNone(grandchild)
        alive = True
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            try:
                os.kill(grandchild, 0)
            except OSError:
                alive = False
                break
            time.sleep(0.05)
        self.assertFalse(alive)

    def test_sysfs_device_name_matches_worker_gate(self):
        self.assertTrue(preflight.sysfs_device_name_ok("nvme0n1p2"))
        self.assertFalse(preflight.sysfs_device_name_ok("nvme0n1p2:0"))
        self.assertFalse(preflight.sysfs_device_name_ok("nvme0n1p2."))
        self.assertFalse(preflight.sysfs_device_name_ok("nvme0n1p2 "))
        self.assertFalse(preflight.sysfs_device_name_ok("a" * 256))
        self.assertFalse(preflight.sysfs_device_name_ok(".git"))
        self.assertFalse(preflight.sysfs_device_name_ok("id_rsa"))
        self.assertFalse(preflight.sysfs_device_name_ok("foo.env"))
        self.assertTrue(preflight.sysfs_device_name_ok("a" * 255))
        self.assertFalse(preflight.sysfs_device_name_ok("nvme\udc80n1"))
        self.assertFalse(preflight.sysfs_device_name_ok("nvme\x85n1"))

    def test_overflow_kills_probe_descendants(self):
        pidfile = os.path.join(self.tmp.name, "grandchild.pid")
        script = (
            "import subprocess,sys\n"
            "child=subprocess.Popen(['sleep','30'])\n"
            "open(%r,'w').write(str(child.pid))\n"
            "sys.stdout.write('x'*200000)\n"
        ) % pidfile
        with mock.patch.object(subprocess, "Popen", self.real_popen):
            result = preflight.default_executor(
                [sys.executable, "-B", "-c", script], 2.0)
        self.assertTrue(result.get("output_exceeded"))
        with open(pidfile, encoding="utf-8") as handle:
            grandchild = int(handle.read().strip())
        deadline = time.monotonic() + 2
        alive = True
        while time.monotonic() < deadline:
            try:
                os.kill(grandchild, 0)
            except OSError:
                alive = False
                break
            time.sleep(0.05)
        self.assertFalse(alive)

    def test_overflow_kills_child_that_inherited_stdout(self):
        pidfile = os.path.join(self.tmp.name, "overflow-orphan.pid")
        script = (
            "import os,sys,time\n"
            "child=os.fork()\n"
            "if child==0:\n"
            "    open(%r,'w').write(str(os.getpid()))\n"
            "    sys.stdout.write('x'*200000)\n"
            "    sys.stdout.flush()\n"
            "    time.sleep(30)\n"
            "    os._exit(0)\n"
            "os._exit(0)\n"
        ) % pidfile
        with mock.patch.object(subprocess, "Popen", self.real_popen):
            result = preflight.default_executor(
                [sys.executable, "-B", "-c", script], 2.0)
        self.assertTrue(result.get("output_exceeded"))
        deadline = time.monotonic() + 2
        grandchild = None
        while time.monotonic() < deadline:
            if os.path.exists(pidfile):
                with open(pidfile, encoding="utf-8") as handle:
                    text = handle.read().strip()
                if text.isdigit():
                    grandchild = int(text)
                    break
            time.sleep(0.05)
        self.assertIsNotNone(grandchild)
        alive = True
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            try:
                os.kill(grandchild, 0)
            except OSError:
                alive = False
                break
            time.sleep(0.05)
        self.assertFalse(alive)

    def test_nul_output_does_not_timeout_watchdog(self):
        with mock.patch.object(subprocess, "Popen", self.real_popen):
            result = preflight.default_executor(
                [sys.executable, "-B", "-c",
                 "import sys; sys.stdout.buffer.write(b'\\x00'*50000)"],
                2.0)
        self.assertFalse(result.get("output_exceeded"))
        self.assertEqual(result.get("exit_code"), 0)

    def test_stale_rootless_podman_socket_falls_through(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = {
            "stdout": "/run/user/1000/podman/podman.sock\n/run/podman/podman.sock\n"}
        fixture["podman_info"] = [{"stdout": "\n"}, podman_ok()]
        with mock.patch.dict(os.environ, {"XDG_RUNTIME_DIR": "/run/user/1000"}):
            code, report, executor = self.run_main(fixture)
        self.assertEqual(code, 0)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["value"], "podman 4.9.0")
        info_urls = [argv[3] for argv in executor.seen
                     if argv[:3] == ["podman", "--remote=true", "--url"]]
        self.assertEqual(info_urls, [
            "unix:///run/user/1000/podman/podman.sock",
            "unix:///run/podman/podman.sock",
        ])

    def test_podman_probe_forces_local_mode(self):
        self.assertEqual(preflight.PROBE_ARGS["podman_info"][:2], ["podman", "--remote=true"])
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = podman_socket_ok()
        fixture["podman_info"] = podman_ok()
        _, _, executor = self.run_main(fixture)
        self.assertIn(
            ["podman", "--remote=true", "--url", "unix:///run/podman/podman.sock",
             "info", "--format", "{{.Version.Version}}"],
            executor.seen)

    def test_podman_socket_discovery_timeout_is_bounded(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = {"timeout": True}
        code, report, executor = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("timed out", by_id["container_engine"]["detail"])
        self.assertIn(["podman", "--version"], executor.seen)
        self.assertFalse(any(argv and argv[0] == "podman" and "info" in argv
                             for argv in executor.seen))

    def test_podman_socket_helper_rejects_unexpected_path(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture.pop("docker_version")
        fixture.pop("docker_info")
        fixture.pop("docker_context", None)
        fixture["podman_client"] = podman_client_ok()
        fixture["podman_socket"] = {"stdout": "/tmp/evil.sock\n"}
        fixture["podman_info"] = podman_ok()
        code, report, executor = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("unexpected path", by_id["container_engine"]["detail"])
        self.assertIn(["podman", "--version"], executor.seen)
        self.assertFalse(any(argv and argv[0] == "podman" and "info" in argv
                             for argv in executor.seen))

    def test_select_mount_point_does_not_realpath(self):
        with mock.patch.object(os.path, "realpath",
                               side_effect=AssertionError("realpath in main process")):
            self.assertEqual(
                preflight.select_mount_point(["/", "/mnt/data"], "/mnt/data/workers"),
                "/mnt/data")

    def test_decode_probe_output_replaces_invalid_utf8(self):
        self.assertIn("\ufffd", preflight.decode_probe_output(b"ok\xffend"))

    def test_uname_nonzero_exit_is_error_not_rejection(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["os"] = {"exit_code": 1, "stdout": "", "stderr": "uname: boom"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["os_linux"]["status"], "error")

    def test_os_detail_reports_release_and_arch(self):
        code, report, _ = self.run_main(dict(DEFAULT_FIXTURE))
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(code, 0)
        self.assertIn("6.1.0", by_id["os_linux"]["detail"])
        self.assertIn("x86_64", by_id["os_linux"]["detail"])

    def test_engine_probe_timeout(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = {"timeout": True}
        fixture.pop("docker_info")
        fixture.pop("podman_info", None)
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("timed out", by_id["container_engine"]["detail"])


