"""Canonical Linux casting benchmark; all network endpoints are owned loopback sockets."""
import argparse
import hashlib
from importlib.metadata import version
import json
import math
import os
from pathlib import Path
import platform
import signal
import statistics
import subprocess
import sys
import threading
import time

from receiver import Receiver

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
FRAME_BYTES = 64 * 33 * 3


def execute(command, **kwargs):
    return subprocess.run(command, check=True, timeout=180, **kwargs)


def fingerprint(paths):
    digest = hashlib.sha256()
    for path in sorted(paths):
        digest.update(str(path.relative_to(REPO)).encode())
        digest.update(path.read_bytes())
    return digest.hexdigest()


def build(backend, root):
    command = ["cargo", "build", "--locked", "--release", "-p", "horizon-cast",
               "--example", "cast_bench", "--message-format=json"]
    if backend == "gpu":
        command.extend(["--features", "nvenc"])
    with (root / "build.log").open("w") as errors:
        output = execute(command, cwd=REPO, capture_output=False, stdout=subprocess.PIPE,
                         stderr=errors, text=True).stdout
    artifacts = [json.loads(line) for line in output.splitlines() if line.startswith("{")]
    binary = next(Path(item["executable"]) for item in artifacts
                  if item.get("reason") == "compiler-artifact"
                  and item.get("target", {}).get("name") == "cast_bench"
                  and item.get("executable"))
    frozen = root / "cast_bench"
    frozen.write_bytes(binary.read_bytes())
    frozen.chmod(0o700)
    return frozen


def process_tree(pid):
    pending, found = [pid], set()
    while pending:
        current = pending.pop()
        if current in found:
            continue
        found.add(current)
        try:
            for task in Path(f"/proc/{current}/task").iterdir():
                pending.extend(int(child) for child in (task / "children").read_text().split())
        except (FileNotFoundError, ProcessLookupError):
            pass
    return found


def group_members(group):
    members = []
    for directory in Path("/proc").iterdir():
        if not directory.name.isdigit():
            continue
        try:
            fields = (directory / "stat").read_text().rsplit(")", 1)[1].split()
            if fields[0] != "Z" and int(fields[2]) == group and int(fields[3]) == group:
                members.append(int(directory.name))
        except (FileNotFoundError, ProcessLookupError):
            pass
    return members


def stop_group(process):
    # Every sender starts a new session; this group contains only task-owned descendants.
    leaked = process.poll() is not None and bool(group_members(process.pid))
    for signal_number, seconds in [(signal.SIGTERM, 1), (signal.SIGKILL, 3)]:
        if not group_members(process.pid):
            break
        try:
            os.killpg(process.pid, signal_number)
        except ProcessLookupError:
            break
        deadline = time.monotonic() + seconds
        while group_members(process.pid) and time.monotonic() < deadline:
            time.sleep(0.02)
    process.wait(timeout=1)
    if group_members(process.pid):
        raise RuntimeError("sender process group did not stop")
    return leaked


def sample(process, readings, gpu):
    try:
        sample_resources(process, readings, gpu)
    except Exception as error:
        readings["sampler_error"] = str(error)


def sample_resources(process, readings, gpu):
    last_gpu = 0.0
    while process.poll() is None:
        pids = process_tree(process.pid)
        rss = 0
        for pid in pids:
            try:
                for line in Path(f"/proc/{pid}/status").read_text().splitlines():
                    if line.startswith("VmRSS:"):
                        rss += int(line.split()[1])
            except (FileNotFoundError, ProcessLookupError):
                pass
        readings["peak_tree_rss_kib"] = max(readings["peak_tree_rss_kib"], rss)
        if gpu and time.monotonic() - last_gpu >= 0.5:
            last_gpu = time.monotonic()
            query = subprocess.run(["nvidia-smi", "--query-compute-apps=pid,used_gpu_memory",
                                    "--format=csv,noheader,nounits"], capture_output=True,
                                   text=True, timeout=5)
            if query.returncode:
                readings["gpu_error"] = query.stderr.strip() or "GPU sampling failed"
                return
            total, observed = 0, False
            for line in query.stdout.splitlines():
                fields = line.split(",")
                if len(fields) == 2 and fields[0].strip().isdigit() and int(fields[0]) in pids:
                    if not fields[1].strip().isdigit():
                        readings["gpu_error"] = "GPU allocation measurement unavailable"
                        return
                    total += int(fields[1]); observed = True
            if observed:
                readings["gpu_memory_mib"] = max(readings.get("gpu_memory_mib") or 0, total)
        time.sleep(0.05)


