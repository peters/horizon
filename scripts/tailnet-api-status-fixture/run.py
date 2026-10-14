#!/usr/bin/env python3
"""Freeze synthetic native inputs and control only the exact owned fixture container."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import platform
import subprocess
import sys
import uuid

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]


def storage():
    path = HERE.parent / "tailnet-api-store-fixture/run.py"
    spec = importlib.util.spec_from_file_location("store_fixture_build", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def owned(path):
    if path.is_symlink() or path.stat().st_mode & 0o077 or path.stat().st_size > 4 * 1024 * 1024:
        raise RuntimeError("private ownership record refused")
    record = json.loads(path.read_text())
    store = storage()
    if store.docker_id() != record["daemon_id"]:
        raise RuntimeError("Docker identity changed; preserve state")
    value = subprocess.check_output(["docker", "inspect", record["container_id"]], text=True, timeout=20)
    actual = json.loads(value)[0]
    if actual["Id"] != record["container_id"] or any(actual["Config"]["Labels"].get(k) != v for k, v in record["labels"].items()):
        raise RuntimeError("fixture container ownership uncertain")
    return record


def freeze_artifacts(manifest, destination):
    if manifest.stat().st_size > 1024 * 1024:
        raise RuntimeError("artifact manifest oversized")
    artifacts = json.loads(manifest.read_text())
    result = {}
    for key, name in [("horizon", "horizon"), ("cloud_deploy", "cloud_deploy"), ("horizon_device", "horizon-device")]:
        item = artifacts[key]
        source = Path(item["path"])
        digest = hashlib.sha256(source.read_bytes()).hexdigest()
        if digest != item["sha256"]:
            raise RuntimeError("frozen artifact hash mismatch")
        target = destination / name
        shutil.copyfile(source, target)
        target.chmod(0o500)
        result[key] = {"path": str(target), "sha256": digest}
    return result


def source_inputs():
    names = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=REPO).decode().split("\0")
    return {name: hashlib.sha256((REPO / name).read_bytes()).hexdigest() for name in sorted(set(filter(None, names))) if (REPO / name).is_file()}


def runtime_mounts():
    executables = ["dbus-daemon", "gnome-keyring-daemon", "gdbus", "Xvfb", "xkbcomp", "openbox", "x11vnc"]
    directories = ["/usr/lib/x86_64-linux-gnu", "/usr/share/X11", "/usr/share/X11/xkb"]
    for directory in directories:
        if not Path(directory).is_dir():
            raise RuntimeError("native runtime directory missing: " + directory)
    mounts = [(Path("/usr/lib/x86_64-linux-gnu"), "/usr/lib/x86_64-linux-gnu", True)]
    for executable in executables:
        path = Path("/usr/bin") / executable
        if not path.is_file() or not os.access(path, os.X_OK):
            raise RuntimeError("native runtime executable unavailable: " + str(path))
        mounts.append((path, str(path), True))
    mounts.append((Path("/usr/share/X11"), "/usr/share/X11", True))
    for directory in ["/usr/share/fonts", "/usr/share/openbox", "/usr/share/vulkan/icd.d"]:
        path = Path(directory)
        if path.exists():
            if not path.is_dir():
                raise RuntimeError("native runtime directory refused: " + directory)
            mounts.append((path, directory, True))
    return mounts


def start(args):
    if sys.platform != "linux" or platform.machine() != "x86_64":
        raise RuntimeError("native fixture requires Linux x86_64")
    runtime = runtime_mounts()
    store = storage()
    base = args.evidence_dir.absolute()
    if base.resolve().is_relative_to(REPO):
        raise RuntimeError("private evidence must stay outside the checkout")
    base.mkdir(mode=0o700, parents=True, exist_ok=False)
    state = base / "state"
    state.mkdir(mode=0o700)
    artifacts = base / "artifacts"
    artifacts.mkdir(mode=0o700)
    frozen = freeze_artifacts(args.manifest, artifacts)
    inputs = source_inputs()
    binary, command = store.build(state, args.target_dir)
    if inputs != source_inputs():
        raise RuntimeError("source changed during fixture build; preserve evidence")
    source = base / "source/scripts"
    source.mkdir(mode=0o700, parents=True)
    for name in ["tailnet-api-status-fixture", "tailnet-api-store-fixture"]:
        shutil.copytree(HERE.parent / name, source / name, ignore=shutil.ignore_patterns("__pycache__"))
    name = "horizon-api-native-" + uuid.uuid4().hex
    labels = {"horizon.task": "1335", "horizon.fixture": "api-status-native", "horizon.fixture-owner": uuid.uuid4().hex}
    argv = ["docker", "run", "-d", "--pull=never", "--init", "--name", name,
            "--network", "none", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
            "--read-only", "--user", f"{os.getuid()}:{os.getgid()}", "--pids-limit", "256", "--memory", "2g",
            "--tmpfs", "/tmp:rw,exec,nosuid,nodev", "--tmpfs", "/run:rw,nosuid,nodev",
            "--env", "DISPLAY=", "--env", "WAYLAND_DISPLAY=", "--env", "DBUS_SESSION_BUS_ADDRESS=",
            "--env", "DBUS_SYSTEM_BUS_ADDRESS=unix:path=/fixture/state/runtime/no-system-bus"]
    for key, value in labels.items():
        argv += ["--label", key + "=" + value]
    mounts = [(state, "/fixture/state", False), (base / "source", "/fixture/source", True),
              (artifacts, "/candidate", True), (binary, "/fixture/bin/tailnet-api-store-fixture", True),
              *runtime]
    for path, target, readonly in mounts:
        if not path.exists():
            raise RuntimeError("native runtime dependency missing")
        argv += ["--mount", f"type=bind,source={path},target={target}" + (",readonly" if readonly else "")]
    argv += ["--entrypoint", "/usr/bin/python3", store.IMAGE,
             "/fixture/source/scripts/tailnet-api-status-fixture/control.py", "--sha256", frozen["horizon"]["sha256"]]
    if args.check_only:
        argv += ["--check-only"]
    if args.expect_api_status:
        argv += ["--expect-api-status"]
    record = {"container_name": name, "container_id": None, "daemon_id": store.docker_id(), "labels": labels,
              "artifacts": frozen, "test_artifact_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "build_command": command, "source_inputs_sha256": inputs,
              "manifest_sha256": hashlib.sha256(args.manifest.read_bytes()).hexdigest(),
              "build_environment_sha256": {key: hashlib.sha256(os.environ[key].encode()).hexdigest() for key in ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_JOBS", "CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_TEST_DEBUG", "RUSTUP_TOOLCHAIN"] if key in os.environ},
              "source_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip(),
              "source_files_sha256": {str(p.relative_to(base / "source")): hashlib.sha256(p.read_bytes()).hexdigest()
                                      for p in (base / "source").rglob("*") if p.is_file()},
              "argv": argv, "check_only": args.check_only, "expect_api_status": args.expect_api_status, "cleanup_confirmed": False}
    ownership = base / "ownership.json"
    encoded = json.dumps(record, indent=2) + "\n"
    if len(encoded.encode()) > 4 * 1024 * 1024:
        raise RuntimeError("ownership evidence exceeds bounded reader")
    ownership.write_text(encoded)
    record["container_id"] = subprocess.check_output(argv, text=True).strip()
    ownership.write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps({"ownership": str(ownership), "container": record["container_id"],
                      "state": str(state), "check_only": args.check_only}), flush=True)


def action(args):
    record = owned(args.ownership)
    container = record["container_id"]
    if args.action in ("keyring-off", "keyring-on"):
        subprocess.run(["docker", "kill", "--signal=" + ("USR1" if args.action == "keyring-off" else "USR2"), container], check=True)
    elif args.action == "cli":
        if not record["expect_api_status"]:
            raise RuntimeError("baseline mode does not invoke the unshipped tailnets CLI route")
        subprocess.run(["docker", "exec", "--env", "DBUS_SESSION_BUS_ADDRESS=unix:path=/fixture/state/runtime/bus",
                        "--env", "DBUS_SYSTEM_BUS_ADDRESS=unix:path=/fixture/state/runtime/no-system-bus",
                        container, "/candidate/cloud_deploy", "tailnets", "/fixture/state/horizon/cloud"], check=True)
    else:
        state = args.ownership.parent / "state"
        subprocess.run(["docker", "logs", container], stdout=(state / "container.log").open("wb"), stderr=subprocess.STDOUT, check=True)
        value = json.loads(subprocess.check_output(["docker", "inspect", container], text=True))[0]
        if value["State"]["Running"]:
            raise RuntimeError("close owned Horizon normally and stop its viewer/proxy before cleanup")
        result_path = state / "native-result.json"
        result = json.loads(result_path.read_text()) if result_path.is_file() else {}
        if not (result.get("state") == "passed" and result.get("owned_service_empty") and result.get("owned_processes_stopped") and result.get("target_expired") and value["State"]["ExitCode"] == 0):
            raise RuntimeError("native cleanup not qualified; preserve exact container/state")
        if not record["check_only"]:
            proxy = state / "vnc-proxy-result.json"
            if (state / "vnc-address.json").exists() or not proxy.is_file() or not json.loads(proxy.read_text()).get("owned_relay_stopped"):
                raise RuntimeError("owned VNC relay cleanup unresolved")
        subprocess.run(["docker", "rm", container], check=True)
        store = storage()
        check = subprocess.run(["docker", "inspect", container], text=True, capture_output=True)
        if store.docker_id() != record["daemon_id"] or not store.absent(check, container):
            raise RuntimeError("container absence uncertain; preserve synthetic state")
        record["cleanup_confirmed"] = True
        for relative in ["private-home", "runtime", "horizon"]:
            path = state / relative
            if path.exists():
                if path.is_symlink():
                    raise RuntimeError("synthetic cleanup path refused")
                shutil.rmtree(path)
        (state / "keyring-password").unlink(missing_ok=True)
        args.ownership.write_text(json.dumps(record, indent=2) + "\n")
        print(json.dumps({"cleanup_confirmed": True, "evidence": str(args.ownership.parent)}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    launch = sub.add_parser("start")
    launch.add_argument("--evidence-dir", type=Path, required=True)
    launch.add_argument("--manifest", type=Path, required=True)
    launch.add_argument("--target-dir", type=Path)
    launch.add_argument("--check-only", action="store_true")
    launch.add_argument("--expect-api-status", action="store_true")
    for name in ["keyring-off", "keyring-on", "cli", "cleanup"]:
        sub.add_parser(name).add_argument("--ownership", type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    start(args) if args.action == "start" else action(args)


if __name__ == "__main__":
    main()
