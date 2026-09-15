#!/usr/bin/env python3
"""Second-computer smoke for remote browser targets (peters/horizon#628).

Two halves, run on two computers:

  prepare  (computer A)  writes an isolated Horizon configuration with the
                         hosted-grid targets, exports the portable profile
                         through `horizon --export-remote-profile`, and checks
                         that the file carries no binding, value or local path.
  run      (computer B)  imports that file through `horizon
                         --import-remote-profile` into a fresh configuration,
                         binds each credential reference to this computer's OS
                         store (the same binding the Settings > Remote browsers
                         row adds), enters the credential into that store,
                         starts Horizon, drives one target through the public
                         MCP tools only, closes it, asks the provider whether
                         the session is released, then removes the stored
                         credential, restarts Horizon and shows that the target
                         is refused as `credentials_not_ready`: nothing about
                         the credential survives outside the store.

Computer B needs Horizon (this branch's binary), network access, Python for
this harness and nothing else: no browser, driver binary, Appium, Selenium,
mobile SDK, Node.js or provider SDK. The agent panel that owns the browser
panel runs a shell one-liner (cmd.exe on Windows, /bin/sh elsewhere) so
Horizon's own side of the flow needs no Python either.

Credentials come from a mode-600 netrc on computer B and go only into that
computer's OS credential store (Secret Service, macOS Keychain or Windows
Credential Manager) under the exact item Horizon's keyring adapter addresses;
they never appear in the profile, the configuration, the logs or the report.
"""

from __future__ import annotations

import argparse
import base64
import json
import netrc
import os
import pathlib
import platform
import queue
import shutil
import signal
import stat
import subprocess
import sys
import threading
import time

HERE = pathlib.Path(__file__).resolve().parent
REPO = pathlib.Path(os.environ["HORIZON_REPO"]) if os.environ.get("HORIZON_REPO") else HERE.parents[1]
sys.path.insert(0, str(REPO / "scripts" / "remote-browser-evidence"))
sys.path.insert(0, str(REPO / "scripts" / "browser-smoke"))
from live_smoke import (  # noqa: E402
    API,
    FIXTURE,
    HUB,
    HUB_ORIGIN,
    RPC_LOG,
    SERVICE,
    SLOTS,
    TARGETS,
    handshake,
    provider_session_state,
    raw,
    wait_for,
)


