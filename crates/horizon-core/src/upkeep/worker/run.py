"""The Upkeep scheduler on a cloud worker, as the agent account.

One run at a time, on the schedule in config.json, sooner when run-now appears, never while
paused exists. A run lists the open bot pull requests of every repository without cloning,
then starts one agent per repository that has some: the repositories in `order` one at a
time first, then the others side by side. What each repository cost before (minutes and peak
memory) decides how many run together: a heavy one runs alone, and the number side by side
never passes what the worker's cores and memory allow. A repository whose pull requests did
not move since its last clean run is skipped, except in a run that Run now started. No agent
starts where the worker keeps agent approvals (ask): an Upkeep agent has nobody to ask.

Between the planned runs, and during one, the loop lists the pull requests again every minute.
A repository whose pull requests moved since its agent last started (a new pull request, a
finished check, a review, a rebase) gets its agent at once; one whose agent runs or waits is
left until that agent ends. The planned run catches what this missed. A rate limit of GitHub
makes the loop wait longer before it lists again.

The first planned run of each UTC day ends with the daily maintenance pass: one agent for the
whole Upkeep, with the instructions in daily.md, after the agents of the repositories ended,
so no two agents work in one repository at once.
"""
import glob
import hashlib
import itertools
import json
import os
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time

DIR = '/workspace/home/upkeep'
APPROVALS = '/run/horizon-worker/agent-approvals'
# Root writes it when Horizon set the Upkeep policy of the worker's GitHub access.
GUARD = '/run/horizon-worker/github-upkeep-policy'
GUARD_OWNER = 0
HOST_INSTANCE = '/run/horizon-worker/browser-host-instance'
# What the merged status may hold, so it stays far below what the exchange reads (1 MiB).
PULL_REQUESTS = 300
ENTRY_LIMIT = 2048
ACTIVITY = 80
LOG_LIMIT = 8 * 1024 * 1024
# A job that runs longer than this is stopped, so one stuck repository cannot hold the hour.
JOB_LIMIT = 90 * 60
# The daily maintenance pass works across every repository, so it may take longer.
DAILY_LIMIT = 4 * 60 * 60
# The file name of the daily pass's report and log. A repository's name always holds `__`.
DAILY = 'daily'
# The newest lines of the daily pass that the status keeps besides those of the repositories.
DAILY_ACTIVITY = 20
REFRESH = 5
# How often the loop lists the pull requests of every repository for a change, in seconds.
POLL = 60
# After GitHub reports a rate limit, the loop waits this long before it lists again, twice as
# long after each further limit, up to BACKOFF_MAX.
BACKOFF = 120
BACKOFF_MAX = 30 * 60
# What gh prints when GitHub limits the requests: the hourly limit or a secondary limit.
RATE_LIMITED = re.compile(r'rate limit|abuse detection|HTTP 429', re.IGNORECASE)
# What gh prints when the worker's GitHub access refuses a request before GitHub sees it.
REFUSED = re.compile(r'Horizon: ')
# How many open pull requests of one author a listing of one repository takes.
LISTED = 200
# How many repositories one search of a look lists. One search costs GitHub's hourly
# budget the same as one listing of one repository, so a look every minute stays small.
SEARCHED = 20
# The longest search query that the worker's GitHub access accepts.
SEARCH_LENGTH = 1024
# The pages of a search, of 100 pull requests each. A look that finds more leaves the
# repositories to the planned run.
PAGES = 10
SEARCH = '''query($q: String!, $after: String) {
  search(query: $q, type: ISSUE, first: 100, after: $after) {
    pageInfo { hasNextPage endCursor }
    nodes { ... on PullRequest { repository { nameWithOwner } state number headRefOid reviewDecision mergeable
      commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100) { nodes {
        __typename
        ... on CheckRun { name status conclusion checkSuite { workflowRun { workflow { name } } } }
        ... on StatusContext { context state } } } } } } } } }
  }
}'''
# The kind of a run that a change between the planned runs started.
CHANGE = 'change'
# Far more than the instructions Horizon writes (8 KiB of the person's and the parts it adds).
PROMPT_LIMIT = 256 * 1024
# The largest JSON file of the agent's folder that the loop reads, such as one agent's report.
REPORT_LIMIT = 4 * 1024 * 1024
# The names Horizon accepts. They reach command lines, file names and prompts, so the loop
# checks them again: config.json is a file in the agent's own folder.
REPOSITORY = re.compile(r'(?!-)[A-Za-z0-9-]{1,39}(?<!-)/(?!\.\.?$)[A-Za-z0-9._-]{1,100}')
AUTHOR = re.compile(r'app/(?!-)[A-Za-z0-9-]{1,39}(?<!-)|(?!-)[A-Za-z0-9-]{1,39}(?<!-)(\[bot\])?')


