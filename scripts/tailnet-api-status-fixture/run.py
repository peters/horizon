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
import stat
import re
import selectors
import time
import subprocess
import sys
import uuid

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
DOCKER_TIMEOUT = 20
MAX_SOURCE_BYTES = 128 * 1024 * 1024
MAX_ARTIFACT_BYTES = 2 * 1024 * 1024 * 1024
CARGO_INPUTS = ("Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml")
BUILD_ROOTS = ("crates", "assets", "vendor", ".cargo")
MAX_LOG_BYTES = 2 * 1024 * 1024
FIXTURE_FILES = (
    "scripts/tailnet-api-status-fixture/run.py",
    "scripts/tailnet-api-status-fixture/control.py",
    "scripts/tailnet-api-status-fixture/observer.py",
    "scripts/tailnet-api-status-fixture/vnc_proxy.py",
    "scripts/tailnet-api-status-fixture/safe_io.py",
    "scripts/tailnet-api-status-fixture/tests/test_controls.py",
    "scripts/tailnet-api-status-fixture/tests/test_launch.py",
    "scripts/tailnet-api-status-fixture/tests/test_safe_io.py",
    "scripts/tailnet-api-status-fixture/tests/test_source_io.py",
    "scripts/tailnet-api-store-fixture/run.py",
    "scripts/tailnet-api-store-fixture/secret_service.py",
)