class McpClient:
    """JSON-RPC over the MCP server's stdio, one request at a time. A reader
    thread feeds a queue, so the same client runs where `select` does not
    work on pipes (Windows). Same shape as `mcp_gate.McpClient`."""

    def __init__(self, command: pathlib.Path, log_path: pathlib.Path, timeout: float, actor: str, host_instance: str | None = None) -> None:
        self.log = log_path.open("w", encoding="utf-8")
        environment = os.environ.copy()
        environment["HORIZON_BROWSER_ACTOR"] = actor
        environment.pop("HORIZON_BROWSER_HOST_INSTANCE", None)
        if host_instance:
            environment["HORIZON_BROWSER_HOST_INSTANCE"] = host_instance
        environment["RUST_LOG"] = "off"
        self.process = subprocess.Popen([str(command), "--browser-mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=self.log, text=True, encoding="utf-8", bufsize=1, env=environment)
        self.timeout = timeout
        self.next_id = 1
        self.lines: queue.Queue[str | None] = queue.Queue()
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()

    def _read(self) -> None:
        assert self.process.stdout is not None
        for line in self.process.stdout:
            self.lines.put(line)
        self.lines.put(None)

    def _send(self, message: dict) -> None:
        assert self.process.stdin is not None
        self.process.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
        self.process.stdin.flush()

    def request(self, method: str, params: dict | None = None) -> dict:
        request_id = self.next_id
        self.next_id += 1
        message: dict = {"jsonrpc": "2.0", "id": request_id, "method": method}
        if params is not None:
            message["params"] = params
        self._send(message)
        deadline = time.monotonic() + self.timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"MCP request timed out: {method}")
            try:
                line = self.lines.get(timeout=remaining)
            except queue.Empty as error:
                raise TimeoutError(f"MCP request timed out: {method}") from error
            if line is None:
                raise RuntimeError(f"MCP server closed while waiting for {method}")
            response = json.loads(line)
            if response.get("id") == request_id:
                return response

    def notify(self, method: str) -> None:
        self._send({"jsonrpc": "2.0", "method": method})

    def close(self) -> None:
        if self.process.stdin is not None:
            self.process.stdin.close()
        try:
            status = self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill()
            status = self.process.wait(timeout=10)
        self.log.close()
        if status != 0:
            raise AssertionError(f"MCP server exited with status {status}")

PROVIDER = "browserstack"
BINDINGS = {"user": SLOTS["user"], "key": SLOTS["key"]}
# Windows Credential Manager target name used by the windows-native keyring
# store Horizon links: `<user>.<service>` with the store's default delimiters.
WINDOWS_TARGET = "{user}.{service}"


NETRC = pathlib.Path(os.environ.get("HORIZON_NETRC", "~/.config/horizon-dev/browserstack.netrc")).expanduser()


def load_netrc() -> tuple[str, str]:
    """The provider credential for this computer. POSIX: the file must be
    mode 600; Windows: NTFS permissions, checked by the operator instead."""
    if os.name != "nt" and stat.S_IMODE(NETRC.stat().st_mode) & 0o077:
        raise SystemExit("netrc must be mode 600")
    entry = netrc.netrc(str(NETRC)).hosts.get("hub-cloud.browserstack.com")
    if not entry or not entry[0] or not entry[2]:
        raise SystemExit("netrc has no complete hub-cloud.browserstack.com entry")
    return entry[0], entry[2]


def utc_now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def session_name(run_name: str, target: str) -> str:
    return f"{run_name}-{target}"


def keyring_user(reference: str) -> str:
    return f"{HUB_ORIGIN}|{BINDINGS[reference]}"


def probe_command(root: pathlib.Path, actor_path: pathlib.Path, host_path: pathlib.Path) -> tuple[str, list[str]]:
    """The agent panel's command: write the injected identity to two files
    and stay alive, with no Python on Horizon's side of the flow.

    Horizon wraps every agent command in `$SHELL -ic <command>` (a POSIX
    login-shell convention with no Windows default yet), so on Windows the
    harness points SHELL at a two-line batch shim that unwraps the quoted
    command and runs it; the probe itself is a batch file."""
    if os.name == "nt":
        probe = root / "probe.cmd"
        probe.write_text(
            "@echo off\r\n"
            f'<nul set /p ="%HORIZON_BROWSER_HOST_INSTANCE%" > "{host_path}"\r\n'
            f'<nul set /p ="%HORIZON_BROWSER_ACTOR%" > "{actor_path}"\r\n'
            "ping -n 3600 127.0.0.1 > nul\r\n",
            encoding="ascii",
        )
        shim = root / "shim.cmd"
        shim.write_text(
            "@echo off\r\n"
            'set "TARGET=%~2"\r\n'
            "set \"TARGET=%TARGET:'=%\"\r\n"
            "call %TARGET%\r\n",
            encoding="ascii",
        )
        os.environ.setdefault("SHELL", str(shim))
        return str(probe), []
    script = (
        f'printf %s "$HORIZON_BROWSER_HOST_INSTANCE" > "{host_path}"; '
        f'printf %s "$HORIZON_BROWSER_ACTOR" > "{actor_path}"; sleep 3600'
    )
    return "/bin/sh", ["-c", script]


def base_config(root: pathlib.Path) -> dict:
    command, args = probe_command(root, root / "agent-actor", root / "agent-host-instance")
    return {
        "version": 10,
        "window": {"width": 1400, "height": 900},
        "workspaces": [
            {
                "name": "Second computer",
                "position": [30, 30],
                "terminals": [
                    {
                        "name": "Evidence agent",
                        "kind": "codex",
                        "command": command,
                        "args": args,
                        "position": [30, 30],
                        "size": [620, 360],
                    }
                ],
            }
        ],
    }


def horizon_command(horizon: pathlib.Path, config: pathlib.Path, *extra: str, env: dict) -> dict:
    """Run the Horizon binary in command mode (it exits) and keep its output."""
    started = time.monotonic()
    proc = subprocess.run([str(horizon), "--config", str(config), *extra], env=env, capture_output=True, text=True, timeout=120)
    return {"args": list(extra), "exit": proc.returncode, "stdout": proc.stdout.strip(), "stderr_lines": len(proc.stderr.splitlines()),
            "ms": int((time.monotonic() - started) * 1000)}


def prepare(args: argparse.Namespace) -> int:
    root = pathlib.Path(args.out) / f"prepare-{args.run_name}"
    root.mkdir(parents=True, exist_ok=True)
    config = dict(base_config(root))
    targets = {}
    for name in args.targets:
        target = json.loads(json.dumps(TARGETS[name]))
        target["capability_extensions"]["bstack:options"]["sessionName"] = session_name(args.run_name, name)
        targets[name] = target
    config["browser"] = {
        "remote": {
            "providers": {
                PROVIDER: {
                    "adapter": "browserstack",
                    "endpoint": HUB,
                    "authentication": {"kind": "basic", "username_ref": "user", "password_ref": "key"},
                    # Computer A's own bindings: they must not travel.
                    "credential_bindings": {ref: {"store": "os_keychain", "slot": slot} for ref, slot in BINDINGS.items()},
                    "limits": {"max_sessions": 1, "allocation_timeout_seconds": 180, "idle_release_seconds": 180, "max_session_seconds": 900},
                }
            },
            "targets": targets,
        }
    }
    config_path = root / "config.json"
    config_path.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")
    env = dict(os.environ)
    env["HOME"] = str(root / "home")
    env.pop("HORIZON", None)
    (root / "home").mkdir(exist_ok=True)
    profile = root / "remote-browser-profile.yaml"
    result = horizon_command(pathlib.Path(args.horizon), config_path, "--export-remote-profile", str(profile), env=env)
    document = profile.read_text(encoding="utf-8") if profile.exists() else ""
    checks = {
        "export_exit": result["exit"],
        "export_stdout": result["stdout"],
        "starts_with_format_marker": document.startswith("horizon_remote_browser_profile: 1\n"),
        "carries_bindings": "credential_bindings" in document,
        "carries_slot": any(slot in document for slot in BINDINGS.values()),
        "carries_local_path": str(root) in document or "home" in document.split("providers:")[0],
        "bytes": len(document.encode("utf-8")),
    }
    report = {"half": "prepare", "run_name": args.run_name, "started": utc_now(), "host": host_facts(), "profile": str(profile), "checks": checks}
    report["passed"] = result["exit"] == 0 and checks["starts_with_format_marker"] and not checks["carries_bindings"] and not checks["carries_slot"] and not checks["carries_local_path"]
    (root / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))
    return 0 if report["passed"] else 1