def refuse_constant(name):
    raise ValueError(f'{name} is not JSON that Horizon reads')


def load(path, default):
    """The JSON in `path`, or `default`. Every file here is the agent's, so it is read only as
    a regular file of a bounded size, never through a link or a pipe that could block."""
    data = read_regular(path, REPORT_LIMIT)
    try:
        return default if data is None else json.loads(data, parse_constant=refuse_constant)
    except (ValueError, RecursionError):
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


def guarded():
    """Whether root set the Upkeep policy of this worker's GitHub access, which refuses the
    changes that an Upkeep never needs. An older worker image has no such policy."""
    try:
        info = os.stat(GUARD, follow_symlinks=False)
    except OSError:
        return False
    return stat.S_ISREG(info.st_mode) and info.st_uid == GUARD_OWNER


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


def read_small(path):
    """At most 64 bytes of the regular file `path`, or None. The folder is the agent's, so a
    link, a pipe or a device there is never followed or opened in a way that blocks."""
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except OSError:
        return None
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            return None
        return os.read(fd, 64)
    finally:
        os.close(fd)


def read_regular(path, limit):
    """The bytes of the regular file `path`, read as read_small reads, or None when it is not
    a regular file or is larger than `limit`."""
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except OSError:
        return None
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            return None
        data = b''
        while len(data) <= limit:
            chunk = os.read(fd, 65536)
            if not chunk:
                return data
            data += chunk
        return None
    finally:
        os.close(fd)


def read_prompt(name):
    """The instructions in `name`, or None when they are not a regular file or are larger than
    any instructions Horizon writes."""
    data = read_regular(DIR + '/' + name, PROMPT_LIMIT)
    return None if data is None else data.decode('utf-8', 'replace')


def take_run_now():
    """Takes the Run now signal. Returns whether it asks for a run: a signal whose token the
    loop took before asks for none, since Horizon sends a token again when it did not learn
    whether an earlier exchange delivered it. The signal is moved aside first, so a new one
    that an exchange writes meanwhile stays, and its token is recorded before the signal
    goes: a loop that stops in between, or cannot record the token, never runs it twice."""
    taking = DIR + '/run-now.taking'
    try:
        os.replace(DIR + '/run-now', taking)
    except OSError:
        return False
    token = read_small(taking)
    fresh = token is not None and (not token or token != read_small(DIR + '/run-now-token'))
    if token and fresh:
        try:
            fd, temporary = tempfile.mkstemp(prefix='.run-now-token.', dir=DIR)
            with os.fdopen(fd, 'wb') as handle:
                handle.write(token)
            os.replace(temporary, DIR + '/run-now-token')
        except OSError as error:  # a full disk must not end the loop, nor run a token twice
            print(f'Could not record the Run now token, so it starts no run: {error}', flush=True)
            fresh = False
    try:
        os.remove(taking)
    except OSError:
        pass
    return fresh


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


class Limited(Exception):
    """GitHub refused a listing because of a rate limit."""


class Refused(Exception):
    """The worker's GitHub access refused a search, for example for a repository it does not
    grant."""


def open_pull_requests(repository, authors):
    """The open pull requests from `authors` with what decides whether they moved: head,
    checks, review and mergeability. None when the repository cannot be read yet. Raises
    Limited when GitHub refused a listing for a rate limit."""
    found = []
    for author in authors:
        try:
            result = subprocess.run(
                ['gh', 'pr', 'list', '--repo', repository, '--author', author, '--state', 'open',
                 '--limit', str(LISTED), '--json', 'number,headRefOid,statusCheckRollup,reviewDecision,mergeable'],
                capture_output=True, text=True, timeout=60)
            if result.returncode != 0 and RATE_LIMITED.search(result.stderr or ''):
                raise Limited(repository)
            page = json.loads(result.stdout or '[]') if result.returncode == 0 else None
        except (OSError, ValueError, subprocess.TimeoutExpired):
            page = None
        if not isinstance(page, list):
            return None
        found += [pr for pr in page if isinstance(pr, dict)]
    return found


def dig(value, *keys):
    """The value at `keys` in nested objects, or None where one is missing."""
    for key in keys:
        if isinstance(key, int):
            value = value[key] if isinstance(value, list) and len(value) > key else None
        else:
            value = value.get(key) if isinstance(value, dict) else None
    return value


def flat(node):
    """A pull request of a search in the shape that gh pr list gives, so its fingerprint is
    the same as in a run."""
    checks = []
    for check in dig(node, 'commits', 'nodes', 0, 'commit', 'statusCheckRollup', 'contexts', 'nodes') or []:
        if dig(check, '__typename') == 'CheckRun':
            checks.append({'name': check.get('name') or '', 'status': check.get('status') or '',
                           'conclusion': check.get('conclusion') or '',
                           'workflowName': dig(check, 'checkSuite', 'workflowRun', 'workflow', 'name') or ''})
        elif isinstance(check, dict):
            checks.append({'context': check.get('context') or '', 'state': check.get('state') or ''})
    return {'number': node.get('number'), 'headRefOid': node.get('headRefOid') or '',
            'statusCheckRollup': checks, 'reviewDecision': node.get('reviewDecision') or '',
            'mergeable': node.get('mergeable') or ''}


