#!/usr/bin/env python3
"""Owned-container desktop control; results are fixture inputs, never MCP proof."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import time
import uuid

ROOT = Path("/fixture/state")
SOURCE = Path("/fixture/source")


def document(sha256, expect_api_status=False):
    observer = {"name": "Public MCP observer", "kind": "codex", "command": "/usr/bin/python3",
                "args": [str(SOURCE / "scripts/tailnet-api-status-fixture/observer.py"),
                         "/candidate/horizon", sha256, "--seconds", "1800"],
                "position": [40, 60], "size": [640, 300]}
    if expect_api_status:
        observer["args"].append("--expect-api-status")
    return {"version": 11, "window": {"width": 1480, "height": 900},
            "appearance": {"theme": "dark"}, "presets": [observer],
            "workspaces": [{"name": "Synthetic tailnet fixture", "cwd": str(ROOT / "repo"),
                            "terminals": [observer]}]}


def fixture_class():
    path = SOURCE / "scripts/tailnet-api-store-fixture/secret_service.py"
    spec = importlib.util.spec_from_file_location("owned_secret_service", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.Fixture


def prepare_x11_directory(path=Path("/tmp/.X11-unix")):
    try:
        path.mkdir(mode=0o700)
    except FileExistsError as error:
        raise RuntimeError("private isolated X11 directory must be new") from error
    if path.is_symlink() or not path.is_dir() or path.stat().st_mode & 0o777 != 0o700 or path.stat().st_uid != os.getuid():
        raise RuntimeError("private isolated X11 directory ownership refused")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--check-only", action="store_true")
    parser.add_argument("--expect-api-status", action="store_true")
    args = parser.parse_args()
    os.umask(0o077)
    if Path("/run/dbus/system_bus_socket").exists() or os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY"):
        raise RuntimeError("container must start without host desktop/bus")
    if not args.check_only:
        prepare_x11_directory()
    fixture = fixture_class()(ROOT)
    requested = {"off": False, "on": False, "stop": False}
    signal.signal(signal.SIGUSR1, lambda *_: requested.update(off=True))
    signal.signal(signal.SIGUSR2, lambda *_: requested.update(on=True))
    signal.signal(signal.SIGTERM, lambda *_: requested.update(stop=True))
    results = []
    record = {"state": "failed", "synthetic_only": True, "mcp_claimed": False,
              "mode": "api_status" if args.expect_api_status else "baseline"}
    target = ROOT / "target.json"
    app = None
    try:
        fixture.start()
        prepared = fixture.run_helper("native-prepare")
        results.append(prepared)
        (ROOT / "expected-inputs.json").write_text(json.dumps({"names": {row["id"]: row["name"] for row in prepared["tailnets"]}, "credentials": prepared["direct_store_status"]}))
        if args.check_only:
            results.append(fixture.run_helper("native-check"))
        if not args.check_only:
            (ROOT / "horizon/.task-1335-api-native-fixture").touch(mode=0o600)
            (ROOT / "repo").mkdir(mode=0o700)
            (ROOT / "config.json").write_text(json.dumps(document(args.sha256, args.expect_api_status)))
            # This target belongs to the container's X11 namespace, not the host.
            target.write_text(json.dumps({"id": "native-api-" + uuid.uuid4().hex,
                                          "endpoint": {"kind": "local_x11", "display": ":71"}}))
            xvfb = fixture.spawn("xvfb", ["/usr/bin/Xvfb", ":71", "-screen", "0", "1600x1000x24",
                                           "-nolisten", "tcp", "-extension", "MIT-SHM"])
            fixture.wait(lambda: Path("/tmp/.X11-unix/X71").is_socket(), xvfb)
            fixture.env.update(DISPLAY=":71", XDG_SESSION_TYPE="x11", LIBGL_ALWAYS_SOFTWARE="1")
            fixture.spawn("openbox", ["/usr/bin/openbox"])
            fixture.spawn("vnc", ["/usr/bin/x11vnc", "-norc", "-no6", "-display", ":71", "-listen", "127.0.0.1",
                                  "-rfbport", "5900", "-viewonly", "-forever", "-shared", "-nopw", "-noxdamage", "-noshm"])
            app = fixture.spawn("horizon", ["/candidate/horizon", "--config", str(ROOT / "config.json"), "--new-session"])
            running_hash = hashlib.sha256()
            with open(f"/proc/{app.pid}/exe", "rb") as executable:
                for chunk in iter(lambda: executable.read(1024 * 1024), b""):
                    running_hash.update(chunk)
            digest = running_hash.hexdigest()
            if digest != args.sha256:
                raise RuntimeError("running candidate hash mismatch")
            record.update(horizon_pid=app.pid, horizon_sha256=digest)
            deadline = time.monotonic() + 1800
            while app.poll() is None and not requested["stop"]:
                if time.monotonic() >= deadline:
                    raise RuntimeError("native fixture lifetime exceeded")
                if requested["off"]:
                    fixture.stop_keyring()
                    requested["off"] = False
                if requested["on"]:
                    fixture.start_keyring()
                    requested["on"] = False
                time.sleep(.1)
            if requested["stop"]:
                raise RuntimeError("forced fixture stop is not normal native qualification")
            if app.returncode:
                raise RuntimeError("native candidate exit failed")
            qualification = ROOT / "mcp-evidence/qualification.json"
            public = json.loads(qualification.read_text()) if qualification.is_file() else {}
            if public.get("mode") != record["mode"] or not public.get("public_legacy_three_tailnets"):
                raise RuntimeError("public baseline bootstrap not qualified")
            if args.expect_api_status and not public.get("public_api_status_initial"):
                raise RuntimeError("public API status bootstrap not qualified")
            record.update(public_bootstrap_checked=True, public_evidence="mcp-evidence/rpc.jsonl")
        if not fixture.secret_owned():
            fixture.start_keyring()
        results.append(fixture.run_helper("native-cleanup"))
        items = fixture.bus_call("--dest", "org.freedesktop.secrets", "--object-path", "/org/freedesktop/secrets",
                                 "--method", "org.freedesktop.Secret.Service.SearchItems", "{}")
        if items not in ("(@ao [], @ao [])", "([], [])"):
            raise RuntimeError("owned Secret Service is not empty")
        record.update(state="passed", owned_service_empty=True)
    finally:
        target.unlink(missing_ok=True)
        fixture.close()
        record.update(results=results, owned_processes_stopped=all(p.poll() is not None for p in fixture.children),
                      native_started=app is not None, native_exit=None if app is None else app.returncode,
                      target_expired=not target.exists())
        (ROOT / "native-result.json").write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps(record), flush=True)


if __name__ == "__main__":
    main()