def host_facts() -> dict:
    absent = {}
    for tool in ("chromedriver", "geckodriver", "safaridriver", "msedgedriver", "appium", "selenium-server", "node", "npm", "adb", "xcrun"):
        absent[tool] = shutil.which(tool) is None
    return {
        "platform": platform.platform(),
        "machine": platform.machine(),
        "python_for_harness_only": platform.python_version(),
        "tools_absent": absent,
    }


# --- OS credential store entry on computer B -------------------------------

WINDOWS_CRED_SCRIPT = r"""
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class HorizonCred {
  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
  public struct CREDENTIAL {
    public uint Flags; public uint Type; public string TargetName; public string Comment;
    public System.Runtime.InteropServices.ComTypes.FILETIME LastWritten;
    public uint CredentialBlobSize; public IntPtr CredentialBlob; public uint Persist;
    public uint AttributeCount; public IntPtr Attributes; public string TargetAlias; public string UserName;
  }
  [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
  public static extern bool CredWriteW(ref CREDENTIAL cred, uint flags);
  [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
  public static extern bool CredReadW(string target, uint type, uint flags, out IntPtr credential);
  [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
  public static extern bool CredDeleteW(string target, uint type, uint flags);
  [DllImport("advapi32.dll")]
  public static extern void CredFree(IntPtr buffer);
  public static void Write(string target, string user, byte[] blob) {
    var c = new CREDENTIAL();
    c.Type = 1; c.TargetName = target; c.UserName = user; c.Persist = 2;
    c.CredentialBlobSize = (uint)blob.Length;
    c.CredentialBlob = Marshal.AllocHGlobal(blob.Length);
    Marshal.Copy(blob, 0, c.CredentialBlob, blob.Length);
    try { if (!CredWriteW(ref c, 0)) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error()); }
    finally { Marshal.FreeHGlobal(c.CredentialBlob); }
  }
  public static string Read(string target) {
    IntPtr p;
    if (!CredReadW(target, 1, 0, out p)) return null;
    try {
      var c = (CREDENTIAL)Marshal.PtrToStructure(p, typeof(CREDENTIAL));
      var blob = new byte[c.CredentialBlobSize];
      if (c.CredentialBlobSize > 0) Marshal.Copy(c.CredentialBlob, blob, 0, blob.Length);
      return Convert.ToBase64String(blob);
    } finally { CredFree(p); }
  }
  public static bool Delete(string target) { return CredDeleteW(target, 1, 0); }
}
"@
$mode = $env:HORIZON_SEED_MODE
foreach ($reference in @('user', 'key')) {
  $user = [Environment]::GetEnvironmentVariable("HORIZON_SEED_USER_$reference")
  $target = "$user." + $env:HORIZON_SEED_SERVICE
  if ($mode -eq 'read') {
    $blob = [HorizonCred]::Read($target)
    if ($blob -eq $null) { "$reference=" } else { "$reference=$blob" }
  } elseif ($mode -eq 'write') {
    $value = [Environment]::GetEnvironmentVariable("HORIZON_SEED_VALUE_$reference")
    if ($value -eq $null -or $value -eq '') { [void][HorizonCred]::Delete($target) }
    else { [HorizonCred]::Write($target, $user, [Convert]::FromBase64String($value)) }
  }
}
"""