def query(repositories, author):
    return ' '.join(['is:pr', 'is:open', f'author:{author}'] + [f'repo:{repository}' for repository in repositories])


def search(repositories, author):
    """The open pull requests from `author` in `repositories`, by repository, from one
    search. None when the search failed or found too many, and None for a repository with
    more than a run's listing takes. Raises Limited when GitHub refused it for a rate
    limit, and Refused when the worker's GitHub access did."""
    names = {repository.lower(): repository for repository in repositories}
    found = {repository: [] for repository in repositories}
    command = ['gh', 'api', 'graphql', '-f', f'query={SEARCH}', '-f', f'q={query(repositories, author)}']
    after = None
    for _ in range(PAGES):
        try:
            result = subprocess.run(command + (['-f', f'after={after}'] if after else []),
                                    capture_output=True, text=True, timeout=60)
            if result.returncode != 0 and RATE_LIMITED.search(result.stderr or ''):
                raise Limited(repositories[0])
            if result.returncode != 0 and REFUSED.search(result.stderr or ''):
                raise Refused(repositories[0])
            page = dig(json.loads(result.stdout or '{}'), 'data', 'search') if result.returncode == 0 else None
        except (OSError, ValueError, subprocess.TimeoutExpired):
            page = None
        if not isinstance(dig(page, 'nodes'), list):
            return None
        for node in page['nodes']:
            repository = names.get(str(dig(node, 'repository', 'nameWithOwner')).lower())
            # The search can still find a pull request that merged or closed a moment ago.
            if repository is not None and dig(node, 'state') == 'OPEN':
                found[repository].append(flat(node))
        after = dig(page, 'pageInfo', 'endCursor')
        if not dig(page, 'pageInfo', 'hasNextPage'):
            # The planned run sees the same as its own listing; a look would see more.
            return {repository: listed if len(listed) <= LISTED else None for repository, listed in found.items()}
    return None


def groups(repositories, author):
    """`repositories` in groups of at most SEARCHED, each with a query that the worker's
    GitHub access accepts."""
    group = []
    for repository in repositories:
        if group and (len(group) == SEARCHED or len(query(group + [repository], author)) > SEARCH_LENGTH):
            yield group
            group = []
        group.append(repository)
    if group:
        yield group


def searched(repositories, authors, apart):
    """The open pull requests from `authors` in `repositories`, by repository, from a few
    searches for all of them instead of a listing for each; None for a repository whose
    search failed. The worker's GitHub access refuses a whole search that names one
    repository it does not grant, so the repositories of a refused search go into `apart`:
    each is searched on its own until that works, then with the others again. Raises
    Limited when GitHub refused a search for a rate limit."""
    found = {repository: [] for repository in repositories}
    alone = [repository for repository in repositories if repository in apart]
    for author in authors:
        # A repository whose search failed for one author is not searched for the others.
        together = [repository for repository in repositories if repository not in alone and found[repository] is not None]
        lone = [repository for repository in alone if found[repository] is not None]
        for group in [*groups(together, author), *([repository] for repository in lone)]:
            try:
                pulls = search(group, author)
            except Refused:
                apart.update(group if len(group) > 1 else ())
                pulls = None
            # Another failure, such as a timeout, is tried again with the same group.
            if pulls is None:
                found.update(dict.fromkeys(group))
                continue
            for repository, listed in pulls.items():
                if listed is None:
                    found[repository] = None
                elif found[repository] is not None:
                    found[repository] += listed
    apart.difference_update(repository for repository in alone if found[repository] is not None)
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


