"""The Upkeep scheduler on a cloud worker, as the agent account.

One run at a time, on the schedule in config.json, sooner when run-now appears, never while
paused exists. A run lists the open bot pull requests of every repository without cloning,
then starts one agent per repository that has some: the repositories in `order` one at a
time first, then the others side by side. What each repository cost before (minutes and peak
memory) decides how many run together: a heavy one runs alone, and the number side by side
never passes what the worker's cores and memory allow. A repository whose pull requests did
not move since its last clean run is skipped, except in a run that Run now started. No agent
starts where the worker keeps agent approvals (ask): an Upkeep agent has nobody to ask.
"""
import glob
import hashlib
import itertools
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time

DIR = '/workspace/home/upkeep'
APPROVALS = '/run/horizon-worker/agent-approvals'
HOST_INSTANCE = '/run/horizon-worker/browser-host-instance'
# What the merged status may hold, so it stays far below what the exchange reads (1 MiB).
PULL_REQUESTS = 300
ENTRY_LIMIT = 2048
ACTIVITY = 80
LOG_LIMIT = 8 * 1024 * 1024
# A job that runs longer than this is stopped, so one stuck repository cannot hold the hour.
JOB_LIMIT = 90 * 60
REFRESH = 5
# The names Horizon accepts. They reach command lines, file names and prompts, so the loop
# checks them again: config.json is a file in the agent's own folder.
REPOSITORY = re.compile(r'(?!-)[A-Za-z0-9-]{1,39}(?<!-)/(?!\.\.?$)[A-Za-z0-9._-]{1,100}')
AUTHOR = re.compile(r'app/(?!-)[A-Za-z0-9-]{1,39}(?<!-)|(?!-)[A-Za-z0-9-]{1,39}(?<!-)(\[bot\])?')


def refuse_constant(name):
    raise ValueError(f'{name} is not JSON that Horizon reads')


def load(path, default):
    try:
        with open(path) as handle:
            return json.load(handle, parse_constant=refuse_constant)
    except (OSError, ValueError, RecursionError):
        return default


def save(path, value):
    temporary = path + '.tmp'
    with open(temporary, 'w') as handle:
        json.dump(value, handle, indent=1)
    os.replace(temporary, path)


def config():
    cfg = load(DIR + '/config.json', {})
    return cfg if isinstance(cfg, dict) else {}


def names(cfg, key, pattern, default=()):
    """The valid names in the list `key`, once each; the others are reported and left out."""
    values = cfg.get(key, list(default))
    found = []
    for value in values if isinstance(values, list) else []:
        if not isinstance(value, str) or not pattern.fullmatch(value):
            print(f'Left out an invalid name in the {key} of config.json: {value!r:.80}', flush=True)
        elif value not in found:
            found.append(value)
    return found


def codex_enabled(environment):
    """Whether the cloud profile of this worker enables Codex, as horizon-worker-session asks
    before it starts a panel."""
    try:
        result = subprocess.run(['horizon-worker-check', '--agent', 'codex'], stdin=subprocess.DEVNULL,
                                capture_output=True, timeout=60, env=environment)
        return result.returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return False


def approvals():
    """'auto' only where the worker lets agents run without asking, decided as
    horizon-worker-run decides it: never as root, and only when the worker's policy says so."""
    if os.geteuid() == 0:
        return 'ask'
    try:
        with open(APPROVALS) as policy:
            return 'auto' if policy.read() == 'auto\n' else 'ask'
    except OSError:
        return 'ask'


def session_environment():
    """The environment that the image layers give every session, as horizon-worker-run applies
    it, or None when the worker refuses it."""
    try:
        result = subprocess.run(['horizon-worker-session-env', 'apply'], capture_output=True, timeout=60)
        refusal = result.stderr.decode(errors='replace').strip()[:300] if result.returncode else None
    except (OSError, subprocess.TimeoutExpired) as error:
        refusal = str(error)
    if refusal is not None:
        print(f'The worker refused its session environment, so Upkeep starts no agent: {refusal}', flush=True)
        return None
    environment = dict(os.environ)
    for assignment in result.stdout.split(b'\0'):
        key, separator, value = assignment.decode(errors='replace').partition('=')
        if separator:
            environment[key] = value
    # The control service publishes it when it answers, and again after a restart.
    environment.pop('HORIZON_BROWSER_HOST_INSTANCE', None)
    try:
        with open(HOST_INSTANCE) as handle:
            instance = handle.read().strip()
        if instance:
            environment['HORIZON_BROWSER_HOST_INSTANCE'] = instance
    except OSError:
        pass
    return environment


