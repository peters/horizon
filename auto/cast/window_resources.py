"""Fixed-window, application-subtree resource observations; never a benchmark score."""
import hashlib
import os
from pathlib import Path
import re
import subprocess
import threading
import time
import xml.etree.ElementTree as ET


def record(pid, proc=Path('/proc')):
    fields = (proc / str(pid) / 'stat').read_text().rsplit(')', 1)[1].split()
    return {'pid': pid, 'start': int(fields[19]), 'state': fields[0],
            'self_ticks': int(fields[11]) + int(fields[12]),
            'reaped_ticks': int(fields[13]) + int(fields[14]),
            'rss_kib': max(0, int(fields[21])) * os.sysconf('SC_PAGE_SIZE') // 1024}


def tree(pid, proc=Path('/proc')):
    pending, records = [pid], {}
    while pending:
        current = pending.pop()
        if current in records:
            continue
        records[current] = record(current, proc)
        for task in (proc / str(current) / 'task').iterdir():
            pending.extend(int(child) for child in (task / 'children').read_text().split())
    return records


def stable_tree(pid, expected_start, proc=Path('/proc')):
    # Reaping transfers CPU counters to a parent. A topology or transfer change
    # between the two passes requires retrying instead of double-counting a child.
    for _ in range(5):
        try:
            before, after = tree(pid, proc), tree(pid, proc)
            if after[pid]['start'] != expected_start or after[pid]['state'] == 'Z':
                raise RuntimeError('application process identity changed or exited')
            if before.keys() == after.keys() and all(
                (before[p]['start'], before[p]['reaped_ticks']) ==
                (after[p]['start'], after[p]['reaped_ticks']) for p in before
            ):
                return after
        except (FileNotFoundError, ProcessLookupError):
            pass
        time.sleep(0.005)
    raise RuntimeError('application subtree did not yield a stable resource snapshot')


def identity(pid):
    info = record(pid)
    if info['state'] == 'Z':
        raise RuntimeError('application already exited')
    executable = Path(f'/proc/{pid}/exe')
    return {'pid': pid, 'start_ticks': info['start'],
            'executable_sha256': hashlib.sha256(executable.read_bytes()).hexdigest()}


def gpu_allocation(xml, pids):
    """Include graphics and compute allocations; an absent observation is unknown."""
    total, observed = 0, False
    for item in ET.fromstring(xml).findall('./gpu/processes/process_info'):
        pid = item.findtext('pid', '')
        if not pid.isdigit() or int(pid) not in pids:
            continue
        if item.findtext('type') not in {'C', 'G', 'C+G', 'M', 'C+G+M'}:
            raise RuntimeError('unknown process GPU allocation type')
        memory = re.fullmatch(r'(\d+) MiB', item.findtext('used_memory', ''))
        if not memory:
            raise RuntimeError('process GPU allocation unavailable')
        total += int(memory[1]); observed = True
    return total if observed else None


def total_ticks(records):
    return sum(item['self_ticks'] + item['reaped_ticks'] for item in records.values())


class Sampler:
    def __init__(self, pid, gpu=True):
        self.pid, self.gpu = pid, gpu
        self.identity = identity(pid)
        self.readings = []
        self.errors = []
        self.gpu_errors = []
        self.done = threading.Event()
        self.thread = None
        self.begin = None
        self.end = None

    def snapshot(self):
        return stable_tree(self.pid, self.identity['start_ticks'])

    def start(self):
        if self.thread is not None:
            raise RuntimeError('resource sampler already started')
        self.begin = self.snapshot()
        self.started = time.monotonic()
        self.thread = threading.Thread(target=self.sample, daemon=True)
        self.thread.start()
        return self

    def sample(self):
        next_gpu = 0
        while not self.done.is_set():
            try:
                records = self.snapshot()
                reading = {'elapsed_seconds': time.monotonic() - self.started,
                           'tree_rss_kib': sum(item['rss_kib'] for item in records.values()),
                           'pids': sorted(records), 'gpu_memory_mib': None}
                if self.gpu and time.monotonic() >= next_gpu:
                    next_gpu = time.monotonic() + 0.5
                    try:
                        result = subprocess.run(['nvidia-smi', '-q', '-x'], capture_output=True,
                                                text=True, timeout=5)
                        if result.returncode:
                            raise RuntimeError('per-process GPU query failed')
                        # Recheck identities after the query so a recycled PID
                        # cannot attribute an unrelated allocation to the fixture.
                        after = self.snapshot()
                        pids = {p for p in records if p in after and records[p]['start'] == after[p]['start']}
                        reading['gpu_memory_mib'] = gpu_allocation(result.stdout, pids)
                        reading['gpu_observed_seconds'] = time.monotonic() - self.started
                    except (RuntimeError, ET.ParseError, OSError, subprocess.TimeoutExpired) as error:
                        self.gpu_errors.append(str(error))
                self.readings.append(reading)
            except Exception as error:
                self.errors.append(str(error))
            self.done.wait(0.05)

    def stop(self):
        if self.thread is None:
            raise RuntimeError('resource sampler was not started')
        if self.end is None:
            self.done.set()
            self.end = self.snapshot()
            self.ended = time.monotonic()

    def finish(self):
        self.stop()
        self.thread.join(timeout=6)
        if self.thread.is_alive():
            raise RuntimeError('resource sampler did not terminate')
        end = self.end
        elapsed = self.ended - self.started
        ticks = total_ticks(end) - total_ticks(self.begin)
        if ticks < 0 or not self.readings or self.errors:
            raise RuntimeError(f'resource measurement incomplete: {self.errors}')
        allocations = [item['gpu_memory_mib'] for item in self.readings if item['gpu_memory_mib'] is not None
                       and item.get('gpu_observed_seconds', float('inf')) <= elapsed]
        return {'contract': 'whole-window-resources-v1', 'score': None,
                'scope': 'verified Horizon application and descendants only',
                'identity': self.identity, 'seconds': elapsed,
                'cpu_ms': ticks * 1000 / os.sysconf('SC_CLK_TCK'),
                'peak_tree_rss_kib': max(item['tree_rss_kib'] for item in self.readings),
                'gpu_memory_mib': max(allocations) if allocations and not self.gpu_errors else None,
                'gpu_errors': self.gpu_errors,
                'gpu_unavailable_reason': ('sampling disabled' if not self.gpu else
                    '; '.join(self.gpu_errors) if self.gpu_errors else
                    'no attributable graphics/compute allocation observed' if not allocations else None),
                'readings': self.readings,
                'limitations': 'Sampled aggregate RSS/GPU allocation; endpoint CPU counter observations; not encoder-v1, displayed FPS or a physical latency score.'}