class Poller:
    """Lists the pull requests of the repositories at most every POLL seconds and finds the
    ones that moved since their agent last started. After a rate limit it waits longer."""

    def __init__(self, started):
        # The fingerprint that each repository's last agent started with. A failed agent's
        # repository is not started again until its pull requests move, or the planned run.
        self.started = {repository: value for repository, value in started.items()
                        if isinstance(repository, str) and isinstance(value, str)} if isinstance(started, dict) else {}
        self.saved = (dict(self.started), set())
        self.after = 0
        self.backoff = 0
        # The repositories that Run now asked for but a rate limit held back: a later look
        # starts them although nothing moved.
        self.forced = set()
        # The repositories that a look searches on their own, see searched().
        self.apart = set()

    def due(self):
        return time.time() >= self.after

    def save(self):
        """Keeps `started` and `forced` for the next loop (see restored), so a restart does not
        start an agent again for a change whose agent failed or did not start, and keeps a
        Run now that waits."""
        if (self.started, self.forced) != self.saved:
            try:
                save(DIR + '/started.json', {'started': self.started, 'forced': sorted(self.forced)})
                self.saved = (dict(self.started), set(self.forced))
            except OSError:
                pass

    def listed(self):
        """A run listed every repository without a limit: the next look is a minute after
        that, and the next limit waits the shortest time again."""
        self.backoff = 0
        self.after = max(self.after, time.time() + POLL)

    def limited(self):
        self.backoff = min(BACKOFF_MAX, self.backoff * 2 or BACKOFF)
        self.after = time.time() + self.backoff
        print(f'GitHub limits the requests; the loop lists the pull requests again in '
              f'{self.backoff // 60} minutes.', flush=True)

    def changed(self, repositories, authors, template, busy=(), forget=None):
        """The repositories that moved, with their fingerprints, never one in `busy`: its
        agent runs or waits, and the change is looked at when it has ended. `forget` is called
        for a repository that has no open pull request any more, as a run does."""
        if not self.due():
            return {}
        moved = {}
        try:
            pulls = searched([repository for repository in repositories if repository not in busy], authors,
                             self.apart)
        except Limited:
            self.limited()
            return moved
        # Before the results are used, so a failure there does not search again at once. A
        # minute after the search ended, so a slow search never makes the looks follow each other.
        # Only a look that read a repository ends the longer waits after a limit.
        if any(found is not None for found in pulls.values()):
            self.backoff = 0
        self.after = time.time() + (self.backoff or POLL)
        for repository, found in pulls.items():
            # A repository that the worker cannot read gets its agent in the planned run, which
            # asks for access; after another failure the next look tries again.
            if found is None:
                continue
            if not found:
                # A pull request that opens again later is new to the agent, even when it is the
                # same. Also without a fingerprint, for example after a failed first agent, the
                # board drops the pull requests that closed.
                self.started.pop(repository, None)
                (forget or forget_seen)(repository)
                continue
            value = fingerprint(found, template)
            if value != self.started.get(repository) or repository in self.forced:
                moved[repository] = value
        return moved


def restored():
    """The Poller of the loop before, or one that starts from the last clean agents."""
    state = load(DIR + '/started.json', None)
    if not isinstance(state, dict) or not isinstance(state.get('started'), dict):
        return Poller(load(DIR + '/fingerprints.json', {}))
    poller = Poller(state['started'])
    forced = state.get('forced')
    poller.forced = {repository for repository in forced if isinstance(repository, str)
                     and REPOSITORY.fullmatch(repository)} if isinstance(forced, list) else set()
    poller.saved = (dict(poller.started), set(poller.forced))
    return poller


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
    def __init__(self, repository, key=None, limit=JOB_LIMIT):
        self.repository = repository
        self.slug = key or slug(repository)
        self.limit = limit
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
        environment = session_environment() if approvals() == 'auto' and guarded() else None
        if environment is None or not codex_enabled(environment):
            self.code = None
            with open(self.log, 'a') as log:
                log.write('\nThe worker asks before an agent runs a command, does not limit its GitHub '
                          'access, refused its session environment, or does not enable Codex; this agent '
                          'did not start.\n')
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
                if not stopped and time.time() - self.started > self.limit:
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
    """One status document from the jobs' own reports, which the agents write as they like.
    Only the lines of the daily pass's own report are marked as daily."""
    reports, activity = [], []
    for repository in repositories:
        report = load(DIR + '/status.d/' + slug(repository) + '.json', {})
        reports.append(entries(report, 'pull_requests'))
        activity += entries(report, 'activity')
    by_time = lambda entry: str(entry.get('at', ''))  # noqa: E731
    activity.sort(key=by_time)
    daily = entries(load(DIR + '/status.d/' + DAILY + '.json', {}), 'activity')
    daily.sort(key=by_time)
    activity = ([{key: value for key, value in entry.items() if key != 'daily'} for entry in activity[-ACTIVITY:]]
                + [{**entry, 'daily': True} for entry in daily[-DAILY_ACTIVITY:]])
    activity.sort(key=by_time)
    # Open pull requests first, taken from every repository in turn, so a cut leaves out merged
    # ones before open ones and never all of a late repository.
    pull_requests = []
    for merged_now in (False, True):
        lists = [[pr for pr in report if (pr.get('state') == 'merged') == merged_now] for report in reports]
        pull_requests += [pr for row in itertools.zip_longest(*lists) for pr in row if pr is not None]
    status = {'pull_requests': pull_requests[:PULL_REQUESTS], 'activity': activity}
    with open(DIR + '/status.json.tmp', 'w') as handle:
        json.dump(status, handle, ensure_ascii=False, separators=(',', ':'))
    os.replace(DIR + '/status.json.tmp', DIR + '/status.json')


