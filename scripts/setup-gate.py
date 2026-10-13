#!/usr/bin/env python3
"""Run the setup gate: Horizon's proof and desktop preview on a list of real repositories.

The runner builds the horizon-ui test binary, splits the list into lanes, runs the
ignored gate test once per lane and prints a summary. On Ctrl-C or SIGTERM it stops
the processes it started and removes the Docker resources and temporary folders of
this run only. docs/testing/procedures/cloud-setup-gate.md describes the list and the
results. It runs on Linux with Docker.

Usage: setup-gate.py LIST [--out DIR] [--lanes N] [--min-free-gb N]
       [--only PATTERN]... [--binary PATH]
"""

import argparse
import fnmatch
import json
import os
import re
import secrets
import selectors
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
GATE_TEST = "app::cloud_panel::production::repository_setup::proof::gate::proves_each_listed_repository"
DEFAULT_OUT = Path("/var/tmp/horizon-setup-gate")
DEFAULT_LANES = 2
DEFAULT_MIN_FREE_GB = 100
# A heavy entry counts as this many other entries when the runner balances the lanes.
HEAVY_WEIGHT = 3
# How long a lane may run for each entry: above the gate's own limits for one proof (6 hours)
# and its preview (10 minutes), with time for the cleanup.
ENTRY_LIMIT_SECONDS = 7 * 60 * 60
# Above this load average, tests with time limits inside repositories failed for no real reason.
HIGH_LOAD = 30
ENTRY_KEYS = {"repo", "label", "path", "spec", "preview", "heavy", "leave_out"}
LABEL = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,63}")
PHASES = ("Build image", "Run CI in image", "Fast rerun")
HEADER = ["Entry", "Result", "Build", "CI", "Fast", "Preview", "First failure"]
LISTINGS = {
    "container": ["ps", "--all", "--format", "{{.Names}}"],
    "image": ["images", "--format", "{{.Repository}}:{{.Tag}}"],
    "volume": ["volume", "ls", "--format", "{{.Name}}"],
    "network": ["network", "ls", "--format", "{{.Name}}"],
}
# The most output the runner reads from one docker command.
OUTPUT_LIMIT = 16 * 1024 * 1024
# How long one removal command may take.
REMOVAL_SECONDS = 15 * 60
# How long what a finished entry left in Docker may take to go on its own before it is a leak.
LEAK_GRACE_SECONDS = 2 * 60
# In this order: a container holds its image, volume and network.
REMOVALS = (
    ("container", ["rm", "--force", "--volumes"]),
    ("image", ["rmi", "--force"]),
    ("volume", ["volume", "rm", "--force"]),
    ("network", ["network", "rm"]),
)


class ListError(Exception):
    pass


