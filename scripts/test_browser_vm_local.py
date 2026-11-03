#!/usr/bin/env python3
"""Tests for scripts/browser-vm-local.sh.

The tests run the script against a fake `docker` that records its arguments, so
they need no Docker daemon and no image. They prove the two contracts that a
person depends on: every port is published on the loopback only, and a `down`
keeps the state volume that holds the sign-ins.
"""

import os
import socket
import threading
import stat
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "browser-vm-local.sh"

FAKE_DOCKER = textwrap.dedent(
    """\
    #!/usr/bin/env bash
    # Answers the subcommands of scripts/browser-vm-local.sh from a state
    # directory, and records every call.
    set -euo pipefail
    printf '%s\\n' "$*" >>"${FAKE_DOCKER_LOG}"
    state_dir="${FAKE_DOCKER_STATE}"
    case "${1:-}" in
        container)
            case "${2:-}" in
                inspect)
                    [[ -f "${state_dir}/container" ]] || exit 1
                    if [[ "${3:-}" == "--format" ]]; then
                        cat "${state_dir}/running"
                    fi
                    ;;
                *) exit 1 ;;
            esac
            ;;
        image)
            [[ "${2:-}" == "inspect" ]] || exit 1
            [[ -f "${state_dir}/image" ]] || exit 1
            ;;
        exec)
            if [[ "$*" == *ssh_host_ed25519_key.pub* ]]; then
                printf 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakeHostKeyOfTheTest %s\n' \\
                    horizon-browser-vm-local
            else
                printf 'probe\\n'
            fi
            ;;
        stop|rm) ;;
        run)
            touch "${state_dir}/container"
            printf 'true\\n' >"${state_dir}/running"
            ;;
        build) ;;
        *) ;;
    esac
    """
)


FAKE_SSH = textwrap.dedent(
    """\
    #!/usr/bin/env bash
    # Answers the key check of the launcher, and records its arguments.
    set -euo pipefail
    printf 'ssh %s\\n' "$*" >>"${FAKE_DOCKER_LOG}"
    exit "${FAKE_SSH_EXIT:-0}"
    """
)

VNC_BANNER = b"RFB 003.008\n"


class VncResponder(threading.Thread):
    """Greets like the VNC server, so the launcher's protocol check can pass."""

    def __init__(self, port):
        super().__init__(daemon=True)
        self.socket = socket.socket()
        self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.socket.bind(("127.0.0.1", port))
        self.socket.listen(8)
        self.stop = threading.Event()

    def run(self):
        self.socket.settimeout(0.2)
        while not self.stop.is_set():
            try:
                connection, _ = self.socket.accept()
            except OSError:
                continue
            with connection:
                connection.sendall(VNC_BANNER)

    def close(self):
        self.stop.set()
        self.socket.close()
        self.join(timeout=2)


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


class LocalContainerTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="browser-vm-local-"))
        self.addCleanup(subprocess.run, ["rm", "-rf", str(self.root)], check=False)
        self.home = self.root / "home"
        self.home.mkdir()
        self.state = self.root / "docker-state"
        self.state.mkdir()
        self.log = self.root / "docker.log"
        self.log.write_text("")
        fake = self.root / "bin" / "docker"
        fake.parent.mkdir()
        fake.write_text(FAKE_DOCKER)
        fake.chmod(0o755)
        # The launcher proves the services: a fake `ssh` answers the key check,
        # and the VNC port greets. Neither a Docker daemon nor an image is used.
        ssh = self.root / "bin" / "ssh"
        ssh.write_text(FAKE_SSH)
        ssh.chmod(0o755)
        self.script_state = self.root / "script-state"

    def run_script(self, *arguments, ports=None, env=None):
        environment = {
            "PATH": f"{self.root / 'bin'}:{os.environ['PATH']}",
            "HOME": str(self.home),
            "FAKE_DOCKER_LOG": str(self.log),
            "FAKE_DOCKER_STATE": str(self.state),
            **(env or {}),
        }
        listeners = []
        responders = []
        extra = list(arguments)
        if ports is not None:
            extra += ["--vnc-port", str(ports[0]), "--ssh-port", str(ports[1])]
            responder = VncResponder(ports[0])
            responder.start()
            responders.append(responder)
            listener = socket.socket()
            listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            listener.bind(("127.0.0.1", ports[1]))
            listener.listen(8)
            listeners.append(listener)
        try:
            return subprocess.run(
                [str(SCRIPT), *extra, "--state", str(self.script_state)],
                capture_output=True,
                text=True,
                env=environment,
                timeout=120,
            )
        finally:
            for listener in listeners:
                listener.close()
            for responder in responders:
                responder.close()

    def calls(self):
        return [line for line in self.log.read_text().splitlines() if line]

    def mark_image(self):
        (self.state / "image").touch()

    def mark_container(self):
        (self.state / "container").touch()
        (self.state / "running").write_text("true\n")

    def test_the_alias_names_the_loopback_and_the_state_key(self):
        result = self.run_script("alias", "--ssh-port", "2223")
        self.assertEqual(result.returncode, 0, result.stderr)
        block = result.stdout
        self.assertIn("Host horizon-browser-vm-local\n", block)
        self.assertIn("  HostName 127.0.0.1\n", block)
        self.assertIn("  Port 2223\n", block)
        self.assertIn(f"  IdentityFile {self.script_state}/ssh/id_ed25519\n", block)
        self.assertIn(
            f"  UserKnownHostsFile {self.script_state}/ssh/known_hosts-horizon-browser-vm-local\n",
            block,
        )
        self.assertIn("  StrictHostKeyChecking yes\n", block)
        # A known host that the script does not check would let another local
        # process answer in place of the container.
        self.assertIn("  IdentitiesOnly yes\n", block)

    def test_up_publishes_the_ports_on_the_loopback_only(self):
        self.mark_image()
        vnc, ssh = free_port(), free_port()
        result = self.run_script("up", ports=(vnc, ssh))
        self.assertEqual(result.returncode, 0, result.stderr)
        run = next(call for call in self.calls() if call.startswith("run "))
        self.assertIn(f"--publish 127.0.0.1:{vnc}:5900", run)
        self.assertIn(f"--publish 127.0.0.1:{ssh}:2222", run)
        self.assertNotIn("--publish 0.0.0.0", run)
        self.assertNotIn(f"--publish {vnc}:5900", run)
        self.assertIn("--volume horizon-browser-vm-local-state:/state", run)
        self.assertIn("--shm-size 1g", run)
        self.assertIn("--env HORIZON_BROWSER_VM_SSH=1", run)
        self.assertIn("--env HORIZON_BROWSER_VM_SSH_PUBKEY=ssh-ed25519 ", run)
        self.assertIn("horizon-browser-vm-local:latest", run)

    def test_up_writes_a_private_key_and_keeps_an_existing_one(self):
        self.mark_image()
        first = self.run_script("up", ports=(free_port(), free_port()))
        self.assertEqual(first.returncode, 0, first.stderr)
        key = self.script_state / "ssh" / "id_ed25519"
        self.assertTrue(key.is_file())
        self.assertEqual(stat.S_IMODE(key.stat().st_mode), 0o600)
        before = key.read_text()
        (self.state / "container").unlink()
        second = self.run_script("up", ports=(free_port(), free_port()))
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(key.read_text(), before, "a second up must keep the key")

    def test_up_builds_the_local_target_when_the_image_is_absent(self):
        result = self.run_script("up", ports=(free_port(), free_port()))
        self.assertEqual(result.returncode, 0, result.stderr)
        build = next(call for call in self.calls() if call.startswith("build "))
        self.assertIn("--target browser-vm-local", build)
        self.assertIn("--file packaging/browser-vm/Dockerfile", build)
        self.assertIn("--tag horizon-browser-vm-local:latest", build)

    def test_up_without_build_refuses_an_absent_image(self):
        result = self.run_script("up", "--no-build", ports=(free_port(), free_port()))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("the image horizon-browser-vm-local:latest is absent", result.stderr)
        self.assertEqual([call for call in self.calls() if call.startswith("run ")], [])

    def test_up_refuses_a_container_that_already_exists(self):
        self.mark_image()
        self.mark_container()
        result = self.run_script("up", ports=(free_port(), free_port()))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("run 'down' first", result.stderr)

    def test_down_keeps_the_state_volume(self):
        self.mark_container()
        result = self.run_script("down")
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.calls()
        self.assertIn("stop horizon-browser-vm-local", calls)
        self.assertIn("rm horizon-browser-vm-local", calls)
        self.assertEqual(
            [call for call in calls if call.startswith("volume")],
            [],
            "a down must keep the state volume that holds the sign-ins",
        )

    def test_up_pins_the_host_key_of_the_container(self):
        self.mark_image()
        vnc, ssh = free_port(), free_port()
        result = self.run_script("up", ports=(vnc, ssh))
        self.assertEqual(result.returncode, 0, result.stderr)
        pinned = (
            self.script_state / "ssh" / "known_hosts-horizon-browser-vm-local"
        ).read_text()
        self.assertEqual(
            pinned,
            f"[127.0.0.1]:{ssh} ssh-ed25519 "
            "AAAAC3NzaC1lZDI1NTE5AAAAIFakeHostKeyOfTheTest\n",
        )
        # A start with a new state volume replaces the earlier entry of the same
        # address instead of leaving a key that ssh would refuse.
        (self.state / "container").unlink()
        again = self.run_script("up", ports=(vnc, ssh))
        self.assertEqual(again.returncode, 0, again.stderr)
        pinned = (
            self.script_state / "ssh" / "known_hosts-horizon-browser-vm-local"
        ).read_text()
        self.assertEqual(pinned.count("[127.0.0.1]:"), 1)

    def test_up_proves_the_ssh_key_and_the_vnc_greeting(self):
        self.mark_image()
        vnc, ssh = free_port(), free_port()
        result = self.run_script("up", ports=(vnc, ssh))
        self.assertEqual(result.returncode, 0, result.stderr)
        ssh_call = next(call for call in self.calls() if call.startswith("ssh "))
        self.assertIn("-o BatchMode=yes", ssh_call)
        self.assertIn("-o StrictHostKeyChecking=yes", ssh_call)
        self.assertIn(f"-p {ssh}", ssh_call)
        self.assertIn(f"-i {self.script_state}/ssh/id_ed25519", ssh_call)
        self.assertIn(
            f"-o UserKnownHostsFile={self.script_state}/ssh/known_hosts-horizon-browser-vm-local",
            ssh_call,
        )
        self.assertIn("root@127.0.0.1", ssh_call)
        self.assertIn("the SSH server accepts the key", result.stderr)
        self.assertIn("the VNC server greets", result.stderr)

    def test_up_fails_when_the_ssh_server_refuses_the_key(self):
        self.mark_image()
        result = self.run_script(
            "up", ports=(free_port(), free_port()), env={"FAKE_SSH_EXIT": "255"}
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("refused the key", result.stderr)

    def test_status_names_the_ports_and_the_ssh_target(self):
        self.mark_image()
        self.mark_container()
        result = self.run_script("status", ports=(free_port(), free_port()))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("running: yes\n", result.stdout)
        self.assertIn("state volume: horizon-browser-vm-local-state\n", result.stdout)
        self.assertIn("(target horizon-browser-vm-local)\n", result.stdout)

    def test_write_alias_keeps_one_block_and_the_other_lines(self):
        config = self.home / ".ssh" / "config"
        config.parent.mkdir()
        config.write_text("Host other\n  HostName 10.0.0.5\n  Port 22\n")
        for _ in range(2):
            result = self.run_script("alias", "--write-alias")
            self.assertEqual(result.returncode, 0, result.stderr)
        text = config.read_text()
        self.assertEqual(text.count("# >>> horizon-browser-vm-local:horizon-browser-vm-local >>>"), 1)
        self.assertEqual(text.count("Host horizon-browser-vm-local\n"), 1)
        self.assertIn("Host other\n  HostName 10.0.0.5\n  Port 22\n", text)
        self.assertEqual(stat.S_IMODE(config.stat().st_mode), 0o600)
        # The block starts on its own line even when the file had no final newline.
        self.assertIn("\n# >>> horizon-browser-vm-local:horizon-browser-vm-local >>>", text)

    def test_each_name_keeps_its_own_known_host_file(self):
        self.mark_image()
        first = self.run_script("up", "--name", "first-local", ports=(free_port(), free_port()))
        self.assertEqual(first.returncode, 0, first.stderr)
        (self.state / "container").unlink()
        second = self.run_script("up", "--name", "second-local", ports=(free_port(), free_port()))
        self.assertEqual(second.returncode, 0, second.stderr)
        ssh_calls = [call for call in self.calls() if call.startswith("ssh ")]
        hosts = sorted(
            part.split("=", 1)[1]
            for call in ssh_calls
            for part in call.split()
            if part.startswith("UserKnownHostsFile=")
        )
        self.assertEqual(
            hosts,
            [
                f"{self.script_state}/ssh/known_hosts-first-local",
                f"{self.script_state}/ssh/known_hosts-second-local",
            ],
        )

    def test_two_names_keep_their_own_alias_block(self):
        config = self.home / ".ssh" / "config"
        first = self.run_script("alias", "--write-alias", "--name", "first-local",
                                "--ssh-port", "2223")
        self.assertEqual(first.returncode, 0, first.stderr)
        second = self.run_script("alias", "--write-alias", "--name", "second-local",
                                 "--ssh-port", "2224")
        self.assertEqual(second.returncode, 0, second.stderr)
        text = config.read_text()
        self.assertIn("# >>> horizon-browser-vm-local:first-local >>>\n", text)
        self.assertIn("# >>> horizon-browser-vm-local:second-local >>>\n", text)
        self.assertIn("Host first-local\n", text)
        self.assertIn("Host second-local\n", text)
        # A rewrite of the first name keeps the block of the second name.
        third = self.run_script("alias", "--write-alias", "--name", "first-local",
                                "--ssh-port", "2223")
        self.assertEqual(third.returncode, 0, third.stderr)
        text = config.read_text()
        self.assertEqual(text.count("# >>> horizon-browser-vm-local:first-local >>>"), 1)
        self.assertIn("# >>> horizon-browser-vm-local:second-local >>>\n", text)

    def test_a_port_must_be_a_number_and_the_two_ports_must_differ(self):
        for arguments, message in [
            (["--vnc-port", "abc"], "is not a number"),
            (["--vnc-port", "70000"], "is not a number"),
            (["--vnc-port", "2222", "--ssh-port", "2222"], "must differ"),
        ]:
            with self.subTest(arguments=arguments):
                result = self.run_script("alias", *arguments)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(message, result.stderr)

    def test_the_key_command_prints_the_public_key_the_container_accepts(self):
        result = self.run_script("key")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(result.stdout.startswith("ssh-ed25519 "))
        self.assertIn("horizon-browser-vm-local:", result.stdout)


if __name__ == "__main__":
    unittest.main()