def forget_seen(repository):
    """Between runs: a repository without open pull requests starts again from nothing.
    Returns whether its report showed a pull request that is not merged."""
    seen = load(DIR + '/fingerprints.json', {})
    if isinstance(seen, dict) and seen.pop(repository, None) is not None:
        save(DIR + '/fingerprints.json', seen)
    return forget_open(repository)


def forget_open(repository):
    """Keeps only the merged pull requests in a repository's report once it has no open ones,
    so the board does not show pull requests that closed without an agent. Returns whether
    the report showed one that is not merged."""
    path = DIR + '/status.d/' + slug(repository) + '.json'
    report = load(path, None)
    if isinstance(report, dict) and isinstance(report.get('pull_requests'), list):
        kept = [pr for pr in report['pull_requests'] if isinstance(pr, dict) and pr.get('state') == 'merged']
        if kept != report['pull_requests']:
            report['pull_requests'] = kept
            save(path, report)
            return True
    return False


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


def write_next(upcoming, minutes):
    """Replaces next-run in one step, so the exchange never reads it half written. The second
    field is the schedule it was planned for."""
    with open(DIR + '/next-run.tmp', 'w') as handle:
        handle.write(f'{upcoming} {minutes}\n')
    os.replace(DIR + '/next-run.tmp', DIR + '/next-run')


def merged():
    """The pull requests that the status shows as merged."""
    status = load(DIR + '/status.json', {})
    pull_requests = status.get('pull_requests') if isinstance(status, dict) else None
    return {json.dumps([pr.get('repository'), pr.get('number')]) for pr in pull_requests or []
            if isinstance(pr, dict) and pr.get('state') == 'merged'} if isinstance(pull_requests, list) else set()


def last_run():
    """When the last recorded run started and when it finished (0 for either that is not
    known). A slot recorded as skipped did not start, for example one that passed during a
    run of a change."""
    try:
        with open(DIR + '/runs.jsonl') as handle:
            lines = handle.readlines()
        for line in reversed(lines):
            entry = json.loads(line)
            if entry.get('outcome') != 'skipped':
                return tuple(entry.get(key) if type(entry.get(key)) is int else 0
                             for key in ('started_at', 'finished_at'))
    except (OSError, ValueError, AttributeError):
        pass
    return 0, 0


def held_back(upcoming, minutes):
    """The planned run that was due, but had not started, when a new loop replaced this one:
    it still runs. Otherwise `upcoming`."""
    try:
        with open(DIR + '/next-run') as handle:
            earlier, planned = (int(field) for field in handle.read().split())
    except (OSError, ValueError):
        return upcoming
    started, finished = last_run()
    # Also when a run, for example of a change, was still going at that time and ended later
    # than one more planned time.
    if (planned == minutes and earlier <= time.time() and started < earlier
            and (upcoming - minutes * 60 <= earlier or finished >= earlier)):
        return earlier
    return upcoming


def close_interrupted():
    """A run the loop did not see to its end (the loop was stopped) is recorded as failed, so
    the board never shows it running forever."""
    try:
        with open(DIR + '/runs.jsonl') as handle:
            last = json.loads(handle.readlines()[-1])
        if last.get('outcome') == 'running':
            closed = {'started_at': last['started_at'], 'finished_at': int(time.time()), 'outcome': 'failed'}
            if last.get('kind') in (DAILY, CHANGE):
                closed['kind'] = last['kind']
            record(closed)
    except (OSError, IndexError, ValueError, AttributeError, KeyError):
        pass


def forget_companion_requests():
    """A companion request belongs to the run that wrote it, so one left behind starts nothing.
    Returns whether none is left: a run that would see one does not start."""
    path = DIR + '/companions'
    # A rename needs no permission on the folder itself, so one that the agent made unreadable
    # goes too. A link in its place is moved, never followed.
    trash = f'{DIR}/.companions-removed-{time.time_ns()}'
    try:
        os.rename(path, trash)
    except FileNotFoundError:
        pass
    except OSError as error:
        print(f'Could not remove the companion requests of an earlier run: {error}', flush=True)
    for earlier in glob.glob(glob.escape(DIR) + '/.companions-removed-*'):
        remove_tree(earlier)
    return not os.path.lexists(path)


def remove_tree(path):
    """Removes `path` as far as it can. The agent account owns the folders and can take away
    their permissions; the loop runs as that account and gives them back. Links are removed,
    never followed."""
    try:
        if not stat.S_ISDIR(os.lstat(path).st_mode):
            os.unlink(path)
            return
        os.chmod(path, 0o700)
        for name in os.listdir(path):
            remove_tree(os.path.join(path, name))
        os.rmdir(path)
    except (OSError, RecursionError):
        pass


