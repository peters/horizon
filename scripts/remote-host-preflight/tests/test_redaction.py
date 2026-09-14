"""Deterministic tests for the read-only Linux host preflight."""
from harness import *  # noqa: F401,F403
class RedactionAndDeterminism(Harness):
    def test_credentials_are_redacted(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = docker_daemon_down(
            "connect unix:///var/run/docker.sock token=supersecretvalue refused")
        fixture["tailscale_version"] = {"stdout": "1.0.0 %s\n" % JWT}
        _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn("supersecretvalue", text)
        self.assertNotIn(JWT, text)
        self.assertIn("<redacted>", text)

    def test_redaction_of_nonmatching_line_is_not_quadratic(self):
        blob = "a" * 65536
        started = time.monotonic()
        out = preflight.redact(blob)
        elapsed = time.monotonic() - started
        self.assertEqual(out, blob[:400])
        self.assertLess(elapsed, 0.5)

    def test_overlong_jwt_payload_is_fully_redacted(self):
        token = "eyJhbGciOiJIUzI1NiJ9." + ("a" * 5000) + ".sigsuffix"
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = docker_daemon_down("denied token=" + token)
        _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn("sigsuffix", text)
        self.assertNotIn("a" * 50, text)
        self.assertIn("<redacted>", text)

    def test_quoted_json_diagnostics_are_redacted(self):
        blob = '{"password":"hunter2","Authorization":"Bearer supersecrettok"}'
        self.assertNotIn("hunter2", preflight.redact(blob))
        self.assertNotIn("supersecrettok", preflight.redact(blob))
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = docker_daemon_down(blob)
        _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn("hunter2", text)
        self.assertNotIn("supersecrettok", text)
        self.assertIn("<redacted>", text)

    def test_github_pat_and_uri_userinfo_are_redacted(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = docker_daemon_down(
            "clone https://user:hunter2@github.com/org/repo.git ghp_abcdefghijklmnop123")
        _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn("hunter2", text)
        self.assertNotIn("ghp_abcdefghijklmnop123", text)
        self.assertIn("<redacted>", text)

    def test_overlong_uri_userinfo_is_redacted(self):
        secret = "s" * 300
        fixture = dict(DEFAULT_FIXTURE)
        with mock.patch.dict(os.environ, {
                "DOCKER_HOST": "tcp://user:%s@remote:2376" % secret}):
            _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn(secret, text)
        self.assertIn("<redacted>", text)

    def test_uri_userinfo_above_64kib_is_redacted(self):
        secret = "s" * 70000
        fixture = dict(DEFAULT_FIXTURE)
        with mock.patch.dict(os.environ, {
                "DOCKER_HOST": "tcp://user:%s@remote:2376" % secret}):
            _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn(secret[:64], text)
        self.assertIn("<redacted>", text)

    def test_username_only_uri_userinfo_is_redacted(self):
        fixture = dict(DEFAULT_FIXTURE)
        with mock.patch.dict(os.environ, {"DOCKER_HOST": "tcp://supersecret@remote:2376"}):
            _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn("supersecret", text)
        self.assertIn("<redacted>", text)

    def test_printable_line_strips_c1_and_bidi_controls(self):
        raw = "ok\x9bhidden\u202eflip"
        out = preflight.printable_line(raw)
        self.assertNotIn("\x9b", out)
        self.assertNotIn("\u202e", out)
        self.assertIn("ok", out)
        self.assertIn("hidden", out)

    def test_format_seconds_keeps_submicrosecond_timeouts(self):
        self.assertEqual(preflight.format_seconds(0.1), "0.1")
        self.assertIn("e-", preflight.format_seconds(1e-9))
        self.assertNotEqual(preflight.format_seconds(1e-9), "")

    def test_sysfs_block_access_denied_is_error(self):
        def denied(path):
            raise OSError(errno.EACCES, "Permission denied", path)

        with mock.patch.object(os, "readlink", side_effect=denied):
            code, report, _ = self.run_main(dict(DEFAULT_FIXTURE))
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["storage_ext4_qualifier"]["status"], "error")
        self.assertIn("unreadable", by_id["storage_ext4_qualifier"]["detail"])

    def test_timeout_message_preserves_fractional_seconds(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["os"] = {"timeout": True}
        argv = ["--procfs-root", os.path.join(self.tmp.name, "procfs"),
                "--sysfs-root", os.path.join(self.tmp.name, "sysfs"),
                "--workspace-path", os.path.join(self.tmp.name, "ws"),
                "--now", NOW, "--json", "--timeout", "0.1"]
        os.makedirs(os.path.join(self.tmp.name, "ws"), exist_ok=True)
        executor = self.executor_for(fixture)
        import io
        from contextlib import redirect_stdout
        buffer = io.StringIO()
        with redirect_stdout(buffer):
            preflight.main(argv, executor=executor, now=NOW)
        report = json.loads(buffer.getvalue())
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertIn("0.1", by_id["os_linux"]["detail"])
        self.assertNotIn("after 0s", by_id["os_linux"]["detail"])

    def test_existing_file_workspace_is_rejected(self):
        fixture = dict(DEFAULT_FIXTURE)
        path = os.path.join(self.tmp.name, "not-a-dir")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write("x")
        code, report, _ = self.run_main(fixture, workspace=path)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["disk_capacity"]["status"], "error")
        self.assertIn("not a directory", by_id["disk_capacity"]["detail"])
        self.assertEqual(by_id["storage_ext4_qualifier"]["status"], "unsupported")
        self.assertIn("not a directory", by_id["storage_ext4_qualifier"]["detail"])

    def test_prefixed_credential_keys_are_redacted(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = docker_daemon_down(
            "access_token=aaa111 refresh-token=bbb222 client_secret=ccc333")
        _, report, _ = self.run_main(fixture)
        text = json.dumps(report)
        self.assertNotIn("aaa111", text)
        self.assertNotIn("bbb222", text)
        self.assertNotIn("ccc333", text)
        self.assertIn("<redacted>", text)

    def test_timeout_must_be_positive_finite(self):
        import io
        from contextlib import redirect_stderr
        with redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as caught:
                preflight.main(["--workspace-path", ""])
        self.assertEqual(caught.exception.code, 3)
        with redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as caught:
                preflight.main(["--now", "nonsense"])
        self.assertEqual(caught.exception.code, 3)

        for value in ("-1", "0", "nan", "inf", "-inf", "1e300"):
            with self.subTest(value=value):
                with redirect_stderr(io.StringIO()):
                    with self.assertRaises(SystemExit) as caught:
                        preflight.main(["--timeout", value])
                self.assertEqual(caught.exception.code, 3)

    def test_report_is_deterministic(self):
        import io
        from contextlib import redirect_stdout
        fixture = dict(DEFAULT_FIXTURE)

        def render():
            argv = ["--procfs-root", "/nonexistent-proc", "--sysfs-root", "/nonexistent-sys",
                    "--workspace-path", "/nonexistent-ws", "--now", NOW, "--json"]
            executor = self.executor_for(fixture)
            buffer = io.StringIO()
            with redirect_stdout(buffer):
                preflight.main(argv, executor=executor, now=NOW)
            return buffer.getvalue()

        first = render()
        second = render()
        self.assertEqual(first, second)

    def test_main_honors_injected_now_without_cli_flag(self):
        fixture = dict(DEFAULT_FIXTURE)
        procfs, sysfs, _ = build_roots(self.tmp.name, meminfo(), None, EXT4_OK, True)
        workspace = os.path.join(self.tmp.name, "ws")
        os.makedirs(workspace, exist_ok=True)
        argv = ["--procfs-root", procfs, "--sysfs-root", sysfs,
                "--workspace-path", workspace, "--json"]
        executor = self.executor_for(fixture)
        import io
        from contextlib import redirect_stdout
        buffer = io.StringIO()
        with redirect_stdout(buffer):
            preflight.main(argv, executor=executor, now=NOW)
        report = json.loads(buffer.getvalue())
        self.assertEqual(report["generated_at"], NOW)

    def test_argv_allowlist_enforced(self):
        permitted = {
            "os": ["uname", "-srm"],
            "cores": ["nproc"],
            "docker_version": ["docker", "version", "--format", "{{json .}}"],
            "docker_info": ["docker", "info", "--format", "{{.Driver}}"],
            "docker_context": ["docker", "context", "inspect", "--format",
                               "{{.Endpoints.docker.Host}}"],
            "podman_client": ["podman", "--version"],
            "podman_info": ["podman", "--remote=true", "--url"],
            "podman_info_tail": ["info", "--format", "{{.Version.Version}}"],
            "disk": ["df", "-kP"],
            "tailscale_version": ["tailscale", "version"],
            "tailscale_status": ["tailscale", "status", "--json", "--peers=false"],
        }
        helper_keys = ("podman_socket", "workspace_dir")
        self.assertEqual(set(preflight.PROBE_ARGS), set(permitted) | set(helper_keys))
        for key, args in permitted.items():
            self.assertEqual(list(preflight.PROBE_ARGS[key]), args, key)
        for key in helper_keys:
            argv = list(preflight.PROBE_ARGS[key])
            self.assertEqual(argv[:3], [sys.executable, "-B", "-c"], key)
            self.assertEqual(len(argv), 4, key)
            script = argv[3]
            self.assertNotIn("unlink", script, key)
            self.assertNotIn("mkdir", script, key)
            self.assertNotIn("rmtree", script, key)
            self.assertNotIn("Popen", script, key)
        fixture = dict(DEFAULT_FIXTURE)
        _, _, executor = self.run_main(fixture)
        allowed = [list(args) for args in permitted.values()]
        extra_prefixes = [list(preflight.PROBE_ARGS[key]) for key in helper_keys]
        extra_prefixes.append(list(permitted["podman_info"]))
        extra_prefixes.append(list(permitted["disk"]))
        for argv in executor.seen:
            skipped = False
            if (len(argv) >= 3 and argv[0] == "docker" and argv[1] == "--host"
                    and argv[2].startswith("unix://")
                    and (["docker"] + argv[3:]) in allowed):
                skipped = True
            for prefix in extra_prefixes:
                if argv[:len(prefix)] == prefix and len(argv) > len(prefix):
                    skipped = True
                    break
            if skipped:
                continue
            self.assertIn(argv, allowed)

    def test_docker_daemon_probes_pin_validated_host(self):
        fixture = dict(DEFAULT_FIXTURE)
        _, _, executor = self.run_main(fixture)
        self.assertIn(
            ["docker", "--host", "unix:///var/run/docker.sock",
             "version", "--format", "{{json .}}"],
            executor.seen)
        self.assertIn(
            ["docker", "--host", "unix:///var/run/docker.sock",
             "info", "--format", "{{.Driver}}"],
            executor.seen)
        self.assertNotIn(list(preflight.PROBE_ARGS["docker_version"]), executor.seen)
        self.assertNotIn(list(preflight.PROBE_ARGS["docker_info"]), executor.seen)

    def test_docker_host_pin_is_not_redacted(self):
        socket = "unix:///tmp/client_secret=supersecretvalue/docker.sock"
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_context"] = docker_context_ok(host=socket)
        _, report, executor = self.run_main(fixture)
        self.assertIn(
            ["docker", "--host", socket, "version", "--format", "{{json .}}"],
            executor.seen)
        self.assertNotIn("supersecretvalue", json.dumps(report))

    def test_docker_context_nonzero_exit_is_inspect_failure(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_context"] = {
            "exit_code": 1, "stdout": "", "stderr": "permission denied"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 1)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["container_engine"]["status"], "unsupported")
        self.assertIn("docker context inspect failed", by_id["container_engine"]["detail"])
        self.assertIn("permission denied", by_id["container_engine"]["detail"])
        self.assertNotIn("endpoint missing", by_id["container_engine"]["detail"])

    def test_workspace_resolver_preserves_tab_in_path(self):
        path = "/mnt/ws\tname"

        def executor(argv, timeout):
            return {"exit_code": 0,
                    "stdout": json.dumps({"path": path, "major": 8, "minor": 1}) + "\n",
                    "stderr": ""}

        resolved, major, minor, problem = preflight.resolve_workspace_directory(
            executor, 1.0, path)
        self.assertIsNone(problem)
        self.assertEqual(resolved, path)
        self.assertEqual((major, minor), (8, 1))

    def test_workspace_unreadable_is_error(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["workspace_dir"] = {"exit_code": 4, "stderr": "unreadable\n"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["disk_capacity"]["status"], "error")
        self.assertEqual(by_id["storage_ext4_qualifier"]["status"], "error")
        self.assertIn("unreadable", by_id["storage_ext4_qualifier"]["detail"])

    def test_workspace_resolver_malformed_output_is_error(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["workspace_dir"] = {"stdout": "not-a-triple\n"}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["storage_ext4_qualifier"]["status"], "error")
        self.assertEqual(by_id["disk_capacity"]["status"], "error")
        self.assertIn("malformed", by_id["storage_ext4_qualifier"]["detail"])

    def test_workspace_resolution_timeout_is_bounded(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["workspace_dir"] = {"timeout": True}
        code, report, _ = self.run_main(fixture)
        self.assertEqual(code, 2)
        by_id = {check["id"]: check for check in report["checks"]}
        self.assertEqual(by_id["disk_capacity"]["status"], "error")
        self.assertIn("timed out", by_id["disk_capacity"]["detail"])

    def test_disk_probe_is_restricted_to_workspace_path(self):
        fixture = dict(DEFAULT_FIXTURE)
        workspace = os.path.join(self.tmp.name, "ws")
        os.makedirs(workspace, exist_ok=True)
        _, _, executor = self.run_main(fixture, workspace=workspace)
        disk_calls = [argv for argv in executor.seen
                      if argv[:2] == ["df", "-kP"]]
        self.assertEqual(len(disk_calls), 1)
        resolved, problem = preflight.workspace_directory(workspace)
        self.assertIsNone(problem)
        self.assertEqual(disk_calls[0][-1], resolved)

    def test_human_report_normalizes_multiline_details(self):
        fixture = dict(DEFAULT_FIXTURE)
        fixture["docker_version"] = docker_daemon_down(
            "denied\ncpu_capacity             supported   forged")
        _, report, _ = self.run_main(fixture)
        text = preflight.render_text(report)
        cpu_rows = [line for line in text.splitlines() if line.startswith("cpu_capacity")]
        self.assertEqual(len(cpu_rows), 1)


if __name__ == "__main__":
    unittest.main()