def decode(root, result, receiver):
    if receiver.errors or len(receiver.sessions) != 1:
        raise RuntimeError(f"receiver failed: {receiver.errors}")
    state = receiver.sessions[0]
    if not state["teardown"] or not state["events"] or state["frames"] != result["total_frames"]:
        raise RuntimeError(f"receiver/sender accounting or teardown failed: {state}")
    stream = root / "received.h264"
    info = json.loads(execute(["ffprobe", "-v", "error", "-count_frames", "-show_streams",
                               "-of", "json", str(stream)], capture_output=True, text=True).stdout)["streams"][0]
    width, height = result["width"], result["height"]
    configurations = state.get("config_dimensions", [])
    if state.get("configs", 0) < 1 or len(configurations) != state["configs"] * 3:
        raise RuntimeError("receiver video configuration coverage incomplete")
    if any(tuple(dimensions) != (width, height) for dimensions in configurations):
        raise RuntimeError("receiver video configuration differs from requested canvas")
    if (info["width"], info["height"]) != (width, height):
        raise RuntimeError("decoded canvas differs from requested resolution/orientation")
    if int(info["nb_read_frames"]) != state["frames"]:
        raise RuntimeError("independent decoder frame coverage differs from receiver count")
    if state["frames"] < 2:
        raise RuntimeError("at least two decoded frames are required to verify advancement")
    # First row contains each embedded counter bit twice; remaining rows sample the full canvas.
    graph = "split[a][b];[a]crop=iw:32:0:0,scale=64:1:flags=neighbor[ids];[b]scale=64:32:flags=neighbor[image];[ids][image]vstack"
    pixels = execute(["ffmpeg", "-v", "error", "-i", str(stream), "-filter_complex", graph,
                      "-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1"], capture_output=True).stdout
    if len(pixels) != FRAME_BYTES * state["frames"]:
        raise RuntimeError("decoded quality fixture coverage incomplete")
    identifiers, errors = [], []
    for offset in range(0, len(pixels), FRAME_BYTES):
        identifier = sum((pixels[offset + (2 * bit + 1) * 3] > 127) << bit for bit in range(32))
        if identifier < 1 or identifier > result["submitted"] or (identifiers and identifier <= identifiers[-1]):
            raise RuntimeError("decoded frame IDs are invalid, duplicated or out of order")
        identifiers.append(identifier)
        for row in range(32):
            y = (2 * row + 1) * height // 64
            if y < 32 or min(y % 16, 16 - y % 16) < 3:
                continue
            for column in range(64):
                x = (2 * column + 1) * width // 128
                if min(x % 16, 16 - x % 16) < 3:
                    continue
                expected = 30 if (x // 16 + y // 16) % 2 == 0 else 180
                at = offset + ((row + 1) * 64 + column) * 3
                errors.extend(abs(value - expected) for value in pixels[at:at + 3])
    if not errors or statistics.mean(errors) > 8 or sorted(errors)[int(0.95 * (len(errors) - 1))] > 20:
        raise RuntimeError("decoded image quality regressed")
    quality = {"decoded_frames": len(identifiers), "first_id": identifiers[0], "last_id": identifiers[-1],
               "quality_samples": len(errors), "mean_channel_error": statistics.mean(errors)}
    (root / "decode.json").write_text(json.dumps(quality, indent=2))
    return quality


def validate_backend(result, backend, scaler):
    expected = "h264_nvenc" if backend == "gpu" else "libx264"
    if result.get("encoder") != expected:
        raise RuntimeError("requested benchmark encoder unavailable; software fallback cannot qualify GPU")
    actual = result.get("scaler")
    if actual not in {"cpu", "cuda"} or (backend == "cpu" and actual != "cpu"):
        raise RuntimeError("invalid benchmark scaler evidence")
    if scaler is not None and actual != scaler:
        raise RuntimeError("requested benchmark scaler unavailable")


def run_case(binary, root, arguments, resolution, orientation):
    receiver = Receiver(root)
    gpu = arguments.backend == "gpu"
    expected = "h264_nvenc" if gpu else "libx264"
    command = ["/usr/bin/time", "-f", "%U %S %M", "-o", str(root / "resource.txt"), str(binary),
               f"127.0.0.1:{receiver.port}", resolution, orientation, str(arguments.seconds),
               str(arguments.input_fps), expected]
    readings = {"peak_tree_rss_kib": 0, "gpu_memory_mib": None}
    process = None
    try:
        with (root / "sender.log").open("w") as output, (root / "sender-errors.log").open("w") as errors:
            process = subprocess.Popen(command, stdout=output, stderr=errors, start_new_session=True)
            monitor = threading.Thread(target=sample, args=(process, readings, gpu))
            monitor.start()
            try:
                code = process.wait(timeout=arguments.seconds + 35)
            finally:
                try:
                    leaked = stop_group(process)
                finally:
                    monitor.join(timeout=8)
                    if monitor.is_alive():
                        raise RuntimeError("resource sampler did not stop")
                if leaked:
                    raise RuntimeError("sender exited with live descendants; cleaned up but run invalid")
        if code:
            raise RuntimeError(f"sender exited {code}; see sender-errors.log")
    finally:
        receiver.close()
    result = json.loads((root / "sender.log").read_text())
    validate_backend(result, arguments.backend, arguments.scaler)
    quality = decode(root, result, receiver)
    user, system, maximum = map(float, (root / "resource.txt").read_text().split())
    cpu = (user + system) * 1000 / result["total_frames"]
    if readings.get("sampler_error") or readings["peak_tree_rss_kib"] <= 0:
        raise RuntimeError(readings.get("sampler_error") or "no process-tree memory samples")
    if readings.get("gpu_error") or (gpu and readings["gpu_memory_mib"] is None):
        raise RuntimeError(readings.get("gpu_error") or "no process-attributed GPU allocation sample")
    result.update(readings, **quality, cpu_ms_per_frame=cpu, maximum_process_rss_kib=maximum,
                  wall_ms_per_frame=result["seconds"] * 1000 / result["frames"])
    metric = {"cpu": "cpu_ms_per_frame", "memory": "peak_tree_rss_kib", "throughput": "wall_ms_per_frame",
              "gpu-memory": "gpu_memory_mib"}[arguments.objective]
    result["score"] = result[metric]
    if not math.isfinite(result["score"]) or result["score"] <= 0:
        raise RuntimeError("benchmark score must be finite and positive")
    (root / "result.json").write_text(json.dumps(result, indent=2))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", choices=["cpu", "gpu"], default="cpu")
    parser.add_argument("--scaler", choices=["cpu", "cuda"], help="require this actual scaler; never substitute another path")
    parser.add_argument("--objective", choices=["cpu", "memory", "throughput", "gpu-memory"], default="cpu")
    parser.add_argument("--seconds", type=int, choices=range(1, 121), default=4)
    parser.add_argument("--input-fps", type=int, choices=range(1, 121), default=120)
    parser.add_argument("--resolutions", default="720p,1080p,4k")
    parser.add_argument("--orientations", default="landscape,portrait")
    parser.add_argument("--output", type=Path)
    arguments = parser.parse_args()
    if not __debug__:
        parser.error("optimized Python disables protocol assertions; use normal Python")
    if sys.platform != "linux" or (arguments.objective == "gpu-memory" and arguments.backend != "gpu"):
        parser.error("Linux is required; gpu-memory additionally requires the GPU backend")
    if arguments.backend == "cpu" and arguments.scaler == "cuda":
        parser.error("CUDA scaling requires the GPU backend")
    resolutions, orientations = arguments.resolutions.split(","), arguments.orientations.split(",")
    if not set(resolutions) <= {"720p", "1080p", "4k"} or not set(orientations) <= {"landscape", "portrait"}:
        parser.error("invalid resolution/orientation matrix")
    root = (arguments.output or HERE / "runs" / f"{time.time_ns()}-{arguments.backend}").resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    benchmark_files = [*HERE.glob("*.py"), HERE / "bench.sh", HERE / "requirements.txt",
                       REPO / "crates/horizon-cast/examples/cast_bench.rs"]
    source_files = [*(REPO / "crates/horizon-cast/src").rglob("*.rs"), REPO / "Cargo.lock",
                    REPO / "Cargo.toml", REPO / "crates/horizon-cast/Cargo.toml"]
    manifest = {"backend": arguments.backend, "requested_scaler": arguments.scaler,
                "objective": arguments.objective, "input_fps": arguments.input_fps,
                "seconds": arguments.seconds, "resolutions": resolutions, "orientations": orientations,
                "machine": platform.machine(), "kernel": platform.release(),
                "commit": execute(["git", "rev-parse", "HEAD"], cwd=REPO, capture_output=True, text=True).stdout.strip(),
                "benchmark_sha256": fingerprint(benchmark_files),
                "source_sha256": fingerprint(source_files),
                "rustc": execute(["rustc", "--version"], capture_output=True, text=True).stdout.strip(),
                "ffmpeg": execute(["ffmpeg", "-version"], capture_output=True, text=True).stdout.splitlines()[0]}
    results = []
    try:
        cpu_model = next((line.partition(":")[2].strip() for line in Path("/proc/cpuinfo").read_text().splitlines()
                          if line.startswith("model name")), platform.processor())
        manifest["cpu_model"] = cpu_model
        manifest["cpu_count"] = os.cpu_count()
        manifest["python"] = platform.python_version()
        manifest["reference_versions"] = {name: version(name) for name in ["pyatv", "cryptography"]}
        if arguments.backend == "gpu":
            manifest["gpu"] = execute(["nvidia-smi", "--query-gpu=name,driver_version,memory.total",
                                       "--format=csv,noheader,nounits"], capture_output=True, text=True).stdout.strip()
        binary = build(arguments.backend, root)
        manifest["binary_sha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
        (root / "manifest.json").write_text(json.dumps(manifest, indent=2))
        for resolution in resolutions:
            for orientation in orientations:
                case = root / f"{resolution}-{orientation}"
                case.mkdir()
                result = run_case(binary, case, arguments, resolution, orientation)
                results.append(result)
                print(json.dumps({"case": case.name, "score": result["score"], "decoded_frames": result["decoded_frames"]}), flush=True)
        if fingerprint(benchmark_files) != manifest["benchmark_sha256"] or fingerprint(source_files) != manifest["source_sha256"]:
            raise RuntimeError("benchmark or source changed during measurement; re-baseline")
        if len({result["scaler"] for result in results}) != 1:
            raise RuntimeError("mixed scaler paths cannot share a benchmark score; qualify them separately")
        summary = {"status": "PASS", "score": statistics.mean(r["score"] for r in results),
                   "objective": arguments.objective, "cases": results, "evidence": str(root)}
        (root / "summary.json").write_text(json.dumps(summary, indent=2))
        print(json.dumps(summary), flush=True)
    except Exception as error:
        (root / "failure.json").write_text(json.dumps({"status": "FAIL", "error": str(error), "completed_cases": results}, indent=2))
        raise


if __name__ == "__main__":
    main()