def draw(started, jobs, queued, capacity):
    width = max(40, shutil.get_terminal_size((120, 40)).columns)
    since = f'since {time.strftime("%H:%M", time.gmtime(started))} UTC'
    title = (f'Upkeep daily maintenance {since}' if capacity is None
             else f'Upkeep run {since}, {capacity} side by side at most')
    lines = [title, '']
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


def run_once(cfg, forced, poller=None, only=None, until=None):
    """One run. `forced` (Run now) also starts agents for repositories that did not move.
    `only` holds the repositories that a look found moved, with their fingerprints: the run
    starts their agents without listing them again. Until `until`, the run also starts the
    agents of repositories that `poller` finds moved, when no agent of the run has them."""
    if approvals() != 'auto':
        print('This worker asks before an agent runs a command (agent approvals: ask), and an Upkeep '
              'agent has nobody to ask. Upkeep starts no agent. Use a worker with auto approvals.', flush=True)
        return False
    if not guarded():
        print('This worker does not limit the GitHub access of an agent, so Upkeep starts no agent. '
              'Rebuild the cloud on a current worker image with agent isolation.', flush=True)
        return False
    if session_environment() is None:
        return False
    template = read_prompt('prompt.md')
    if template is None:
        print('The agent instructions in prompt.md cannot be read; this run starts no agent.', flush=True)
        return False
    repositories = names(cfg, 'repositories', REPOSITORY)
    authors = names(cfg, 'authors', AUTHOR, ['app/dependabot'])
    order = [repository for repository in names(cfg, 'order', REPOSITORY) if repository in repositories]
    listed = [repository for repository in repositories if only is None or repository in only]
    print(f'Listing open pull requests in {len(listed)} repositories...' if only is None
          else f'Pull requests changed in {", ".join(listed)}.', flush=True)
    seen = load(DIR + '/fingerprints.json', {})
    seen = seen if isinstance(seen, dict) else {}
    poller = poller or Poller(seen)
    todo, prints, unchanged, limited = [], {}, [], []
    read = False
    for repository in listed:
        if only is not None:
            prints[repository] = only[repository]
            todo.append(repository)
            continue
        try:
            found = open_pull_requests(repository, authors)
        except Limited:
            # These agents wait for a later look: they would meet the same limit, and GitHub
            # asks for no more requests for a while.
            limited = listed[listed.index(repository):]
            break
        # A repository it cannot read yet still gets an agent: the agent asks for access.
        if found is None:
            todo.append(repository)
            continue
        read = True
        if not found:
            # A pull request that opens again later is new to the agent, even when it is the same.
            seen.pop(repository, None)
            poller.started.pop(repository, None)
            forget_open(repository)
            continue
        prints[repository] = fingerprint(found, template)
        # Nothing moved since the last run: the agent would only reach the same answer.
        if not forced and seen.get(repository) == prints[repository]:
            unchanged.append(repository)
            poller.started[repository] = prints[repository]
            continue
        todo.append(repository)
    if unchanged:
        print(f'Unchanged since the last run, skipped: {", ".join(unchanged)}', flush=True)
    if limited:
        if forced:
            poller.forced.update(limited)
        poller.limited()
        print(f'GitHub limits the requests, left for a later listing: {", ".join(limited)}', flush=True)
    elif only is None and read:
        poller.listed()
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
    # The jobs that changes started: those of a run of a change, and those added during a run.
    added = set(jobs) if only is not None else set()
    started = time.time()

    def gone(repository):
        seen.pop(repository, None)
        forget_open(repository)

    def start(job):
        # Without a listing the agent starts from what it finds: the next look compares with the
        # fingerprint of the agent before it.
        if job.repository in prints:
            poller.started[job.repository] = prints[job.repository]
        poller.forced.discard(job.repository)
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
            # A pause that came during the run lets it end; it starts no more agents.
            if until is not None and time.time() < until and poller.due() and not os.path.exists(DIR + '/paused'):
                busy = {job.repository for job in queue} | {job.repository for job, _ in running}
                changes = poller.changed(repositories, authors, template, busy, gone)
                # A pause or the next planned run that came during the search: the changes stay
                # changed for a later look.
                if os.path.exists(DIR + '/paused') or time.time() >= until:
                    changes = {}
                for repository, value in changes.items():
                    prints[repository] = value
                    if repository in order and repository not in first:
                        first.append(repository)
                    job = Job(repository)
                    jobs.append(job)
                    queue.append(job)
                    added.add(job)
            if queue and queue[0] in added and os.path.exists(DIR + '/paused'):
                # A pause leaves the agent of a change for the first look after Resume, which
                # starts it as Run now would, also after a look took the change.
                job = queue.pop(0)
                jobs.remove(job)
                poller.forced.add(job.repository)
                continue
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
    poller.save()
    draw(started, jobs, [], capacity)
    return all(job.code == 0 for job in jobs)


