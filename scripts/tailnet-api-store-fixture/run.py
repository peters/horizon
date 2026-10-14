#!/usr/bin/env python3
"""Build and run the synthetic Linux OS-store test without a desktop bus."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import uuid

IMAGE = "ghcr.io/peters/horizon-worker-base@sha256:6550aacb6c60187cd346a795beb3c8bffd998fe9b95ec74f5fb844cf78870bdc"
REPO = Path(__file__).resolve().parents[2]
CONTROL = Path(__file__).resolve().with_name("secret_service.py")


def docker_id():
    value = subprocess.check_output(
        ["docker", "info", "--format", "{{.ID}}"], text=True, timeout=20
    ).strip()
    if not value:
        raise RuntimeError("Docker identity unavailable")
    return value


def absent(result, name):
    return result.returncode != 0 and re.fullmatch(
        r"error(?::| response from daemon:) no such (?:object|container): " + re.escape(name),
        result.stderr.strip(), re.IGNORECASE,
    ) is not None


def build(state, target):
    command = ["cargo", "test", "-p", "horizon-cloud", "--lib", "--no-run", "--locked", "--message-format=json"]
    if target:
        command += ["--target-dir", str(target)]
    result = subprocess.run(command, cwd=REPO, text=True, capture_output=True)
    (state / "build.stdout").write_text(result.stdout)
    (state / "build.stderr").write_text(result.stderr)
    if result.returncode:
        raise RuntimeError("Fixture compilation failed; see private build logs")
    artifacts = []
    for line in result.stdout.splitlines():
        record = json.loads(line)
        if record.get("reason") == "compiler-artifact" and record.get("target", {}).get("name") == "horizon_cloud" and record.get("executable"):
            artifacts.append(Path(record["executable"]))
    if len(artifacts) != 1:
        raise RuntimeError("Exact library test artifact unavailable")
    binary = state / "tailnet-api-store-fixture"
    shutil.copyfile(artifacts[0], binary)
    binary.chmod(0o500)
    return binary, command


def run(args):
    if sys.platform != "linux" or platform.machine() != "x86_64":
        raise RuntimeError("This Secret Service fixture requires Linux x86_64")
    for path in [Path("/usr/lib/x86_64-linux-gnu"), *[Path("/usr/bin") / name for name in ["dbus-daemon", "gnome-keyring-daemon", "gdbus"]]]:
        if not path.exists():
            raise RuntimeError("Required fixture tools/libraries unavailable")
    base = args.evidence_dir.absolute()
    base.mkdir(parents=True, exist_ok=True, mode=0o700)
    if base.is_symlink() or base.stat().st_mode & 0o077:
        raise RuntimeError("Evidence directory must be private and not a symlink")
    state = base / ("run-" + uuid.uuid4().hex)
    state.mkdir(mode=0o700)
    binary, command = build(state, args.target_dir)
    control = state / "secret_service.py"
    shutil.copyfile(CONTROL, control)
    control.chmod(0o400)
    nonce = uuid.uuid4().hex
    name = "horizon-tailnet-api-store-" + nonce
    daemon = docker_id()
    labels = {"horizon.task": "1335", "horizon.fixture": "tailnet-api-store", "horizon.fixture-owner": nonce}
    argv = ["docker", "run", "--rm", "--pull=never", "--init", "--name", name,
            "--network", "none", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
            "--read-only", "--user", f"{os.getuid()}:{os.getgid()}", "--pids-limit", "128", "--memory", "512m",
            "--tmpfs", "/tmp:rw,exec,nosuid,nodev", "--tmpfs", "/run:rw,nosuid,nodev"]
    for key, value in labels.items():
        argv += ["--label", key + "=" + value]
    for source, destination in [(state, "/fixture/state"), (binary, "/fixture/bin/tailnet-api-store-fixture"), (control, "/fixture/secret_service.py"), (Path("/usr/lib/x86_64-linux-gnu"), "/usr/lib/x86_64-linux-gnu"), *[(Path("/usr/bin") / name, "/usr/bin/" + name) for name in ["dbus-daemon", "gnome-keyring-daemon", "gdbus"]]]:
        argv += ["--mount", f"type=bind,source={source},target={destination}" + ("" if source == state else ",readonly")]
    argv += ["--entrypoint", "/usr/bin/python3", IMAGE, "/fixture/secret_service.py"]
    record = {"container": name, "daemon_id": daemon, "labels": labels, "runtime_image": IMAGE,
              "build_command": command, "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "control_sha256": hashlib.sha256(control.read_bytes()).hexdigest(), "argv": argv,
              "source_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip(),
              "source_diff_sha256": hashlib.sha256(subprocess.check_output(["git", "diff", "HEAD"], cwd=REPO)).hexdigest(),
              "cleanup_confirmed": False}
    inputs = [REPO / "Cargo.toml", REPO / "Cargo.lock", REPO / "rust-toolchain.toml", REPO / ".cargo/config.toml", *sorted((REPO / "crates/horizon-cloud").rglob("*.rs")), REPO / "crates/horizon-cloud/Cargo.toml", Path(__file__).resolve(), CONTROL]
    record["source_inputs_sha256"] = {str(path.relative_to(REPO)): hashlib.sha256(path.read_bytes()).hexdigest() for path in inputs}
    record["rustc"] = subprocess.check_output(["rustc", "--version"], text=True).strip()
    record["build_environment_sha256"] = {key: hashlib.sha256(os.environ[key].encode()).hexdigest() for key in ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_JOBS", "CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_TEST_DEBUG", "RUSTUP_TOOLCHAIN"] if key in os.environ}
    evidence = state / "ownership.json"
    evidence.write_text(json.dumps(record, indent=2) + "\n")
    try:
        result = subprocess.run(argv, text=True, capture_output=True, timeout=120)
        record["exit"] = result.returncode
        (state / "container.log").write_text(result.stdout + result.stderr)
    except subprocess.TimeoutExpired as error:
        record["exit"] = 124
        output = (error.stdout or b"") + (error.stderr or b"")
        (state / "container.log").write_bytes(output)
    finally:
        if docker_id() != daemon:
            raise RuntimeError("Docker identity changed; preserve private state")
        check = subprocess.run(["docker", "inspect", "--format", "{{json .Config.Labels}}", name], text=True, capture_output=True, timeout=20)
        if check.returncode == 0:
            actual = json.loads(check.stdout)
            if any(actual.get(key) != value for key, value in labels.items()):
                raise RuntimeError("Container ownership uncertain; preserve private state")
            subprocess.run(["docker", "rm", "--force", name], capture_output=True, check=True, timeout=20)
        check = subprocess.run(["docker", "inspect", name], text=True, capture_output=True, timeout=20)
        if docker_id() != daemon or not absent(check, name):
            raise RuntimeError("Container absence uncertain; preserve private state")
        record["cleanup_confirmed"] = True
        record["metadata_after_run"] = {path.name: {"bytes": path.stat().st_size, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()} for path in (state / "horizon/cloud").glob("*.json") if path.is_file() and not path.is_symlink()}
        evidence.write_text(json.dumps(record, indent=2) + "\n")
        for relative in ["private-home", "runtime", "horizon"]:
            path = state / relative
            if path.exists():
                if path.is_symlink() or not path.resolve().is_relative_to(state.resolve()):
                    raise RuntimeError("Synthetic cleanup path uncertain")
                shutil.rmtree(path)
        password = state / "keyring-password"
        if password.exists():
            if password.is_symlink():
                raise RuntimeError("Synthetic password path uncertain")
            password.unlink()
        record["owned_secret_files_removed"] = True
        evidence.write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps({"evidence": str(state), "exit": record["exit"], "cleanup_confirmed": True}))
    return record["exit"]


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence-dir", type=Path, required=True)
    parser.add_argument("--target-dir", type=Path)
    os.umask(0o077)
    raise SystemExit(run(parser.parse_args()))