def load_list(path, only=()):
    """The entries of the gate list at `path` that `only` selects, checked, with absolute
    paths and without the jobs that each entry leaves out."""
    items = json.loads(Path(path).read_text(encoding="utf-8"))
    if not isinstance(items, list):
        raise ListError("the list must be a JSON array of entries")
    base = Path(path).resolve().parent
    entries, labels = [], set()
    for number, item in enumerate(items, 1):
        where = f"entry {number}"
        if not isinstance(item, dict):
            raise ListError(f"{where} is not an object")
        unknown = sorted(set(item) - ENTRY_KEYS)
        if unknown:
            raise ListError(f"{where} has unknown keys: {', '.join(unknown)}")
        repo = item.get("repo")
        if not isinstance(repo, str) or not repo:
            raise ListError(f"{where} needs a repo name")
        label = item.get("label", repo)
        if not isinstance(label, str) or not LABEL.fullmatch(label):
            raise ListError(f"{where}: {label!r} cannot name a log file; "
                            "give the entry a label of letters, digits, '.', '_' and '-'")
        if label in labels:
            raise ListError(f"{where}: two entries are called {label}; give one of them another label")
        labels.add(label)
        folder = item.get("path")
        if not isinstance(folder, str) or not folder:
            raise ListError(f"{where} ({label}) needs a path")
        folder = (base / folder).resolve()
        if not folder.is_dir():
            raise ListError(f"{where} ({label}): {folder} is not a folder")
        spec = item.get("spec")
        if not isinstance(spec, dict):
            raise ListError(f"{where} ({label}) needs a spec object")
        leave_out = item.get("leave_out", [])
        if not isinstance(leave_out, list) or not all(isinstance(job, str) for job in leave_out):
            raise ListError(f"{where} ({label}): leave_out must be a list of job names")
        if leave_out:
            jobs = spec.get("jobs")
            if not isinstance(jobs, list):
                raise ListError(f"{where} ({label}): leave_out needs a spec with jobs")
            missing = [job for job in leave_out if job not in jobs]
            if missing:
                raise ListError(f"{where} ({label}): the spec has no job {', '.join(missing)}")
            spec = dict(spec, jobs=[job for job in jobs if job not in leave_out])
            if not spec["jobs"]:
                raise ListError(f"{where} ({label}) leaves out every job")
        preview = item.get("preview")
        if preview is not None and not isinstance(preview, dict):
            raise ListError(f"{where} ({label}): preview must be an object")
        heavy = item.get("heavy", False)
        if not isinstance(heavy, bool):
            raise ListError(f"{where} ({label}): heavy must be true or false")
        if only and not any(fnmatch.fnmatchcase(name, pattern) for name in (label, repo) for pattern in only):
            continue
        entries.append({
            "repo": repo,
            "label": label,
            "path": str(folder),
            "spec": spec,
            "preview": preview,
            "heavy": heavy,
            "leave_out": leave_out,
        })
    if not entries:
        raise ListError("no entry to run" + (" matches --only" if only else ""))
    return entries


def split_lanes(entries, lanes):
    """Every heavy entry goes to the first lane, in list order, so two of them never run
    at the same time. Each other entry goes to the lane with the smallest load."""
    count = max(1, min(lanes, len(entries)))
    split = [[] for _ in range(count)]
    load = [0] * count
    for entry in entries:
        if entry["heavy"]:
            split[0].append(entry)
            load[0] += HEAVY_WEIGHT
    for entry in entries:
        if not entry["heavy"]:
            lane = min(range(count), key=lambda index: (load[index], index))
            split[lane].append(entry)
            load[lane] += 1
    return [lane for lane in split if lane]


def lane_prefix(token, number):
    """The start of the run names in lane `number` of the gate run `token`."""
    return f"gate-{token}-{number}-"


def run_name(prefix, index):
    """The run name of entry `index` of a lane: every Docker resource of it carries it."""
    return f"{prefix}{index:04d}"


def find_test_binary(lines):
    """The test executable of the horizon binary in Cargo's JSON messages, or None."""
    for line in lines:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        target = message.get("target") or {}
        if (message.get("reason") == "compiler-artifact" and target.get("name") == "horizon"
                and "bin" in target.get("kind", []) and (message.get("profile") or {}).get("test")
                and message.get("executable")):
            return message["executable"]
    return None


def build_test_binary():
    print("setup-gate: building the horizon-ui tests", file=sys.stderr)
    try:
        built = subprocess.run(
            ["cargo", "test", "-p", "horizon-ui", "--no-run", "--message-format=json-render-diagnostics"],
            cwd=ROOT, stdout=subprocess.PIPE, text=True, check=False)
    except OSError as error:
        print(f"setup-gate: {error}", file=sys.stderr)
        return None
    # Cargo renders the compiler's diagnostics on stderr, which reaches the terminal.
    if built.returncode != 0:
        return None
    return find_test_binary(built.stdout.splitlines())


