#!/usr/bin/env python3
"""Disposable real SSH tests; no paid worker or GitHub mutation occurs."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time


def main():
    fixture = Path(__file__).resolve().parent
    with tempfile.TemporaryDirectory(prefix="ssh-policy-") as temporary:
        root = Path(temporary)
        server = subprocess.Popen([sys.executable, str(fixture / "serve.py"), "--root", str(root), "--delay", "0.01"],
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            ready = json.loads(server.stdout.readline())
            assert ready["ready"]
            connection = json.loads((root / "connection.json").read_text())

            def call(command, value=None, success=True):
                result = subprocess.run(["ssh"] + connection["ssh_args"] + [command],
                                        input=json.dumps(value) if value is not None else None,
                                        text=True, capture_output=True, timeout=10)
                if success:
                    assert result.returncode == 0, (command, result.stderr)
                else:
                    assert result.returncode != 0, command
                return json.loads(result.stdout) if result.stdout.strip() else None

            def await_status(predicate, seconds=8):
                deadline = time.monotonic() + seconds
                while time.monotonic() < deadline:
                    status = call("maintenance status")
                    if predicate(status):
                        return status
                    time.sleep(0.1)
                raise AssertionError("SSH status did not converge")

            initial = call("maintenance status")
            assert initial["worker_health"]["state"] == "not_started"
            assert not initial["worker_health"]["alive"]
            started = call("maintenance start")
            status = await_status(lambda s: s["worker_health"]["state"] == "idle")
            assert status["worker_health"]["alive"] and status["executed_count"] == 3
            first_beat = status["worker_health"]["heartbeat_at"]
            await_status(lambda s: s["worker_health"]["heartbeat_at"] != first_beat)

            policy = {"global_prompt": "Keep CI and repository recipes mandatory.",
                      "repo_prompts": {"example/sample-service": "Inspect compatibility before retry."}}
            response = call("maintenance configure", policy)
            assert response["accepted"] and response["configRevision"] == 2
            applied = await_status(lambda s: s["applied_revision"] == response["configRevision"])
            assert applied["global_prompt"] == policy["global_prompt"]
            assert next(r for r in applied["repos"] if r["repository"] == "example/sample-service")["prompt"] == policy["repo_prompts"]["example/sample-service"]
            for bad in [dict(policy, command="gh api"),
                        {"global_prompt": "x", "repo_prompts": {"../other": "x"}},
                        {"global_prompt": "x" * 4097, "repo_prompts": {}},
                        {"global_prompt": "x", "repo_prompts": {"example/sample-web": "x" * 2049}}]:
                rejected = call("maintenance configure", bad, success=False)
                assert not rejected["accepted"]
            oversized = subprocess.run(connection["configure_command"], input="x" * 65537,
                                       capture_output=True, text=True, timeout=10)
            assert oversized.returncode != 0 and not json.loads(oversized.stdout)["accepted"]
            arbitrary = subprocess.run(["ssh"] + connection["ssh_args"] + ["gh api repos/example/sample-web/pulls/124/merge"],
                                       capture_output=True, text=True, timeout=10)
            assert arbitrary.returncode != 0
            diagnosis = call("maintenance diagnose")
            assert diagnosis["policy"]["revision"] == 2
            assert "PRIVATE KEY" not in json.dumps(diagnosis)
            assert len(diagnosis["log_tail"].encode()) <= 8192

            # Kill only the PID this disposable SSH server created, then inspect over SSH.
            child_pid = started["pid"]
            assert child_pid == status["worker_health"]["pid"] and child_pid != server.pid
            os.kill(child_pid, signal.SIGKILL)
            dead = await_status(lambda s: not s["worker_health"]["alive"])
            assert dead["worker_health"]["state"] == "error" and dead["worker_health"]["last_error"]
            restarted = call("maintenance start")
            assert restarted["pid"] != child_pid
            persistent = await_status(lambda s: s["applied_revision"] == 2 and s["worker_health"]["alive"])
            assert persistent["global_prompt"] == policy["global_prompt"]
            stopped = call("maintenance stop")
            assert stopped["stopped"]
            stopped_status = call("maintenance status")
            assert not stopped_status["worker_health"]["alive"] and stopped_status["worker_health"]["state"] == "stopped"
            report = {"passed": True, "real_ssh": True, "repos": len(initial["repos"]),
                      "policy_apply_converged": True, "policy_survives_restart": True,
                      "invalid_policy_rejected": True, "oversized_input_rejected": True,
                      "idle_heartbeat_advances": True, "worker_death_observed_over_healthy_ssh": True,
                      "restricted_stop": True, "arbitrary_github_command_rejected": True,
                      "diagnostics_exclude_keys": True, "synthetic": True}
            print(json.dumps(report))
        finally:
            server.terminate()
            server.wait(timeout=6)


if __name__ == "__main__":
    main()