def _windows_items(mode: str, values: dict[str, bytes | None] | None = None) -> dict[str, bytes | None]:
    """Read or write Horizon's two Credential Manager items. Values travel
    to PowerShell through the environment as base64, never as arguments."""
    env = dict(os.environ)
    env["HORIZON_SEED_MODE"] = mode
    env["HORIZON_SEED_SERVICE"] = SERVICE
    for reference in BINDINGS:
        env[f"HORIZON_SEED_USER_{reference}"] = keyring_user(reference)
        if values is not None:
            blob = values.get(reference)
            env[f"HORIZON_SEED_VALUE_{reference}"] = base64.b64encode(blob).decode() if blob is not None else ""
    proc = subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", WINDOWS_CRED_SCRIPT], env=env,
                          check=True, capture_output=True, text=True)
    found: dict[str, bytes | None] = {}
    for line in proc.stdout.splitlines():
        reference, _, blob = line.strip().partition("=")
        if reference in BINDINGS:
            found[reference] = base64.b64decode(blob) if blob else None
    return found


def _macos_read(reference: str, keychain: str | None) -> bytes | None:
    cmd = ["security", "find-generic-password", "-s", SERVICE, "-a", keyring_user(reference), "-w"]
    if keychain:
        cmd.append(keychain)
    proc = subprocess.run(cmd, capture_output=True, text=True, check=False)
    return proc.stdout.rstrip("\n").encode() if proc.returncode == 0 else None


def _macos_write(reference: str, value: bytes | None, keychain: str | None) -> None:
    if value is None:
        cmd = ["security", "delete-generic-password", "-s", SERVICE, "-a", keyring_user(reference)]
        if keychain:
            cmd.append(keychain)
        subprocess.run(cmd, capture_output=True, check=False)
        return
    cmd = ["security", "add-generic-password", "-U", "-s", SERVICE, "-a", keyring_user(reference), "-w", value.decode()]
    if keychain:
        cmd.append(keychain)
    subprocess.run(cmd, check=True, capture_output=True)