def docker(args, timeout=120):
    """Runs a docker command; its standard output, or None when it failed, took longer than
    `timeout` seconds or wrote more than OUTPUT_LIMIT bytes. A cut listing never passes
    for a whole one."""
    try:
        # A session of its own, so a Ctrl-C in the terminal never ends a removal midway.
        process = subprocess.Popen(["docker", *args], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                   stderr=subprocess.DEVNULL, start_new_session=True)
    except OSError:
        return None
    deadline = time.monotonic() + timeout
    output = bytearray()
    with process, selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ)
        while len(output) <= OUTPUT_LIMIT:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            if selector.select(remaining):
                chunk = os.read(process.stdout.fileno(), 65536)
                if not chunk:
                    try:
                        code = process.wait(timeout=max(deadline - time.monotonic(), 0.1))
                    except subprocess.TimeoutExpired:
                        break
                    return output.decode(errors="replace") if code == 0 else None
                output += chunk
        process.kill()
        process.wait()
    return None


def pin_docker():
    """Pins the runner and every lane to the Docker daemon that the Docker CLI uses now,
    as the proof pins its own, so a context switched during a long run changes nothing.
    Why it cannot, or None."""
    host = os.environ.get("DOCKER_HOST") or ""
    if not host:
        host = (docker(["context", "inspect", "--format", "{{.Endpoints.docker.Host}}"], timeout=30) or "").strip()
    # The proof mounts files of this computer, so it accepts only a socket of this computer.
    if not host.startswith("unix://"):
        return (f"Docker here uses {host or 'no daemon that docker context inspect names'}, not a socket of "
                "this computer; switch to a local Docker context")
    os.environ["DOCKER_HOST"] = host
    os.environ.pop("DOCKER_CONTEXT", None)
    return None


def check_preconditions(out, min_free_gb):
    """Why the gate cannot start here, or None."""
    if out == Path("/tmp") or Path("/tmp") in out.parents:
        return f"{out} is under /tmp, which can be a RAM file system; use a folder on disk"
    root = docker(["info", "--format", "{{.DockerRootDir}}"], timeout=30)
    if root is None:
        return "Docker does not answer; start Docker and try again"
    try:
        out.mkdir(parents=True, exist_ok=True)
    except OSError as error:
        return f"cannot make the output folder {out}: {error}"
    if min_free_gb == 0:
        return None
    for place in (out, Path(root.strip())):
        try:
            free = shutil.disk_usage(place).free / 1e9
        except OSError as error:
            return f"cannot read the free disk of {place} ({error}); examine it yourself, then use --min-free-gb 0"
        if free < min_free_gb:
            return f"{place} has {free:.0f} GB free, less than {min_free_gb} GB; free disk or use --min-free-gb"
    return None


def parse_stat(text):
    """The state and session ID in the text of /proc/PID/stat."""
    # The command name in parentheses can hold spaces and parentheses.
    fields = text[text.rindex(")") + 2:].split()
    return fields[0], int(fields[3])


def session_pids(session):
    """The live processes in `session`, a session the runner made for one lane."""
    pids = []
    for name in os.listdir("/proc"):
        if not name.isdigit():
            continue
        try:
            state, owner = parse_stat(Path("/proc", name, "stat").read_text())
        except (OSError, ValueError, IndexError):
            continue
        if owner == session and state != "Z":
            pids.append(int(name))
    return pids


def stop_session(process, grace=5):
    """Stops every process of the lane that `process` leads, also those its children
    left behind, by their exact PIDs, then reaps `process`. Until then `process` is at
    least a zombie, so no other process can have its PID, which is the session ID."""
    if process.returncode is not None:
        return
    for sig in (signal.SIGTERM, signal.SIGKILL):
        pids = session_pids(process.pid)
        if not pids:
            break
        for pid in pids:
            try:
                os.kill(pid, sig)
            except (ProcessLookupError, PermissionError):
                pass
        deadline = time.monotonic() + grace
        while time.monotonic() < deadline and session_pids(process.pid):
            time.sleep(0.2)
    try:
        process.wait(timeout=grace)
    except subprocess.TimeoutExpired:
        pass


