#!/usr/bin/env python3
"""Bounded native Linux UI smoke on a task-owned display; no browser control."""

import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import select
import shlex
import shutil
import signal
import subprocess
import sys
import time

import processes


class Smoke:
    def __init__(self, binary, root, seconds):
        self.binary = binary
        self.root = root
        self.deadline = time.monotonic() + seconds
        self.children = []
        self.logs = []
        self.window = None
        self.app = None
        self.checks = []
        self.supervised = False
        self.stage = "initializing"
        self.browser = None
        self.repository = None
        # An allowlist prevents inherited agent sessions, homes, credentials,
        # display sockets and loader overrides from reaching the candidate.
        self.env = {
            "PATH": "/usr/local/bin:/usr/bin:/bin",
            "LANG": "C.UTF-8",
            "HOME": str(root / "home"),
            "XDG_CONFIG_HOME": str(root / "home/config"),
            "XDG_DATA_HOME": str(root / "home/data"),
            "XDG_CACHE_HOME": str(root / "home/cache"),
            "XDG_RUNTIME_DIR": str(root / "runtime"),
            "LIBGL_ALWAYS_SOFTWARE": "1",
            "GALLIUM_DRIVER": "llvmpipe",
            "WGPU_BACKEND": "vulkan",
            "WINIT_UNIX_BACKEND": "x11",
            "RUST_LOG": "info",
            "SHELL": "/bin/bash",
        }
        for name in ("home", "runtime"):
            (root / name).mkdir(mode=0o700)

    def remaining(self):
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("smoke deadline expired")
        return remaining

    def command(self, argv, check=True):
        try:
            return subprocess.run(
                argv, env=self.env, cwd=self.root, text=True, capture_output=True,
                timeout=min(10, self.remaining()), check=check,
            )
        except (OSError, subprocess.SubprocessError) as error:
            stderr = getattr(error, "stderr", "") or ""
            if isinstance(stderr, bytes):
                stderr = stderr.decode("utf-8", errors="replace")
            stderr = stderr.replace(str(self.root), "<artifacts>")
            stderr = re.sub(r"(?im)(authorization|token|password|secret)(\s*[:=]\s*)[^\r\n]+", r"\1\2<redacted>", stderr)
            stderr = re.sub(r"(https?://[^\s?]+)\?[^\s]+", r"\1?<redacted>", stderr)
            diagnostic = {"stage": self.stage, "tool": Path(argv[0]).name,
                          "returncode": getattr(error, "returncode", None),
                          "error": type(error).__name__, "stderr": stderr[-2048:]}
            (self.root / "command-failure.json").write_text(json.dumps(diagnostic, indent=2) + "\n")
            raise

    def spawn(self, name, argv, **kwargs):
        log = (self.root / (name + ".log")).open("w", encoding="utf-8")
        self.logs.append(log)
        child = subprocess.Popen(
            argv, env=self.env, cwd=self.root, stdout=log, stderr=log,
            start_new_session=True, **kwargs,
        )
        self.children.append(child)
        return child

    def wait_for(self, description, predicate):
        self.stage = description
        while True:
            self.remaining()
            for child in self.children:
                if child.poll() is not None:
                    raise RuntimeError(f"owned process {child.pid} exited during {description}")
            value = predicate()
            if value:
                return value
            time.sleep(min(0.1, self.remaining()))

    def display(self):
        self.stage = "private display"
        read_fd, write_fd = os.pipe()
        try:
            self.spawn("xvfb", [
                "Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1600x1000x24",
                "-nolisten", "tcp", "-extension", "MIT-SHM",
            ], pass_fds=(write_fd,))
        finally:
            os.close(write_fd)
        try:
            if not select.select([read_fd], [], [], min(15, self.remaining()))[0]:
                raise TimeoutError("Xvfb did not allocate a display")
            number = os.read(read_fd, 32).decode("ascii").strip()
        finally:
            os.close(read_fd)
        if not number.isdecimal():
            raise RuntimeError("Xvfb returned no display number")
        self.env["DISPLAY"] = ":" + number
        self.spawn("openbox", ["openbox", "--sm-disable"])
        self.wait_for("window manager", lambda: self.command(["wmctrl", "-m"], False).returncode == 0)
        self.checks.append("private_display")

    def launch(self):
        self.stage = "candidate launch"
        script = "printf ready > terminal-ready; exec /bin/bash --noprofile --norc"
        config = {
            "version": 10,
            "window": {"width": 1200, "height": 800},
            "presets": [],
            "workspaces": [{
                "name": "Linux worker smoke", "cwd": str(self.root),
                "position": [20, 20],
                "terminals": [{
                    "name": "Disposable shell", "kind": "shell",
                    "command": "/bin/bash", "args": ["--noprofile", "--norc", "-c", script],
                    "position": [30, 30], "size": [800, 500],
                }],
            }],
        }
        config_path = self.root / "config.json"
        if self.browser:
            config["browser"] = {"backend": self.browser, "headless": True,
                                 "command": "/usr/bin/chromium", "firefox_command": "/usr/bin/firefox-esr",
                                 "geckodriver_command": "/usr/local/bin/geckodriver"}
            config["workspaces"][0]["terminals"].append({
                "name": "Browser test actor", "kind": "codex", "command": "/usr/bin/python3",
                "args": [str(Path(__file__).resolve(strict=True).with_name("browser-smoke.py")), "--binary", str(self.binary),
                         "--repository", str(self.repository), "--backend", self.browser,
                         "--artifacts", str(self.root)],
                "position": [900, 30], "size": [600, 400],
            })
        config_path.write_text(json.dumps(config), encoding="utf-8")
        self.app = self.spawn("horizon", [str(self.binary), "--config", str(config_path), "--ephemeral"])

        def window():
            found = self.command(["xdotool", "search", "--onlyvisible", "--pid", str(self.app.pid)], False)
            return next(iter(found.stdout.split()), None)

        self.window = self.wait_for("candidate window", window)
        self.wait_for("terminal startup", lambda: (self.root / "terminal-ready").is_file())
        self.command(["xdotool", "windowactivate", "--sync", self.window])
        self.checks.append("window_and_terminal_started")

    def screenshot(self, name):
        self.stage = "screenshot"
        time.sleep(min(1, self.remaining()))
        self.command(["scrot", "--silent", str(self.root / (name + ".png"))])
        if (self.root / (name + ".png")).stat().st_size < 1024:
            raise RuntimeError("screenshot is unexpectedly small")

    def fit(self):
        self.stage = "fit workspace"
        self.command(["xdotool", "windowactivate", "--sync", self.window])
        self.command(["xdotool", "key", "--clearmodifiers", "ctrl+shift+9"])
        time.sleep(min(1, self.remaining()))

    def input(self, name):
        self.stage = "terminal input command"
        marker = "HORIZON_UI_INPUT_OK_" + name
        path = self.root / (name + ".txt")
        self.command(["xdotool", "mousemove", "--window", self.window, "400", "300", "click", "1"])
        text = "printf '%s\\n' " + shlex.quote(marker) + " > " + shlex.quote(str(path))
        self.command(["xdotool", "type", "--clearmodifiers", "--delay", "1", text])
        self.command(["xdotool", "key", "--clearmodifiers", "Return"])
        self.wait_for("terminal input", lambda: path.is_file() and path.read_text().strip() == marker)
        self.checks.append(name)

    def resize(self):
        self.stage = "resize command"
        self.command(["xdotool", "windowsize", self.window, "1000", "700"])

        def resized():
            geometry = self.command(["xdotool", "getwindowgeometry", "--shell", self.window]).stdout
            return "WIDTH=1000\n" in geometry and "HEIGHT=700\n" in geometry

        self.wait_for("window resize", resized)
        self.checks.append("window_resized")

    def close(self):
        self.stage = "normal window close"
        self.command(["wmctrl", "-ic", hex(int(self.window))])
        result = self.app.wait(timeout=min(15, self.remaining()))
        if result != 0:
            raise RuntimeError(f"candidate exited with status {result}")
        self.checks.append("normal_window_close")

    def cleanup(self):
        # Only process groups created above are eligible. A cleanup fallback is
        # failure evidence, never a successful normal-close assertion.
        clean = True
        if self.supervised:
            candidate_closed = self.app is None or self.app.poll() is not None
            clean = processes.cleanup_descendants() and candidate_closed
            for child in self.children:
                child.poll()
            for log in self.logs:
                log.close()
            return clean
        for child in reversed(self.children):
            if child.poll() is None:
                if child is self.app:
                    clean = False
                try:
                    os.killpg(child.pid, signal.SIGTERM)
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    clean = False
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait(timeout=5)
                except ProcessLookupError:
                    child.wait(timeout=5)
        for log in self.logs:
            log.close()
        return clean