def read_items(keychain: str | None) -> dict[str, bytes | None]:
    """What Horizon's items hold on this computer right now, per reference."""
    system = platform.system()
    if system == "Darwin":
        return {reference: _macos_read(reference, keychain) for reference in BINDINGS}
    if system == "Windows":
        return _windows_items("read")
    raise SystemExit(f"no OS store handling for {system}")


def write_items(values: dict[str, bytes | None], keychain: str | None) -> None:
    """Put `values` into Horizon's items; `None` removes an item."""
    system = platform.system()
    if system == "Darwin":
        for reference, value in values.items():
            _macos_write(reference, value, keychain)
    elif system == "Windows":
        _windows_items("write", values)
    else:
        raise SystemExit(f"no OS store handling for {system}")


def seed_store(login: str, password: str, keychain: str | None) -> dict:
    """Enter the credential into this computer's OS store under Horizon's
    items and hand back what those items held before, so an already
    configured computer gets its own values back. Seeding is all or nothing:
    a failure part way restores what was already replaced."""
    system = platform.system()
    if system == "Linux":
        from live_smoke import seed_keyring

        return {"system": system, "previous": seed_keyring(login, password)}
    previous = read_items(keychain)
    written: dict[str, bytes | None] = {}
    try:
        for reference, value in (("user", login.encode()), ("key", password.encode())):
            write_items({reference: value}, keychain)
            written[reference] = value
    except Exception:
        write_items({reference: previous.get(reference) for reference in written}, keychain)
        raise
    return {"system": system, "previous": previous}


def clear_store(seeded: dict, keychain: str | None) -> None:
    """Put back what the items held before the run, or remove them."""
    system = seeded["system"]
    if system == "Linux":
        from live_smoke import restore_keyring

        restore_keyring(seeded["previous"])
        return
    write_items({reference: seeded["previous"].get(reference) for reference in BINDINGS}, keychain)


def store_has_items(keychain: str | None) -> bool | None:
    """Presence of Horizon's items after the run, by the platform's own
    listing, in the same keychain the run wrote to."""
    system = platform.system()
    if system == "Windows":
        return any(value is not None for value in _windows_items("read").values())
    if system == "Darwin":
        return any(_macos_read(reference, keychain) is not None for reference in BINDINGS)
    return None


# --- computer B ---------------------------------------------------------------

def bind_references(config_path: pathlib.Path) -> dict:
    """Add this computer's bindings under the imported provider, exactly as
    the Settings > Remote browsers row does when "OS credential store" is
    chosen for an unbound reference: `store: os_keychain`, slot
    `remote-browser/<provider>/<reference>`. The file is the YAML Horizon
    wrote on import, so the provider block is located by its fixed layout."""
    lines = config_path.read_text(encoding="utf-8").splitlines()
    providers_line = lines.index(next(line for line in lines if line.strip() == "providers:"))
    provider_line = next(i for i in range(providers_line + 1, len(lines)) if lines[i].strip() == f"{PROVIDER}:")
    indent = len(lines[provider_line]) - len(lines[provider_line].lstrip()) + 2
    pad = " " * indent
    limits_line = next(i for i in range(provider_line + 1, len(lines)) if lines[i] == f"{pad}limits:")
    inserted = [f"{pad}credential_bindings:"]
    for reference, slot in BINDINGS.items():
        inserted += [f"{pad}  {reference}:", f"{pad}    store: os_keychain", f"{pad}    slot: {slot}"]
    lines[limits_line:limits_line] = inserted
    config_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return {"bindings_added": list(BINDINGS), "line": limits_line + 1}