class JsonLines:
    """The complete lines that the lanes appended to a file since the last read."""

    def __init__(self, path):
        self.path, self.offset, self.rest = path, 0, b""

    def read(self):
        try:
            with open(self.path, "rb") as file:
                file.seek(self.offset)
                data = file.read()
        except FileNotFoundError:
            return []
        self.offset += len(data)
        *lines, self.rest = (self.rest + data).split(b"\n")
        records = []
        for line in lines:
            try:
                value = json.loads(line)
            except ValueError:
                print(f"setup-gate: {self.path} has a line that is not JSON", file=sys.stderr)
                continue
            if isinstance(value, dict):
                records.append(value)
        return records


def check_prefix(volume):
    """The start of the worker-check container names of the run whose volume is `volume`,
    as `check_operations` in proof/resources.rs makes it: a 32-bit FNV-1a hash."""
    value = 0xCBF29CE484222325
    for byte in volume.encode():
        value = ((value ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return f"horizon-contract-horizon-setup-check-{value & 0xFFFFFFFF:08x}-"


def valid_record(record, prefixes):
    """Whether a started record names only resources of a run of this gate run."""
    run, volume = record.get("run"), record.get("volume")
    return (isinstance(run, str) and isinstance(volume, str)
            and any(run.startswith(prefix) for prefix in prefixes)
            and volume.startswith("horizon-setup-") and volume.endswith("-" + run)
            and record.get("image") == volume + ":candidate" and record.get("network") == volume + "-preview"
            and record.get("checks") == check_prefix(volume))


def leftover_names(record, listings):
    """The names in Docker's `listings` that belong to the run of a started record."""
    volume = record["volume"]
    return {
        "container": [n for n in listings["container"] if n.startswith((volume + "-", record["checks"]))],
        "image": [n for n in listings["image"] if n == record["image"] or n.startswith(volume + "-build")],
        "volume": [n for n in listings["volume"] if n == volume],
        "network": [n for n in listings["network"] if n == record["network"]],
    }


def docker_listings():
    listings = {}
    for kind, args in LISTINGS.items():
        output = docker(args, timeout=60)
        if output is None:
            return None
        listings[kind] = output.split()
    return listings


def remove_resources(record):
    """Removes what Docker still holds of a started record's run, by exact name; what it
    removed and what is left."""
    listings = docker_listings()
    if listings is None:
        return [], [f"Docker did not answer; look for names that end in {record['run']}"]
    found = leftover_names(record, listings)
    for kind, args in REMOVALS:
        if found[kind]:
            # A volume of a large Git LFS repository can take minutes to delete.
            docker([*args, *found[kind]], timeout=REMOVAL_SECONDS)
    after = docker_listings()
    if after is None:
        return [], [f"Docker did not answer; look for names that end in {record['run']}"]
    left = leftover_names(record, after)
    removed = [name for kind in found for name in found[kind] if name not in left[kind]]
    return removed, [name for names in left.values() for name in names]


def first_line(text, limit=120):
    line = next((line.strip() for line in str(text or "").splitlines() if line.strip()), "")
    return line if len(line) <= limit else line[:limit - 3] + "..."


def summary_row(entry, result, note=None):
    """One summary row: entry, PASS or FAIL, the phase times, the preview, the first
    failure line."""
    if result is None:
        return [entry["label"], "FAIL", "-", "-", "-", "-", note or "no result"]
    phases = [phase for phase in result.get("phases") or [] if isinstance(phase, dict)]
    by_title = {phase.get("phase"): phase for phase in phases}

    def seconds(title):
        value = (by_title.get(title) or {}).get("seconds")
        return "-" if value is None else f"{value} s"

    preview = result.get("preview") or {}
    shown = preview.get("outcome", "-")
    if shown == "live" and preview.get("seconds") is not None:
        shown = f"live {preview['seconds']} s"
    failure = result.get("error") or next(
        (f"{phase.get('phase')}: {first_line(phase['failure'])}" for phase in phases if phase.get("failure")), None)
    if not failure and preview.get("outcome") == "failed":
        failure = f"preview: {preview.get('detail')}"
    if not failure and result.get("passed") is not True:
        failure = "the proof did not end within the gate's time limit"
    passed = result.get("passed") is True and not result.get("error")
    times = [seconds(title) for title in PHASES]
    return [entry["label"], "PASS" if passed else "FAIL", *times, shown, first_line(failure)]


def format_table(rows):
    table = [HEADER, *rows]
    widths = [max(len(row[column]) for row in table) for column in range(len(HEADER) - 1)]
    return "\n".join("  ".join([*(cell.ljust(width) for cell, width in zip(row, widths)), row[-1]]).rstrip()
                     for row in table)


class Lane:
    """One gate process, which proves its entries one after another."""

    def __init__(self, number, prefix, entries, out):
        self.number, self.prefix, self.entries = number, prefix, entries
        for index, entry in enumerate(entries):
            entry["run"] = run_name(prefix, index)
        self.list_path = out / "lanes" / f"lane-{number}.json"
        self.log_path = out / "lanes" / f"lane-{number}.log"
        # Every temporary folder of the lane's proofs is made here, so the runner can
        # remove them all and nothing else.
        self.tmp = out / "tmp" / f"lane-{number}"
        self.process = None
        self.begun = None
        self.reason = None
        self.closed = False

    def start(self, binary, out):
        keys = ("repo", "label", "path", "spec", "preview", "run")
        gate_entries = [{key: entry[key] for key in keys} for entry in self.entries]
        self.list_path.write_text(json.dumps(gate_entries, indent=2) + "\n", encoding="utf-8")
        self.tmp.mkdir(parents=True, exist_ok=True)
        env = dict(os.environ, GATE_LIST=str(self.list_path), GATE_OUT=str(out), GATE_PREFIX=self.prefix,
                   TMPDIR=str(self.tmp))
        with open(self.log_path, "wb") as log:
            # A session of its own: Ctrl-C reaches the runner only, and the runner finds
            # every process of the lane by its session.
            self.process = subprocess.Popen(
                [binary, GATE_TEST, "--exact", "--ignored", "--nocapture", "--test-threads", "1"],
                cwd=ROOT / "crates" / "horizon-ui", env=env, stdin=subprocess.DEVNULL, stdout=log,
                stderr=subprocess.STDOUT, start_new_session=True)
        self.begun = time.monotonic()

    def running(self):
        """Whether the lane's process runs. This does not reap it, so its session ID stays
        its own until stop_session sweeps the session."""
        if self.process is None or self.process.returncode is not None:
            return False
        try:
            return os.waitid(os.P_PID, self.process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT) is None
        except ChildProcessError:
            return False

    def overdue(self):
        return time.monotonic() - self.begun > ENTRY_LIMIT_SECONDS * len(self.entries)

    def stop(self, reason):
        if self.process is None or self.running():
            self.reason = self.reason or reason
        if self.process is not None:
            stop_session(self.process)

    def why(self):
        """Why the lane ended before all its entries did."""
        if self.reason:
            return self.reason
        if self.process is None:
            return "the lane did not start"
        return f"the lane exited with code {self.process.returncode}; see {self.log_path}"


class Gate:
    """The lanes of one gate run, what they reported and what the runner removed."""

    def __init__(self, out, entries, lanes):
        self.out, self.entries, self.lanes = out, entries, lanes
        self.prefixes = [lane.prefix for lane in lanes]
        self.by_run = {entry["run"]: (lane, entry) for lane in lanes for entry in lane.entries}
        self.started = JsonLines(out / "started.jsonl")
        self.finished = JsonLines(out / "results.jsonl")
        self.records, self.results, self.notes = {}, {}, {}
        # What entries that ended left behind, what the runner removed for the others and
        # what it could not remove.
        self.leaks, self.removed, self.unremoved = [], [], []
        self.interrupted = None

    def run(self, binary):
        def interrupt(signum, _frame):
            # Only a flag: the loop below stops at a known point, never in the middle of
            # starting a lane or removing a resource.
            if self.interrupted:
                os.write(2, b"setup-gate: removing this run's resources first, please wait\n")
            else:
                self.interrupted = signal.Signals(signum).name

        for signum in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
            signal.signal(signum, interrupt)
        try:
            for lane in self.lanes:
                if not self.interrupted:
                    lane.start(binary, self.out)
            while not self.interrupted and any(lane.running() for lane in self.lanes):
                time.sleep(2)
                self.collect()
                for lane in self.lanes:
                    if lane.running() and lane.overdue():
                        lane.stop("the lane timed out")
                    if lane.process is not None and not lane.running() and not lane.closed:
                        self.close(lane)
        finally:
            if self.interrupted:
                print(f"setup-gate: {self.interrupted}: stopping the lanes and removing this run's resources",
                      file=sys.stderr, flush=True)
            for lane in self.lanes:
                lane.stop(f"stopped by {self.interrupted}" if self.interrupted else "the runner stopped")
            for lane in self.lanes:
                if not lane.closed:
                    self.close(lane)
            tmp = self.out / "tmp"
            if tmp.is_dir() and not any(tmp.iterdir()):
                tmp.rmdir()

    def collect(self):
        for record in self.started.read():
            if record.get("run") in self.by_run:
                self.records[record["run"]] = record
        for result in self.finished.read():
            run = result.get("run")
            if run not in self.by_run or run in self.results:
                continue
            self.results[run] = result
            lane, entry = self.by_run[run]
            # The gate removed the entry's resources before it wrote the result; what is
            # still there is a leak of Horizon's cleanup.
            removed, left = self.settle(run)
            if removed or left:
                self.leaks.append(f"{entry['label']}: {', '.join(removed + left)}")
            self.unremoved += left
            label, status, build, ci, fast, preview, failure = summary_row(entry, result)
            minutes = (result.get("seconds") or 0) // 60
            line = f"{status} {label} (lane {lane.number}, {minutes} min): build {build}, CI {ci}, fast {fast}"
            if preview != "-":
                line += f", preview {preview}"
            print(line + (f": {failure}" if failure else ""), flush=True)

    def settle(self, run):
        """Gives what a finished entry still holds in Docker time to go, as Docker can still
        delete a large volume after Horizon's cleanup stopped waiting; then removes the rest."""
        record = self.records.get(run)
        if record is not None and valid_record(record, self.prefixes):
            deadline = time.monotonic() + LEAK_GRACE_SECONDS
            while True:
                listings = docker_listings()
                if listings is not None and not any(leftover_names(record, listings).values()):
                    return [], []
                if self.interrupted or time.monotonic() >= deadline:
                    break
                time.sleep(5)
        return self.remove(run)

    def remove(self, run):
        record = self.records.get(run)
        if record is None:
            return [], []
        if not valid_record(record, self.prefixes):
            return [], [f"nothing of {run}: its started record names resources outside this run"]
        return remove_resources(record)

    def close(self, lane):
        """Removes what a lane that ended left: the Docker resources of its entry that did
        not end, and its temporary folders."""
        if lane.process is not None:
            stop_session(lane.process)
        self.collect()
        ended = all(entry["run"] in self.results for entry in lane.entries)
        for entry in lane.entries:
            run = entry["run"]
            if run in self.results:
                continue
            if run not in self.records:
                self.notes[run] = f"not started: {lane.why()}"
                continue
            self.notes[run] = f"no result: {lane.why()}"
            removed, left = self.remove(run)
            self.removed += removed
            self.unremoved += left
        if lane.tmp.is_dir():
            folders = sorted(str(path) for path in lane.tmp.iterdir())
            if folders and ended:
                self.leaks.append(f"lane {lane.number}: {', '.join(folders)}")
            else:
                self.removed += folders
            shutil.rmtree(lane.tmp, ignore_errors=True)
            if lane.tmp.exists():
                self.unremoved.append(str(lane.tmp))
        lane.closed = True

    def summary(self):
        """The summary text, and whether every entry passed and left nothing behind."""
        rows = [summary_row(entry, self.results.get(entry["run"]), self.notes.get(entry["run"]))
                for entry in self.entries]
        passed = sum(row[1] == "PASS" for row in rows)
        lines = [f"Setup gate {self.out}", "", format_table(rows), ""]
        lines += [f"Left out of the spec: {entry['label']}: {', '.join(entry['leave_out'])}"
                  for entry in self.entries if entry["leave_out"]]
        lines += [f"Left behind by an entry that ended, then removed: {leak}" for leak in self.leaks]
        lines += [f"Removed for an entry that did not end: {name}" for name in self.removed]
        lines += [f"NOT REMOVED, remove it by hand: {name}" for name in self.unremoved]
        lines.append(f"{passed} passed, {len(rows) - passed} failed"
                     + (f", stopped by {self.interrupted}" if self.interrupted else ""))
        return "\n".join(lines), passed == len(rows) and not self.leaks and not self.unremoved


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("list", help="the gate list, a JSON file")
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT,
                        help=f"the folder for the runs (default: {DEFAULT_OUT})")
    parser.add_argument("--lanes", type=int, default=DEFAULT_LANES,
                        help=f"the gate processes in parallel (default: {DEFAULT_LANES})")
    parser.add_argument("--min-free-gb", type=int, default=DEFAULT_MIN_FREE_GB,
                        help=f"refuse to start with less free disk (default: {DEFAULT_MIN_FREE_GB})")
    parser.add_argument("--only", action="append", default=[], metavar="PATTERN",
                        help="run only the entries whose label or repo matches this shell pattern; repeat it "
                        "for more")
    parser.add_argument("--binary", help="a horizon test binary that is already built, in place of a build")
    args = parser.parse_args(argv)
    if args.lanes < 1:
        parser.error("--lanes must be 1 or more")
    if args.min_free_gb < 0:
        parser.error("--min-free-gb must be 0 or more")
    try:
        entries = load_list(args.list, args.only)
    except (OSError, ValueError, ListError) as error:
        print(f"setup-gate: {error}", file=sys.stderr)
        return 2
    if args.binary and not (Path(args.binary).is_file() and os.access(args.binary, os.X_OK)):
        print(f"setup-gate: {args.binary} is not an executable file", file=sys.stderr)
        return 2
    out = args.out.resolve()
    problem = pin_docker() or check_preconditions(out, args.min_free_gb)
    if problem:
        print(f"setup-gate: {problem}", file=sys.stderr)
        return 2
    load = os.getloadavg()[0]
    if load > HIGH_LOAD:
        print(f"setup-gate: the load average is {load:.0f}; above {HIGH_LOAD}, tests with time limits fail "
              "for no real reason; use fewer lanes or wait", file=sys.stderr)
    try:
        binary = str(Path(args.binary).resolve()) if args.binary else build_test_binary()
    except KeyboardInterrupt:
        return 128 + signal.SIGINT.value
    if not binary:
        print("setup-gate: the build of the horizon test binary failed", file=sys.stderr)
        return 2
    # Once more: the build can use enough of the same disk to go below the limit.
    problem = check_preconditions(out, args.min_free_gb)
    if problem:
        print(f"setup-gate: after the build, {problem}", file=sys.stderr)
        return 2
    token = secrets.token_hex(3)
    out = out / f"{time.strftime('%Y%m%d-%H%M%S')}-{token}"
    (out / "lanes").mkdir(parents=True)
    lanes = [Lane(number, lane_prefix(token, number), lane, out)
             for number, lane in enumerate(split_lanes(entries, args.lanes), 1)]
    print(f"setup-gate: run {out}", flush=True)
    for lane in lanes:
        names = ", ".join(entry["label"] + (" (heavy)" if entry["heavy"] else "") for entry in lane.entries)
        print(f"  lane {lane.number}, runs {lane.prefix}NNNN, log {lane.log_path}: {names}", flush=True)
    gate = Gate(out, entries, lanes)
    gate.run(binary)
    text, passed = gate.summary()
    (out / "summary.txt").write_text(text + "\n", encoding="utf-8")
    print("\n" + text, flush=True)
    if gate.interrupted:
        return 128 + signal.Signals[gate.interrupted].value
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