def snapshot_candidate(binary, root):
    candidate = root / "candidate-horizon"
    # One open source descriptor survives an atomic replacement by cargo build.
    with binary.open("rb") as source, candidate.open("xb") as target:
        shutil.copyfileobj(source, target)
    candidate.chmod(0o500)
    return candidate


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--artifacts", required=True, type=Path, help="new directory; existing paths are refused")
    parser.add_argument("--timeout-seconds", type=int, default=90)
    parser.add_argument("--browser", choices=("chromium", "firefox"))
    parser.add_argument("--repository", type=Path, help="candidate checkout containing the existing public MCP test client")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error("--binary must be an executable file")
    if not 30 <= args.timeout_seconds <= 600:
        parser.error("--timeout-seconds must be between 30 and 600")
    if args.browser and (args.repository is None or not (args.repository / "scripts/browser-smoke/mcp_gate.py").is_file()):
        parser.error("browser smoke requires --repository with the public MCP test client")
    for tool in ("Xvfb", "openbox", "scrot", "wmctrl", "xdotool", "bash"):
        if not shutil.which(tool, path="/usr/local/bin:/usr/bin:/bin"):
            parser.error("required tool is missing: " + tool)
    root = args.artifacts.absolute()
    root.mkdir(mode=0o700, parents=False, exist_ok=False)
    binary = snapshot_candidate(binary, root)
    with binary.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    smoke = Smoke(binary, root, args.timeout_seconds)
    smoke.browser = args.browser
    smoke.repository = args.repository.resolve(strict=True) if args.repository else None
    processes.adopt_orphans()
    smoke.supervised = True
    result = {"version": 1, "status": "failed", "binary_sha256": digest, "checks": smoke.checks}
    def interrupted(_number, _frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, interrupted)
    try:
        smoke.display()
        smoke.launch()
        if smoke.browser:
            receipt = root / "browser-result.json"
            smoke.wait_for("browser MCP smoke", receipt.is_file)
            browser_result = json.loads(receipt.read_text())
            if browser_result["status"] != "passed":
                raise RuntimeError("browser MCP smoke failed; see browser-result.json")
            smoke.checks.append(smoke.browser + "_public_mcp")
        smoke.fit()
        if not smoke.browser:
            smoke.input("input_before_resize")
        smoke.screenshot("launch")
        smoke.resize()
        smoke.fit()
        if not smoke.browser:
            smoke.input("input_after_resize")
        smoke.screenshot("resized")
        smoke.close()
        result["status"] = "passed"
    except (OSError, RuntimeError, subprocess.SubprocessError, TimeoutError, KeyboardInterrupt) as error:
        # Child output stays in private files; never echo an inherited environment.
        result["error"] = type(error).__name__
        result["failed_stage"] = smoke.stage
        if (root / "command-failure.json").is_file():
            result["command_diagnostic"] = "command-failure.json"
    finally:
        result["cleanup_complete"] = smoke.cleanup()
        try:
            binary.unlink()
        except OSError:
            result["cleanup_complete"] = False
        if not result["cleanup_complete"]:
            result["status"] = "failed"
            result.setdefault("failed_stage", "cleanup")
        (root / "result.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result))
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