def start_horizon(horizon: pathlib.Path, config: pathlib.Path, root: pathlib.Path, env: dict, log_name: str) -> tuple[subprocess.Popen, str, str]:
    for name in ("agent-actor", "agent-host-instance"):
        (root / name).unlink(missing_ok=True)
    log = (root / log_name).open("a", encoding="utf-8")
    app = subprocess.Popen([str(horizon), "--config", str(config), "--ephemeral"], env=env, stdout=log, stderr=log)
    try:
        actor = wait_for(root / "agent-actor", 120)
        host_instance = wait_for(root / "agent-host-instance", 30)
    except BaseException:
        # A start that never produced the identity must not leave Horizon
        # running: the caller has no handle to it yet.
        stop_horizon(app)
        raise
    time.sleep(3)
    return app, actor, host_instance


def stop_horizon(app: subprocess.Popen) -> None:
    app.terminate()
    try:
        app.wait(timeout=20)
    except subprocess.TimeoutExpired:
        app.kill()


def value_of(result: dict, key: str = "value"):
    return (result.get("structuredContent") or {}).get(key)


def drive_target(client: McpClient, target: str, steps: list[dict]) -> None:
    started = time.monotonic()
    created = raw(client, "browser_create", {"target": target, "url": FIXTURE, "timeout_millis": 90000})
    steps.append({"step": "browser_create", "ms": int((time.monotonic() - started) * 1000), "is_error": created.get("isError"),
                  "result": created.get("structuredContent") or created.get("content")})
    panel = (created.get("structuredContent") or {}).get("panel") or {}
    panel_id = panel.get("panel_id")
    if not panel_id:
        return
    steps.append({"step": "panel_advertises", "remote_target": panel.get("remote_target"), "remote_device": panel.get("remote_device"),
                  "protocol": panel.get("protocol"), "network_capture": (panel.get("network_capture") or {}).get("supported")})
    snap = raw(client, "browser_snapshot", {"panel_id": panel_id, "max_nodes": 40})
    steps.append({"step": "browser_snapshot", "is_error": snap.get("isError"), "node_count": len(value_of(snap, "nodes") or []), "title": value_of(snap, "title")})
    metrics = raw(client, "browser_evaluate", {"panel_id": panel_id, "expression":
        "JSON.stringify({ua: navigator.userAgent, touch: navigator.maxTouchPoints, screen: [screen.width, screen.height], dpr: devicePixelRatio})"})
    steps.append({"step": "device_metrics", "is_error": metrics.get("isError"), "value": value_of(metrics)})
    fill = raw(client, "browser_act", {"panel_id": panel_id, "action": "fill", "selector": "#name", "value": "Second computer"})
    typed = raw(client, "browser_evaluate", {"panel_id": panel_id, "expression": "document.getElementById('name').value"})
    steps.append({"step": "fill_name", "is_error": fill.get("isError"), "field_value": value_of(typed)})
    click = raw(client, "browser_act", {"panel_id": panel_id, "action": "click", "selector": "#submit"})
    waited = raw(client, "browser_wait", {"panel_id": panel_id, "selector": "#result", "state": "visible", "timeout_millis": 10000})
    result = raw(client, "browser_evaluate", {"panel_id": panel_id, "expression": "document.getElementById('result').textContent"})
    steps.append({"step": "click_submit", "is_error": click.get("isError")})
    steps.append({"step": "wait_result", "is_error": waited.get("isError"), "elapsed_millis": value_of(waited, "elapsed_millis")})
    steps.append({"step": "result_text", "is_error": result.get("isError"), "value": value_of(result)})
    closed = raw(client, "browser_close", {"panel_id": panel_id, "timeout_millis": 60000})
    steps.append({"step": "browser_close", "is_error": closed.get("isError"), "result": closed.get("structuredContent") or closed.get("content")})
    listed = raw(client, "browser_list", {})
    steps.append({"step": "browser_list_after_close", "is_error": listed.get("isError"),
                  "panels": [p.get("panel_id") for p in (value_of(listed, "panels") or [])]})