def today():
    return time.strftime('%Y-%m-%d', time.gmtime()).encode()


def daily_due(run_now_only):
    """Whether the daily maintenance pass follows this run: in the first planned run of a UTC
    day, never in a run that only Run now started."""
    return not run_now_only and read_small(DIR + '/daily-day') != today()


def write_day(day):
    """Replaces daily-day with `day` through a new temporary file, or removes it for None."""
    if day is None:
        try:
            os.remove(DIR + '/daily-day')
        except FileNotFoundError:
            pass
        return
    fd, temporary = tempfile.mkstemp(prefix='.daily-day.', dir=DIR)
    with os.fdopen(fd, 'wb') as handle:
        handle.write(day)
    os.replace(temporary, DIR + '/daily-day')


def take_daily():
    """Records today as the day of the daily pass before the pass starts. Returns whether it
    could: a loop that stops during the pass, or that cannot record the day, never starts the
    pass twice in one day."""
    try:
        write_day(today())
    except OSError as error:
        print(f'Could not record the day of the daily maintenance pass, so it does not start: {error}', flush=True)
        return False
    # The report of an earlier pass does not stay: the board shows only what this pass reports.
    try:
        os.remove(DIR + '/status.d/' + DAILY + '.json')
    except OSError:
        pass
    return True


def run_daily(cfg, template):
    """The daily maintenance pass, recorded as a run of its own. Returns whether it started.
    `cfg` and `template` are what the run read before any agent of it started: the agents can
    change the files meanwhile, so the pass never reads them again."""
    if template is None:
        return False
    if approvals() != 'auto':
        return False
    environment = session_environment()
    earlier = read_small(DIR + '/daily-day')
    if environment is None or not codex_enabled(environment):
        return False
    # The checks above can take minutes; a pause that came meanwhile holds the pass back too.
    # The requests go before the pass shows as running to the exchange, as for a run.
    if os.path.exists(DIR + '/paused') or not forget_companion_requests() or not take_daily():
        return False
    repositories = names(cfg, 'repositories', REPOSITORY)
    # Validated names only; they hold no `@`, so they never make another placeholder.
    template = template.replace('@@REPOSITORIES@@', ', '.join(repositories))
    begun = int(time.time())
    job = Job('daily maintenance', DAILY, DAILY_LIMIT)
    try:
        record({'started_at': begun, 'kind': DAILY, 'outcome': 'running'})
        thread = threading.Thread(target=job.run, args=(template,), daemon=True)
        thread.start()
        try:
            while thread.is_alive():
                merge_status(repositories)
                draw(begun, [job], [], None)
                time.sleep(REFRESH)
        finally:
            thread.join()
        merge_status(repositories)
        draw(begun, [job], [], None)
    finally:
        # A pass whose agent never started, for example because the policy changed in the
        # meantime or the run could not be recorded, stays due for the next planned run.
        if not job.launched:
            try:
                write_day(earlier)
            except OSError as error:
                print(f'Could not make the daily maintenance pass due again: {error}', flush=True)
        # Also when the pass fails here, so the board never shows it running after it ended.
        record({'started_at': begun, 'finished_at': int(time.time()), 'kind': DAILY,
                'outcome': 'done' if job.code == 0 else 'failed'})
    return True


def poll_once(poller, until):
    """Between the planned runs: when the pull requests of a repository moved, a run of the
    kind change starts the agents of those repositories at once. Returns whether one ran."""
    if approvals() != 'auto' or not guarded() or not poller.due():
        return False
    template = read_prompt('prompt.md')
    if template is None:
        return False
    cfg = config()
    repositories = names(cfg, 'repositories', REPOSITORY)
    gone = []

    def forget(repository):
        if forget_seen(repository):
            gone.append(repository)

    moved = poller.changed(repositories, names(cfg, 'authors', AUTHOR, ['app/dependabot']), template, forget=forget)
    if gone:
        # The board drops the pull requests that closed without an agent at once.
        merge_status(repositories)
    # Back where the last clean agent left it, for example after a failed agent: nothing to do.
    seen = load(DIR + '/fingerprints.json', {})
    for repository, value in list(moved.items()):
        if isinstance(seen, dict) and seen.get(repository) == value and repository not in poller.forced:
            poller.started[repository] = value
            del moved[repository]
    # A pause that came during the search: Resume starts these, as they are still changed. The
    # planned run that came due during the search lists them itself.
    if not moved or os.path.exists(DIR + '/paused') or time.time() >= until:
        poller.save()
        return False
    # Taken before the run: a run that starts no agent, for example because the session
    # environment failed, starts no run again; the next change or the planned run does.
    for repository, value in moved.items():
        poller.started[repository] = value
        poller.forced.discard(repository)
    poller.save()
    begun = int(time.time())
    before = merged()
    # As before a planned run: a run that could see a companion request of an earlier run fails.
    ok = forget_companion_requests()
    if ok:
        record({'started_at': begun, 'kind': CHANGE, 'outcome': 'running'})
        try:
            ok = run_once(cfg, False, poller, moved, until)
        except Exception as error:  # noqa: BLE001 - one failed run must not end the loop
            print(f'The run failed: {error}', flush=True)
            ok = False
    record({'started_at': begun, 'finished_at': int(time.time()), 'kind': CHANGE,
            'outcome': 'done' if ok else 'failed', 'exit': 0 if ok else 1, 'merged': len(merged() - before)})
    return True