def codex_command(environment):
    """`codex` with the options that horizon-worker-run gives it. Codex 0.156 and later start a
    shared background server that refuses a folder that others can write, which a network
    volume can report; the loop keeps the agent running, so it starts without that server."""
    try:
        usage = subprocess.run(['codex', '--help'], stdin=subprocess.DEVNULL, capture_output=True, text=True,
                               timeout=60, env=environment)
        daemon = '--no-daemon' in usage.stdout
    except (OSError, subprocess.TimeoutExpired):
        daemon = False
    return ['codex', *(['--no-daemon'] if daemon else []), 'exec', '--skip-git-repo-check',
            '--dangerously-bypass-approvals-and-sandbox', '-']


def every(cfg):
    try:
        return max(15, int(cfg.get('every_minutes', 60)))
    except (TypeError, ValueError, OverflowError):
        return 60


def next_slot(cfg):
    minutes = every(cfg)
    return (int(time.time()) // (minutes * 60) + 1) * minutes * 60


def record(entry):
    path = DIR + '/runs.jsonl'
    try:
        with open(path) as handle:
            lines = handle.readlines()[-199:]
    except OSError:
        lines = []
    lines.append(json.dumps(entry) + '\n')
    with open(path + '.tmp', 'w') as handle:
        handle.writelines(lines)
    os.replace(path + '.tmp', path)


def memory_mb():
    try:
        with open('/proc/meminfo') as handle:
            for line in handle:
                if line.startswith('MemTotal:'):
                    return int(line.split()[1]) // 1024
    except OSError:
        pass
    return 4096


def slots(costs):
    """How many agents run side by side: all cores but one, within memory, by the largest
    peak an agent has needed so far (about 1 GB until one has been measured)."""
    peak = max([cost.get('peak_mb', 0) for cost in costs.values()] + [1024])
    return max(1, min((os.cpu_count() or 2) - 1, int(memory_mb() * 0.8 // peak)))


def open_pull_requests(repository, authors):
    """The open pull requests from `authors` with what decides whether they moved: head,
    checks, review and mergeability. None when the repository cannot be read yet."""
    found = []
    for author in authors:
        try:
            result = subprocess.run(
                ['gh', 'pr', 'list', '--repo', repository, '--author', author, '--state', 'open',
                 '--limit', '200', '--json', 'number,headRefOid,statusCheckRollup,reviewDecision,mergeable'],
                capture_output=True, text=True, timeout=60)
            page = json.loads(result.stdout or '[]') if result.returncode == 0 else None
        except (OSError, ValueError, subprocess.TimeoutExpired):
            page = None
        if not isinstance(page, list):
            return None
        found += [pr for pr in page if isinstance(pr, dict)]
    return found


def slug(repository):
    """The file name of a repository. An owner has no `_`, so the first `__` is always the
    slash and two repositories never share a name."""
    return repository.replace('/', '__', 1)


def fingerprint(pull_requests, template):
    """What a repository's agent would see: when it is the same as last time, nothing moved."""
    state = []
    for pr in sorted(pull_requests, key=lambda pr: pr.get('number', 0)):
        # Each check by its name, so two checks that swap their results still count as a change.
        checks = sorted(json.dumps([str(check.get('workflowName')), str(check.get('name') or check.get('context')),
                                    str(check.get('conclusion') or check.get('state') or check.get('status'))])
                        for check in pr.get('statusCheckRollup') or [] if isinstance(check, dict))
        state.append([pr.get('number'), pr.get('headRefOid'), checks, pr.get('reviewDecision'), pr.get('mergeable')])
    return hashlib.sha256(json.dumps([state, template]).encode()).hexdigest()


def tree(pid):
    """`pid` and its descendants."""
    children = {}
    for status in glob.glob('/proc/[0-9]*/stat'):
        try:
            with open(status) as handle:
                fields = handle.read().rsplit(')', 1)[1].split()
            children.setdefault(int(fields[1]), []).append(int(status.split('/')[2]))
        except (OSError, IndexError, ValueError):
            continue
    found, stack = [], [pid]
    while stack:
        current = stack.pop()
        found.append(current)
        stack.extend(children.get(current, []))
    return found


def tree_rss_mb(pid):
    """Resident memory of `pid` and its descendants."""
    total = 0
    for current in tree(pid):
        try:
            with open(f'/proc/{current}/status') as handle:
                for line in handle:
                    if line.startswith('VmRSS:'):
                        total += int(line.split()[1])
        except OSError:
            continue
    return total // 1024


class Job:
    def __init__(self, repository):
        self.repository = repository
        self.slug = slug(repository)
        self.started = None
        self.finished = None
        self.peak_mb = 0
        self.code = None
        self.launched = False
        self.mark = 0
        self.log = DIR + '/logs/' + self.slug + '.log'

    def last_line(self):
        try:
            with open(self.log, 'rb') as handle:
                handle.seek(0, os.SEEK_END)
                handle.seek(max(0, handle.tell() - 4096))
                text = handle.read().decode('utf-8', 'replace')
            # The log holds text from pull requests: no control sequence reaches the terminal.
            lines = [''.join(c for c in line if c.isprintable()).strip() for line in text.splitlines()]
            lines = [line for line in lines if line]
            return lines[-1][:90] if lines else ''
        except OSError:
            return ''

    def run(self, template):
        self.started = time.time()
        try:
            self.attempt(template)
            # The model service sometimes has no capacity for a moment; one later try often works.
            if self.launched and self.code != 0 and 'at capacity' in self.tail(4096):
                time.sleep(60)
                self.attempt(template)
        except Exception as error:  # noqa: BLE001 - the run records the job as failed
            self.code = None
            with open(self.log, 'a') as log:
                log.write(f'\nThe agent could not run: {error}\n')
        finally:
            self.finished = time.time()

    def tail(self, size):
        """The end of what the last attempt wrote, never text of an earlier run."""
        try:
            with open(self.log, 'rb') as handle:
                handle.seek(0, os.SEEK_END)
                handle.seek(max(self.mark, handle.tell() - size))
                return handle.read().decode('utf-8', 'replace')
        except OSError:
            return ''

    def attempt(self, template):
        # A run can last hours: each agent starts under the policy and with the session
        # environment of its own moment, as a panel would.
        environment = session_environment() if approvals() == 'auto' else None
        if environment is None or not codex_enabled(environment):
            self.code = None
            with open(self.log, 'a') as log:
                log.write('\nThe worker asks before an agent runs a command, refused its session '
                          'environment, or does not enable Codex; this agent did not start.\n')
            return
        prompt = (template.replace('@@REPOSITORY@@', self.repository)
                  .replace('@@STATUS@@', DIR + '/status.d/' + self.slug + '.json'))
        # The prompt goes on standard input: as an argument, text that starts with a dash
        # would be read as an option, and a long one would pass the argument size limit.
        trim(self.log)
        with open(self.log, 'a') as log, tempfile.TemporaryFile() as instructions:
            self.mark = log.tell()
            instructions.write(prompt.encode())
            instructions.seek(0)
            log.write(f'\n=== {time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())} {self.repository} ===\n')
            log.flush()
            process = subprocess.Popen(
                codex_command(environment), cwd=DIR + '/repos', stdin=instructions, stdout=log, stderr=subprocess.STDOUT,
                env=environment)
            self.launched = True
            stopped = False
            while process.poll() is None:
                self.peak_mb = max(self.peak_mb, tree_rss_mb(process.pid))
                if not stopped and time.time() - self.started > JOB_LIMIT:
                    # With every process it started, so no tool of the agent keeps running.
                    for pid in tree(process.pid):
                        try:
                            os.kill(pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                    log.write('\nStopped: the run passed its time limit.\n')
                    stopped = True
                time.sleep(REFRESH)
            self.code = process.returncode


def heavy(costs, repository, memory):
    cost = costs.get(repository) or {}
    return cost.get('minutes', 0) >= 20 or cost.get('peak_mb', 0) >= memory * 0.35


def nesting(value):
    """How deep lists and objects nest in `value`."""
    children = value.values() if isinstance(value, dict) else value if isinstance(value, list) else None
    return 0 if children is None else 1 + max(map(nesting, children), default=0)


def entry_size(value):
    """The size of an entry as Horizon gets it, or None when Horizon could not read it (Horizon
    reads at most 128 levels, of which the status takes three)."""
    try:
        if nesting(value) > 32:
            return None
        return len(json.dumps(value, ensure_ascii=False, allow_nan=False).encode())
    except (ValueError, UnicodeError, RecursionError):
        return None


def entries(report, key):
    """The entries of one list in an agent's report that are objects of a reasonable size."""
    values = report.get(key) if isinstance(report, dict) else None
    if not isinstance(values, list):
        return []
    return [value for value in values if isinstance(value, dict) and (entry_size(value) or ENTRY_LIMIT + 1) <= ENTRY_LIMIT]


def merge_status(repositories):
    """One status document from the jobs' own reports, which the agents write as they like."""
    reports, activity = [], []
    for repository in repositories:
        report = load(DIR + '/status.d/' + slug(repository) + '.json', {})
        reports.append(entries(report, 'pull_requests'))
        activity += entries(report, 'activity')
    activity.sort(key=lambda entry: str(entry.get('at', '')))
    # Open pull requests first, taken from every repository in turn, so a cut leaves out merged
    # ones before open ones and never all of a late repository.
    pull_requests = []
    for merged_now in (False, True):
        lists = [[pr for pr in report if (pr.get('state') == 'merged') == merged_now] for report in reports]
        pull_requests += [pr for row in itertools.zip_longest(*lists) for pr in row if pr is not None]
    status = {'pull_requests': pull_requests[:PULL_REQUESTS], 'activity': activity[-ACTIVITY:]}
    with open(DIR + '/status.json.tmp', 'w') as handle:
        json.dump(status, handle, ensure_ascii=False, separators=(',', ':'))
    os.replace(DIR + '/status.json.tmp', DIR + '/status.json')


def forget_open(repository):
    """Keeps only the merged pull requests in a repository's report once it has no open ones,
    so the board does not show pull requests that closed without an agent."""
    path = DIR + '/status.d/' + slug(repository) + '.json'
    report = load(path, None)
    if isinstance(report, dict) and isinstance(report.get('pull_requests'), list):
        report['pull_requests'] = [pr for pr in report['pull_requests']
                                   if isinstance(pr, dict) and pr.get('state') == 'merged']
        save(path, report)


def trim(path):
    """Keeps the last part of a log that grew past its limit, so the volume never fills."""
    try:
        if os.path.getsize(path) <= LOG_LIMIT:
            return
        with open(path, 'rb') as handle:
            handle.seek(-LOG_LIMIT // 8, os.SEEK_END)
            tail = handle.read()
        with open(path + '.tmp', 'wb') as handle:
            handle.write(tail)
        os.replace(path + '.tmp', path)
    except OSError:
        pass


def write_next(upcoming):
    """Replaces next-run in one step, so the exchange never reads it half written."""
    with open(DIR + '/next-run.tmp', 'w') as handle:
        handle.write(str(upcoming))
    os.replace(DIR + '/next-run.tmp', DIR + '/next-run')


def merged():
    """The pull requests that the status shows as merged."""
    status = load(DIR + '/status.json', {})
    pull_requests = status.get('pull_requests') if isinstance(status, dict) else None
    return {json.dumps([pr.get('repository'), pr.get('number')]) for pr in pull_requests or []
            if isinstance(pr, dict) and pr.get('state') == 'merged'} if isinstance(pull_requests, list) else set()


def close_interrupted():
    """A run the loop did not see to its end (the loop was stopped) is recorded as failed, so
    the board never shows it running forever."""
    try:
        with open(DIR + '/runs.jsonl') as handle:
            last = json.loads(handle.readlines()[-1])
        if last.get('outcome') == 'running':
            record({'started_at': last['started_at'], 'finished_at': int(time.time()), 'outcome': 'failed'})
    except (OSError, IndexError, ValueError, AttributeError, KeyError):
        pass


def draw(started, jobs, queued, capacity):
    width = max(40, shutil.get_terminal_size((120, 40)).columns)
    lines = [f'Upkeep run since {time.strftime("%H:%M", time.gmtime(started))} UTC, {capacity} side by side at most', '']
    now = time.time()
    for job in jobs:
        if job.started is None:
            continue
        minutes = int(((job.finished or now) - job.started) // 60)
        mark = '>' if job.finished is None else ('done' if job.code == 0 else 'failed')
        name = job.repository.split('/', 1)[-1]
        lines.append(f'{mark:6} {name:22.22} {minutes:3}m {job.peak_mb / 1024:4.1f}G  {job.last_line()}')
    for repository in queued:
        lines.append(f'{"queued":6} {repository.split("/", 1)[-1]}')
    # Cut, never wrap: a wrapped line pushes the table off the panel.
    sys.stdout.write('\x1b[H\x1b[2J' + '\n'.join(line[:width] for line in lines) + '\n')
    sys.stdout.flush()


def run_once(cfg, forced):
    """One run. `forced` (Run now) also starts agents for repositories that did not move."""
    if approvals() != 'auto':
        print('This worker asks before an agent runs a command (agent approvals: ask), and an Upkeep '
              'agent has nobody to ask. Upkeep starts no agent. Use a worker with auto approvals.', flush=True)
        return False
    if session_environment() is None:
        return False
    with open(DIR + '/prompt.md') as handle:
        template = handle.read()
    repositories = names(cfg, 'repositories', REPOSITORY)
    authors = names(cfg, 'authors', AUTHOR, ['app/dependabot'])
    order = [repository for repository in names(cfg, 'order', REPOSITORY) if repository in repositories]
    print(f'Listing open pull requests in {len(repositories)} repositories...', flush=True)
    seen = load(DIR + '/fingerprints.json', {})
    seen = seen if isinstance(seen, dict) else {}
    todo, prints, unchanged = [], {}, []
    for repository in repositories:
        found = open_pull_requests(repository, authors)
        # A repository it cannot read yet still gets an agent: the agent asks for access.
        if found is None:
            todo.append(repository)
            continue
        if not found:
            # A pull request that opens again later is new to the agent, even when it is the same.
            seen.pop(repository, None)
            forget_open(repository)
            continue
        prints[repository] = fingerprint(found, template)
        # Nothing moved since the last run: the agent would only reach the same answer.
        if not forced and seen.get(repository) == prints[repository]:
            unchanged.append(repository)
            continue
        todo.append(repository)
    if unchanged:
        print(f'Unchanged since the last run, skipped: {", ".join(unchanged)}', flush=True)
    first = [repository for repository in order if repository in todo]
    rest = [repository for repository in todo if repository not in first]
    costs = load(DIR + '/costs.json', {})
    costs = {name: cost for name, cost in costs.items() if isinstance(cost, dict)} if isinstance(costs, dict) else {}
    memory = memory_mb()
    capacity = slots(costs)
    # Heavy repositories last, so the light ones finish while one heavy one runs alone.
    rest.sort(key=lambda repository: heavy(costs, repository, memory))
    jobs = [Job(repository) for repository in first + rest]
    queue = list(jobs)
    running = []
    started = time.time()

    def start(job):
        thread = threading.Thread(target=job.run, args=(template,), daemon=True)
        thread.start()
        running.append((job, thread))

    try:
        while queue or running:
            for job, thread in [entry for entry in running if not entry[1].is_alive()]:
                running.remove((job, thread))
                # Remembered as seen only after a clean finish, as the agent found it: whatever
                # moved while it worked, its own pushes included, is looked at again next run.
                if job.code == 0 and job.repository in prints:
                    seen[job.repository] = prints[job.repository]
            merge_status(repositories)
            if queue:
                job = queue[0]
                ordered = job.repository in first
                alone = ordered or heavy(costs, job.repository, memory)
                if (alone and not running) or (not alone and len(running) < capacity
                                                and not any(heavy(costs, other.repository, memory)
                                                            or other.repository in first for other, _ in running)):
                    queue.pop(0)
                    start(job)
                    continue
            draw(started, jobs, [job.repository for job in queue], capacity)
            time.sleep(REFRESH)
    finally:
        # When the run fails here, its agents still end before the next run can start.
        for _, thread in running:
            thread.join()
    merge_status(repositories)
    # Only agents that started say what a repository costs.
    for job in (job for job in jobs if job.launched):
        cost = costs.get(job.repository, {})
        minutes = (job.finished - job.started) / 60
        runs = cost.get('runs', 0)
        # A moving average, so one unusual run does not reclassify a repository.
        cost['minutes'] = round((cost.get('minutes', minutes) * runs + minutes) / (runs + 1), 1)
        cost['peak_mb'] = max(job.peak_mb, int(cost.get('peak_mb', 0) * 0.8))
        cost['runs'] = min(runs + 1, 10)
        costs[job.repository] = cost
    save(DIR + '/costs.json', costs)
    save(DIR + '/fingerprints.json', seen)
    draw(started, jobs, [], capacity)
    return all(job.code == 0 for job in jobs)


def main():
    for folder in ('repos', 'logs', 'status.d'):
        os.makedirs(os.path.join(DIR, folder), exist_ok=True)
    close_interrupted()
    cfg = config()
    minutes = every(cfg)
    upcoming = next_slot(cfg)
    # A run that was due when a new loop replaced this one still runs.
    try:
        with open(DIR + '/next-run') as handle:
            earlier = int(handle.read().strip())
        if upcoming - minutes * 60 <= earlier < upcoming:
            upcoming = earlier
    except (OSError, ValueError):
        pass
    write_next(upcoming)
    print(f'Upkeep waits for its next run at {time.strftime("%H:%M", time.gmtime(upcoming))} UTC.', flush=True)
    while True:
        # The exchange asks for a restart after it installed a complete new loop. Here, at the
        # top, no run is going, and a due run or Run now is left for the new loop.
        if os.path.exists(DIR + '/restart'):
            os.remove(DIR + '/restart')
            print('\nAn exchange installed a new loop; starting it.', flush=True)
            os.execv('/bin/bash', ['/bin/bash', '--noprofile', '--norc', DIR + '/run.sh', *sys.argv[1:]])
        cfg = config()
        paused = os.path.exists(DIR + '/paused')
        # A pause skips the runs it covers; the next run is the next one on the schedule.
        if every(cfg) != minutes or (paused and time.time() >= upcoming):
            minutes = every(cfg)
            upcoming = next_slot(cfg)
            write_next(upcoming)
        forced = not paused and os.path.exists(DIR + '/run-now')
        if not paused and (forced or time.time() >= upcoming):
            try:
                os.remove(DIR + '/run-now')
            except OSError:
                pass
            cfg = config()
            begun = int(time.time())
            record({'started_at': begun, 'outcome': 'running'})
            before = merged()
            try:
                ok = run_once(cfg, forced)
            except Exception as error:  # noqa: BLE001 - one failed run must not end the loop
                print(f'The run failed: {error}', flush=True)
                ok = False
            ended = int(time.time())
            record({'started_at': begun, 'finished_at': ended, 'outcome': 'done' if ok else 'failed',
                    'exit': 0 if ok else 1, 'merged': len(merged() - before)})
            # Run now during this run asked for the run that just ended.
            try:
                os.remove(DIR + '/run-now')
            except OSError:
                pass
            # Slots that passed while the run was busy were skipped, never run beside it.
            slot = (begun // (every(cfg) * 60) + 1) * every(cfg) * 60
            while slot < ended:
                record({'started_at': slot, 'outcome': 'skipped'})
                slot += every(cfg) * 60
            minutes = every(cfg)
            upcoming = next_slot(cfg)
            write_next(upcoming)
            print(f'\nUpkeep waits for its next run at {time.strftime("%H:%M", time.gmtime(upcoming))} UTC.', flush=True)
        time.sleep(10)


if __name__ == '__main__':
    main()