def run(args: argparse.Namespace) -> int:
    root = pathlib.Path(args.out) / f"run-{args.run_name}-{int(time.time())}"
    (root / "home").mkdir(parents=True)
    horizon = pathlib.Path(args.horizon)
    profile = pathlib.Path(args.profile)
    login, password = load_netrc()
    auth = "Basic " + base64.b64encode(f"{login}:{password}".encode()).decode()
    env = dict(os.environ)
    env["HOME"] = str(root / "home")
    env["RUST_LOG"] = "info"
    for name in ("HORIZON", "HORIZON_BROWSER_ACTOR", "HORIZON_BROWSER_HOST_INSTANCE"):
        env.pop(name, None)
    os.environ.clear()
    os.environ.update(env)
    report: dict = {"half": "run", "run_name": args.run_name, "started": utc_now(), "host": host_facts(), "steps": [], "targets": {}}
    steps = report["steps"]

    # 1. A fresh configuration with only the agent panel, then the import.
    #    (The probe may add SHELL to the environment; Horizon gets the result.)
    config_path = root / "config.yaml"
    config_path.write_text(json.dumps(base_config(root), indent=2) + "\n", encoding="utf-8")
    env = dict(os.environ)
    before = config_path.read_text(encoding="utf-8")
    imported = horizon_command(horizon, config_path, "--import-remote-profile", str(profile), env=env)
    after = config_path.read_text(encoding="utf-8")
    steps.append({"step": "import_profile", **imported, "config_changed": before != after,
                  "config_has_provider": any(line.strip() == f"{PROVIDER}:" for line in after.splitlines()),
                  "config_has_bindings": "credential_bindings" in after,
                  "config_has_targets": all(any(line.strip() == f"{t}:" for line in after.splitlines()) for t in args.targets)})
    if imported["exit"] != 0:
        return finish(root, report, args.targets)

    # 2. Bind the references on this computer and enter the credential here.
    steps.append({"step": "bind_references", **bind_references(config_path)})
    seeded = None
    app = None
    # A termination signal must unwind through the finally below so the OS
    # store is restored and Horizon is stopped; Python's default would exit
    # at once. Only the signals this platform delivers to a handler.
    restored_handlers = {}
    for name in ("SIGTERM", "SIGHUP", "SIGBREAK"):
        sig = getattr(signal, name, None)
        if sig is not None:
            restored_handlers[sig] = signal.signal(sig, lambda signum, frame: (_ for _ in ()).throw(SystemExit(128 + signum)))
    try:
        seeded = seed_store(login, password, args.keychain)
        steps.append({"step": "enter_credentials", "store": seeded["system"], "references": list(BINDINGS)})

        # 3. Start Horizon and drive each target through the MCP tools only.
        app, actor, host_instance = start_horizon(horizon, config_path, root, env, "horizon.log")
        steps.append({"step": "horizon_started", "agent_identity_injected": bool(actor)})
        for target in args.targets:
            target_steps: list[dict] = []
            client = McpClient(horizon, root / f"mcp-{target}.log", 200.0, actor, host_instance)
            RPC_LOG[:] = [root / f"rpc-{target}.jsonl"]
            try:
                handshake(client)
                drive_target(client, target, target_steps)
            finally:
                client.close()
            time.sleep(5)
            target_steps.append({"step": "provider_release_proof", **provider_session_state(auth, session_name(args.run_name, target))})
            report["targets"][target] = {"steps": target_steps}
            (root / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        stop_horizon(app)
        app = None
        steps.append({"step": "horizon_stopped"})

        # 4. Remove the stored credential, restart, and show the target is
        #    refused: nothing about the value survives outside the store.
        clear_store(seeded, args.keychain)
        seeded = None
        steps.append({"step": "credentials_removed_from_store", "store_still_lists_items": store_has_items(args.keychain)})
        app, actor, host_instance = start_horizon(horizon, config_path, root, env, "horizon-restart.log")
        client = McpClient(horizon, root / "mcp-restart.log", 200.0, actor, host_instance)
        RPC_LOG[:] = [root / "rpc-restart.jsonl"]
        try:
            handshake(client)
            refused = raw(client, "browser_create", {"target": args.targets[0], "url": FIXTURE, "timeout_millis": 30000})
            content = refused.get("structuredContent") or refused.get("content")
            text = json.dumps(content)
            steps.append({"step": "restart_requires_reentry", "is_error": refused.get("isError"), "result": content,
                          "code_is_credentials_not_ready": "credentials_not_ready" in text,
                          "leaks_value": login in text or password in text})
            listed = raw(client, "browser_list", {})
            steps.append({"step": "browser_list_after_refusal", "panels": [p.get("panel_id") for p in (value_of(listed, "panels") or [])]})
        finally:
            client.close()
    finally:
        if app is not None:
            stop_horizon(app)
        if seeded is not None:
            clear_store(seeded, args.keychain)
        for sig, handler in restored_handlers.items():
            signal.signal(sig, handler)
    return finish(root, report, args.targets)


def target_failures(steps: list[dict]) -> list[str]:
    """What the target flow had to show, checked against the recorded steps."""
    by_step = {step["step"]: step for step in steps}
    problems = []

    def need(name: str, ok) -> None:
        step = by_step.get(name)
        if step is None:
            problems.append(f"{name}: not reached")
        elif step.get("is_error"):
            problems.append(f"{name}: error")
        elif not ok(step):
            problems.append(f"{name}: {json.dumps(step)[:160]}")

    need("browser_create", lambda s: (s.get("result") or {}).get("navigation") == "committed")
    need("panel_advertises", lambda s: bool(s.get("remote_device")) and s.get("protocol") == "web_driver")
    need("browser_snapshot", lambda s: s.get("title") == "Horizon mobile fixture")
    need("device_metrics", lambda s: "Mobile" in (s.get("value") or ""))
    need("fill_name", lambda s: s.get("field_value") == "Second computer")
    need("result_text", lambda s: s.get("value") == "result:Second computer")
    need("browser_close", lambda s: (s.get("result") or {}).get("closed") is True)
    need("browser_list_after_close", lambda s: s.get("panels") == [])
    need("provider_release_proof", lambda s: s.get("terminal") is True)
    return problems


def finish(root: pathlib.Path, report: dict, targets: list[str]) -> int:
    failures: dict[str, list[str]] = {}
    for target in targets:
        entry = report["targets"].get(target)
        failures[target] = target_failures(entry["steps"]) if entry else ["no result recorded"]
        report["targets"].setdefault(target, {"steps": []})["failures"] = failures[target]
    by_step = {step["step"]: step for step in report["steps"]}
    problems = []
    if by_step.get("import_profile", {}).get("exit") != 0:
        problems.append("import failed")
    if by_step.get("import_profile", {}).get("config_has_bindings"):
        problems.append("import carried bindings")
    restart = by_step.get("restart_requires_reentry")
    if restart is None:
        problems.append("restart check did not run")
    elif not restart.get("code_is_credentials_not_ready") or restart.get("leaks_value"):
        problems.append("restart did not refuse as credentials_not_ready")
    if by_step.get("credentials_removed_from_store", {}).get("store_still_lists_items"):
        problems.append("store still lists Horizon items after removal")
    report["failures"] = problems
    report["passed"] = not problems and not any(failures.values())
    (root / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))
    return 0 if report["passed"] else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="half", required=True)
    for half in ("prepare", "run"):
        p = sub.add_parser(half)
        p.add_argument("--horizon", required=True, help="Horizon binary on this computer")
        p.add_argument("--run-name", required=True, help="shared between the two halves; names the provider sessions")
        p.add_argument("--targets", nargs="+", default=["ios_phone"], choices=sorted(TARGETS))
        p.add_argument("--out", default=str(pathlib.Path("~/.cache/horizon-628-spike/second-computer").expanduser()))
    sub.choices["run"].add_argument("--profile", required=True, help="the portable profile exported by `prepare`")
    sub.choices["run"].add_argument("--keychain", default=None, help="macOS only: keychain file to add the items to")
    args = parser.parse_args()
    unsupported = [t for t in args.targets if TARGETS[t]["provider"] != PROVIDER]
    if unsupported:
        raise SystemExit(f"only {PROVIDER} targets are supported here: {unsupported}")
    return prepare(args) if args.half == "prepare" else run(args)


if __name__ == "__main__":
    raise SystemExit(main())