def private_io():
    spec = importlib.util.spec_from_file_location("native_fixture_safe_io", HERE / "safe_io.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

def storage():
    path = HERE.parent / "tailnet-api-store-fixture/run.py"
    spec = importlib.util.spec_from_file_location("store_fixture_build", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

def inspect_owned(record):
    store = storage()
    if store.docker_id() != record["daemon_id"]:
        raise RuntimeError("Docker identity changed; preserve state")
    reference = record.get("container_id") or record["container_name"]
    result = subprocess.run(["docker", "inspect", reference], text=True, capture_output=True, timeout=DOCKER_TIMEOUT)
    if result.returncode:
        raise RuntimeError("fixture container unresolved; preserve launch journal")
    values = json.loads(result.stdout)
    if not isinstance(values, list) or len(values) != 1:
        raise RuntimeError("fixture container ownership uncertain")
    actual = values[0]
    identity = actual.get("Id")
    if (not isinstance(identity, str) or re.fullmatch(r"[0-9a-f]{64}", identity) is None
            or actual.get("Name") != "/" + record["container_name"]
            or (record.get("container_id") is not None and identity != record["container_id"])
            or any(actual.get("Config", {}).get("Labels", {}).get(key) != value for key, value in record["labels"].items())
            or store.docker_id() != record["daemon_id"]):
        raise RuntimeError("fixture container ownership uncertain")
    return actual

def owned(path):
    record = private_io().read_json(path, 4 * 1024 * 1024)
    actual = inspect_owned(record)
    if record.get("container_id") is None:
        record["container_id"] = actual["Id"]
        record["launch_reconciled"] = True
        private_io().replace_json(path, record)
    return record


def freeze_artifacts(manifest, destination, manifest_bytes=None):
    if manifest_bytes is None:
        manifest_bytes = private_io().read_public_bytes(manifest, 1024 * 1024)
    artifacts = json.loads(manifest_bytes)
    result = {}
    for key, name in [("horizon", "horizon"), ("cloud_deploy", "cloud_deploy"), ("horizon_device", "horizon-device")]:
        item = artifacts[key]
        if (not isinstance(item, dict) or not isinstance(item.get("path"), str)
                or not isinstance(item.get("sha256"), str) or re.fullmatch(r"[0-9a-f]{64}", item["sha256"]) is None):
            raise RuntimeError("artifact manifest entry refused")
        target = destination / name
        digest = private_io().copy_verified(Path(item["path"]), target, MAX_ARTIFACT_BYTES,
                                            expected=item["sha256"], mode=0o500)
        result[key] = {"path": str(target), "sha256": digest}
    return result

def source_bytes(name):
    parts = Path(name).parts
    if not parts or Path(name).is_absolute() or any(part in ("", ".", "..") for part in parts) or "/".join(parts) != name:
        raise RuntimeError("checkout input path refused")
    try:
        return private_io().read_public_bytes(REPO / name, MAX_SOURCE_BYTES)
    except RuntimeError as error:
        raise RuntimeError("checkout input file refused") from error

def source_inputs():
    untracked = subprocess.check_output(["git", "ls-files", "--others", "--exclude-standard", "-z", "--",
                                         *BUILD_ROOTS, *CARGO_INPUTS], cwd=REPO, timeout=20)
    # Ignored config can override tracked config too. Refuse its name without reading it.
    cargo_untracked = subprocess.check_output(["git", "ls-files", "--others", "-z", "--", ".cargo", *CARGO_INPUTS],
                                             cwd=REPO, timeout=20)
    if untracked or cargo_untracked:
        raise RuntimeError("untracked build inputs require staging before qualification")
    tracked = subprocess.check_output(["git", "ls-files", "--cached", "-z"], cwd=REPO, timeout=20).decode().split("\0")
    names = {name for name in tracked if name in CARGO_INPUTS
             or name.startswith(tuple(root + "/" for root in BUILD_ROOTS))}
    names.update(FIXTURE_FILES)
    return {name: hashlib.sha256(source_bytes(name)).hexdigest() for name in sorted(names)}

def freeze_controls(destination, inputs):
    hashes = {}
    for name, expected in inputs.items():
        if name not in FIXTURE_FILES:
            continue
        target = destination / name.removeprefix("scripts/")
        target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        try:
            digest = private_io().copy_verified(REPO / name, target, MAX_SOURCE_BYTES, expected=expected)
        except RuntimeError as error:
            raise RuntimeError("checkout input changed or refused while freezing") from error
        hashes[str(target.relative_to(destination.parent))] = digest
    return hashes


def inventory_digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()

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
    try:
        xkb = Path("/usr/share/X11/xkb").resolve(strict=True)
        share = Path("/usr/share").resolve(strict=True)
        evdev = (xkb / "rules/evdev").resolve(strict=True)
    except (OSError, RuntimeError) as error:
        raise RuntimeError("native XKB directory or rules/evdev unavailable") from error
    if not xkb.is_relative_to(share) or xkb == share or not evdev.is_relative_to(xkb) or not evdev.is_file():
        raise RuntimeError("resolved native XKB rules refused")
    mounts.append((Path("/usr/share/X11"), "/usr/share/X11", True))
    mounts.append((xkb, "/fixture/xkb", True))
    for directory in ["/usr/share/fonts", "/usr/share/openbox", "/usr/share/vulkan/icd.d"]:
        path = Path(directory)
        if path.exists():
            if not path.is_dir():
                raise RuntimeError("native runtime directory refused: " + directory)
            mounts.append((path, directory, True))
    return mounts

def launch_container(argv, ownership, record):
    private_io().write_exclusive_json(ownership, record)
    try:
        identity = subprocess.check_output(argv, text=True, timeout=DOCKER_TIMEOUT).strip()
        if re.fullmatch(r"[0-9a-f]{64}", identity) is None:
            raise RuntimeError("Docker launch response invalid; preserve journal")
        record["container_id"] = identity
        inspect_owned(record)
        record["launch_phase"] = "created"
        private_io().replace_json(ownership, record)
        subprocess.check_output(["docker", "start", identity], text=True, timeout=DOCKER_TIMEOUT)
        record["launch_phase"] = "started"
        private_io().replace_json(ownership, record)
    except (subprocess.SubprocessError, RuntimeError, ValueError, OSError) as error:
        journal = private_io().read_json(ownership, 4 * 1024 * 1024)
        journal["launch_uncertain"] = True
        private_io().replace_json(ownership, journal)
        try:
            owned(ownership)
        except (RuntimeError, subprocess.SubprocessError, ValueError, OSError):
            pass
        raise RuntimeError("Docker launch uncertain; reconcile the exact recorded ownership") from error

def collect_logs(container, path):
    with private_io().open_exclusive(path) as output:
        process = subprocess.Popen(["docker", "logs", "--tail", "200", "--timestamps", container],
                                   stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        deadline, length = time.monotonic() + DOCKER_TIMEOUT, 0
        try:
            with selectors.DefaultSelector() as poll:
                poll.register(process.stdout, selectors.EVENT_READ)
                while True:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise RuntimeError("Docker log collection timed out; preserve state")
                    if not poll.select(timeout=min(.1, remaining)):
                        continue
                    chunk = os.read(process.stdout.fileno(), min(65536, MAX_LOG_BYTES + 1 - length))
                    if not chunk:
                        break
                    length += len(chunk)
                    if length > MAX_LOG_BYTES:
                        raise RuntimeError("Docker log collection oversized; preserve state")
                    output.write(chunk)
            if process.wait(timeout=max(.001, deadline - time.monotonic())):
                raise RuntimeError("Docker log collection failed; preserve state")
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=3)
            process.stdout.close()


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
    # Candidate state is writable; host relay custody must never be mounted there.
    (base / "relay").mkdir(mode=0o700)
    artifacts = base / "artifacts"
    artifacts.mkdir(mode=0o700)
    manifest_bytes = private_io().read_public_bytes(args.manifest, 1024 * 1024)
    frozen = freeze_artifacts(args.manifest, artifacts, manifest_bytes)
    inputs = source_inputs()
    built_binary, command = store.build(state, args.target_dir)
    binary = artifacts / "tailnet-api-store-fixture"
    test_digest = private_io().copy_verified(built_binary, binary, MAX_ARTIFACT_BYTES, mode=0o500)
    after_build = source_inputs()
    if inputs != after_build:
        raise RuntimeError("source changed during fixture build; preserve evidence")
    source = base / "source/scripts"
    source.mkdir(mode=0o700, parents=True)
    controls = freeze_controls(source, inputs)
    after_freeze = source_inputs()
    if inputs != after_freeze:
        raise RuntimeError("source changed while freezing controls; preserve evidence")
    declaration = {"tracked_roots": BUILD_ROOTS, "root_inputs": CARGO_INPUTS, "fixture_files": FIXTURE_FILES}
    with private_io().open_exclusive(base / "artifact-manifest.json") as output:
        output.write(manifest_bytes)
    name = "horizon-api-native-" + uuid.uuid4().hex
    labels = {"horizon.task": "1335", "horizon.fixture": "api-status-native", "horizon.fixture-owner": uuid.uuid4().hex}
    argv = ["docker", "create", "--pull=never", "--init", "--name", name,
            "--log-driver", "local", "--log-opt", "max-size=1m", "--log-opt", "max-file=2",
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
              "artifacts": frozen, "test_artifact_sha256": test_digest,
              "build_command": command, "source_inputs_sha256": inputs,
              "manifest_sha256": hashlib.sha256(manifest_bytes).hexdigest(),
              "source_input_declaration": declaration, "source_input_declaration_sha256": inventory_digest(declaration),
              "source_inventory_digests": {"before_build": inventory_digest(inputs),
                                           "after_build": inventory_digest(after_build), "after_freeze": inventory_digest(after_freeze)},
              "build_environment_sha256": {key: hashlib.sha256(os.environ[key].encode()).hexdigest() for key in ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_JOBS", "CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_TEST_DEBUG", "RUSTUP_TOOLCHAIN"] if key in os.environ},
              "source_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True, timeout=20).strip(),
              "source_files_sha256": controls,
              "argv": argv, "launch_phase": "intent", "check_only": args.check_only, "expect_api_status": args.expect_api_status, "cleanup_confirmed": False}
    ownership = base / "ownership.json"
    encoded = json.dumps(record, indent=2) + "\n"
    if len(encoded.encode()) > 4 * 1024 * 1024:
        raise RuntimeError("ownership evidence exceeds bounded reader")
    launch_container(argv, ownership, record)
    print(json.dumps({"ownership": str(ownership), "container": record["container_id"],
                      "state": str(state), "relay": str(base / "relay"), "check_only": args.check_only}), flush=True)

def container_absent(record):
    store = storage()
    if not record.get("container_id") or store.docker_id() != record["daemon_id"]:
        raise RuntimeError("container absence uncertain; preserve synthetic state")
    check = subprocess.run(["docker", "inspect", record["container_id"]], text=True, capture_output=True, timeout=DOCKER_TIMEOUT)
    if store.docker_id() != record["daemon_id"]:
        raise RuntimeError("Docker identity changed; preserve state")
    if check.returncode == 0:
        return False
    if not store.absent(check, record["container_id"]):
        raise RuntimeError("container absence uncertain; preserve synthetic state")
    return True

def cleanup_files(ownership):
    with private_io().PrivateDirectory(ownership.parent / "state") as state:
        for name in ["private-home", "runtime", "horizon"]:
            try:
                metadata = os.stat(name, dir_fd=state.descriptor, follow_symlinks=False)
            except FileNotFoundError:
                continue
            if not stat.S_ISDIR(metadata.st_mode):
                raise RuntimeError("synthetic cleanup path refused")
            shutil.rmtree(name, dir_fd=state.descriptor)
        try:
            descriptor = os.open("keyring-password", os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC,
                                 dir_fd=state.descriptor)
        except FileNotFoundError:
            return
        try:
            metadata = os.fstat(descriptor)
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or metadata.st_nlink != 1 or metadata.st_mode & 0o077:
                raise RuntimeError("synthetic password cleanup refused")
            state.unlink_owned("keyring-password", (metadata.st_dev, metadata.st_ino))
        finally:
            os.close(descriptor)

def qualify_cleanup(ownership, record):
    container = record["container_id"]
    log = ownership.parent / ("container-" + uuid.uuid4().hex + ".log")
    collect_logs(container, log)
    record["container_log"] = str(log)
    private_io().replace_json(ownership, record)
    value = inspect_owned(record)
    if value["State"]["Running"]:
        raise RuntimeError("close owned Horizon normally and stop its viewer/proxy before cleanup")
    state = ownership.parent / "state"
    result = private_io().read_json(state / "native-result.json", 4 * 1024 * 1024)
    if not (isinstance(result, dict) and result.get("state") == "passed" and result.get("owned_service_empty") and result.get("owned_processes_stopped") and result.get("target_expired") and value["State"]["ExitCode"] == 0):
        raise RuntimeError("native cleanup not qualified; preserve exact container/state")
    if not record["check_only"]:
        from vnc_proxy import binding_for, read_control
        host = ownership.parent / "relay"
        relay = read_control(host / "vnc-relay-owner.json")
        if relay != binding_for(ownership, record, relay.get("relay_nonce")):
            raise RuntimeError("owned VNC relay binding changed")
        receipt = read_control(host / "vnc-proxy-result.json")
        if os.path.lexists(host / "vnc-address.json") or not receipt.get("owned_relay_stopped") or not receipt.get("owned_children_stopped") or receipt.get("container_id") != container or receipt.get("relay_nonce") != relay["relay_nonce"]:
            raise RuntimeError("owned VNC relay cleanup unresolved")
    record["cleanup_phase"] = "qualified"
    private_io().replace_json(ownership, record)

def cleanup(ownership):
    record = private_io().read_json(ownership, 4 * 1024 * 1024)
    phase = record.get("cleanup_phase")
    if phase not in ("qualified", "absent", "complete"):
        record = owned(ownership)
        qualify_cleanup(ownership, record)
        phase = "qualified"
    absent = container_absent(record)
    if phase in ("absent", "complete") and not absent:
        raise RuntimeError("confirmed container absence changed; preserve state")
    if not absent:
        actual = inspect_owned(record)
        if actual["State"]["Running"]:
            raise RuntimeError("qualified container restarted; preserve state")
        try:
            subprocess.run(["docker", "rm", record["container_id"]], check=True, timeout=DOCKER_TIMEOUT)
        except subprocess.SubprocessError as error:
            if container_absent(record):
                record["cleanup_phase"] = "absent"
                private_io().replace_json(ownership, record)
            raise RuntimeError("container removal uncertain; retry exact cleanup") from error
        if not container_absent(record):
            raise RuntimeError("container absence uncertain; preserve synthetic state")
    record["cleanup_phase"] = "absent"
    private_io().replace_json(ownership, record)
    cleanup_files(ownership)
    record["cleanup_confirmed"] = True
    record["cleanup_phase"] = "complete"
    private_io().replace_json(ownership, record)
    print(json.dumps({"cleanup_confirmed": True, "evidence": str(ownership.parent)}))

def action(args):
    if args.action == "cleanup":
        return cleanup(args.ownership)
    record = owned(args.ownership)
    container = record["container_id"]
    if args.action in ("keyring-off", "keyring-on"):
        subprocess.run(["docker", "kill", "--signal=" + ("USR1" if args.action == "keyring-off" else "USR2"), container], check=True, timeout=DOCKER_TIMEOUT)
    elif args.action == "cli":
        if not record["expect_api_status"]:
            raise RuntimeError("baseline mode does not invoke the unshipped tailnets CLI route")
        subprocess.run(["docker", "exec", "--env", "DBUS_SESSION_BUS_ADDRESS=unix:path=/fixture/state/runtime/bus",
                        "--env", "DBUS_SYSTEM_BUS_ADDRESS=unix:path=/fixture/state/runtime/no-system-bus",
                        container, "/candidate/cloud_deploy", "tailnets", "/fixture/state/horizon/cloud"], check=True, timeout=DOCKER_TIMEOUT)


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