def main():
    for folder in ('repos', 'logs', 'status.d'):
        os.makedirs(os.path.join(DIR, folder), exist_ok=True)
    close_interrupted()
    cfg = config()
    minutes = every(cfg)
    upcoming = next_slot(cfg)
    upcoming = held_back(upcoming, minutes)
    write_next(upcoming, minutes)
    print(f'Upkeep waits for its next run at {time.strftime("%H:%M", time.gmtime(upcoming))} UTC.', flush=True)
    poller = restored()
    while True:
        # The exchange asks for a restart after it installed a complete new loop. Here, at the
        # top, no run is going, and a due run or Run now is left for the new loop.
        if os.path.exists(DIR + '/restart') and not os.path.exists(DIR + '/installing'):
            os.remove(DIR + '/restart')
            print('\nAn exchange installed a new loop; starting it.', flush=True)
            os.execv('/bin/bash', ['/bin/bash', '--noprofile', '--norc', DIR + '/run.sh', *sys.argv[1:]])
        cfg = config()
        paused = os.path.exists(DIR + '/paused')
        # A pause skips the runs it covers; the next run is the next one on the schedule.
        if every(cfg) != minutes or (paused and time.time() >= upcoming):
            minutes = every(cfg)
            upcoming = next_slot(cfg)
            write_next(upcoming, minutes)
        forced = not paused and take_run_now()
        planned = time.time() >= upcoming
        if not paused and (forced or planned):
            # A Run now that came since is taken too, with its token: it asked for this run.
            take_run_now()
            cfg = config()
            # Read before any agent of this run starts, so no agent of it can change them.
            daily_template = read_prompt('daily.md')
            begun = int(time.time())
            before = merged()
            # Before the run shows as running to the exchange. A run that could see a request of
            # an earlier run fails at once.
            ok = forget_companion_requests()
            if ok:
                record({'started_at': begun, 'outcome': 'running'})
                try:
                    # Changes during the run join it until its next planned run would start.
                    ok = run_once(cfg, forced, poller, None, (begun // (every(cfg) * 60) + 1) * every(cfg) * 60)
                except Exception as error:  # noqa: BLE001 - one failed run must not end the loop
                    print(f'The run failed: {error}', flush=True)
                    ok = False
            record({'started_at': begun, 'finished_at': int(time.time()), 'outcome': 'done' if ok else 'failed',
                    'exit': 0 if ok else 1, 'merged': len(merged() - before)})
            # A pause that came during the run holds the pass back until a planned run after it.
            # A Run now that came when the planned run was due anyway is that planned run too.
            if not os.path.exists(DIR + '/paused') and daily_due(forced and not planned):
                try:
                    run_daily(cfg, daily_template)
                except Exception as error:  # noqa: BLE001 - one failed pass must not end the loop
                    print(f'The daily maintenance pass failed: {error}', flush=True)
            ended = int(time.time())
            # Run now during this run asked for the run that just ended. Its token is
            # recorded, so the same Run now sent again starts no run either.
            take_run_now()
            # Slots that passed while the run was busy were skipped, never run beside it.
            slot = (begun // (every(cfg) * 60) + 1) * every(cfg) * 60
            while slot < ended:
                record({'started_at': slot, 'outcome': 'skipped'})
                slot += every(cfg) * 60
            minutes = every(cfg)
            upcoming = next_slot(cfg)
            write_next(upcoming, minutes)
            ran = True
        else:
            # A run of a change leaves the planned run, Run now and the daily pass for the next turn.
            try:
                ran = not paused and poll_once(poller, upcoming)
            except Exception as error:  # noqa: BLE001 - one failed look must not end the loop
                print(f'The look for changes failed: {error}', flush=True)
                ran = False
            # The planned run stays due; later slots that passed during the run were skipped.
            slot = upcoming + minutes * 60
            while ran and slot < time.time():
                record({'started_at': slot, 'outcome': 'skipped'})
                slot += minutes * 60
        if ran:
            print(f'\nUpkeep waits for its next run at {time.strftime("%H:%M", time.gmtime(upcoming))} UTC.', flush=True)
        time.sleep(10)


if __name__ == '__main__':
    main()
