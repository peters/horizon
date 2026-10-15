"""One Upkeep exchange on a worker: install the loop, keep its session running, apply Run now
and Pause, and print what the board shows. Idempotent.

It runs as the account that owns /workspace/home: the agent account where the worker isolates
agents (Horizon starts it through horizon-worker-tailnet agent), root where agents run as
root. It refuses any other account, so it never writes into a folder that a less privileged
account controls. Every path below the workspace folders is opened relative to a checked
folder and without following links, and files are replaced through new temporary files.
"""
import base64
import json
import os
import re
import secrets
import stat
import subprocess
import sys

HOME = '/workspace/home'
SESSIONS = '/workspace/sessions'
SESSION_DATA = '/workspace/session-data'
FOLDER = 'upkeep'
STATUS_LIMIT = 1024 * 1024
RUNS_LIMIT = 256 * 1024
# Horizon reads at most 4 MiB of output; escaping can make the status six times longer.
OUTPUT_LIMIT = 3 * 1024 * 1024
# Horizon reads times as 64-bit integers.
TIME_LIMIT = 2 ** 62
# The log of the GitHub changes that root's GitHub service refused on the worker. The agent
# cannot write it.
REFUSALS = (('/workspace/.horizon-gate', 'refused.jsonl'),)
REFUSALS_OWNER = 0
REFUSED_KINDS = {'dispatch': 'a workflow start', 'merge': 'a merge'}
# Enough for the last REFUSALS_KEPT lines at their longest: a reason of 300 characters that
# JSON escapes to six bytes each, and the other fields.
REFUSALS_LIMIT = 64 * 1024
REFUSALS_KEPT = 20
SESSION = re.compile(r'upkeep-[A-Za-z0-9_-]{1,90}')
REVISION = re.compile(r'[0-9a-f]{40}|[0-9a-f]{64}')


class Refused(Exception):
    pass


def owned_folder(path):
    """An existing folder of this account, opened without following a link."""
    try:
        fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    except OSError as error:
        raise Refused(f'{path} is not a folder') from error
    if os.fstat(fd).st_uid != os.geteuid():
        os.close(fd)
        raise Refused(f'{path} belongs to another account')
    return fd


def folder(parent, name, mode=0o700):
    """The folder `name` in `parent`, created when missing. A link, another kind of file or a
    folder of another account is refused."""
    try:
        os.mkdir(name, mode, dir_fd=parent)
    except FileExistsError:
        pass
    try:
        fd = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent)
    except OSError as error:
        raise Refused(f'{name} is not a folder') from error
    info = os.fstat(fd)
    if info.st_uid != os.geteuid():
        os.close(fd)
        raise Refused(f'{name} belongs to another account')
    if stat.S_IMODE(info.st_mode) != mode:
        os.fchmod(fd, mode)
    return fd


def read(parent, name, limit, tail=False):
    """At most `limit` bytes of the regular file `name`, or None. A link, a pipe or a device
    is never read, so none can redirect or block the exchange."""
    try:
        fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=parent)
    except OSError:
        return None
    info = os.fstat(fd)
    if not stat.S_ISREG(info.st_mode):
        os.close(fd)
        return None
    with os.fdopen(fd, 'rb') as handle:
        if tail and info.st_size > limit:
            handle.seek(info.st_size - limit)
        return handle.read(limit)


def write(parent, name, text, mode=0o600):
    """Replaces `name` with `text` through a new temporary file. A file that had that name, a
    link included, is replaced, never written through; a folder stops the exchange."""
    data = text.encode()
    try:
        current = os.stat(name, dir_fd=parent, follow_symlinks=False)
    except FileNotFoundError:
        current = None
    if current is not None and stat.S_ISDIR(current.st_mode):
        raise Refused(f'{name} is a folder')
    if (current is not None and stat.S_ISREG(current.st_mode) and stat.S_IMODE(current.st_mode) == mode
            and read(parent, name, len(data) + 1) == data):
        return False
    temporary = f'.{name}.{secrets.token_hex(8)}'
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode, dir_fd=parent)
    try:
        with os.fdopen(fd, 'wb') as handle:
            handle.write(data)
            os.fchmod(handle.fileno(), mode)
        os.replace(temporary, name, src_dir_fd=parent, dst_dir_fd=parent)
        return True
    except BaseException:
        try:
            os.unlink(temporary, dir_fd=parent)
        except OSError:
            pass
        raise


def create(parent, name):
    """Makes sure that the marker `name` is a regular file. One that exists keeps its content;
    a link or another kind of file is replaced, and a folder stops the exchange."""
    try:
        if stat.S_ISREG(os.stat(name, dir_fd=parent, follow_symlinks=False).st_mode):
            return
    except FileNotFoundError:
        pass
    write(parent, name, '')


def install(payload, upkeep):
    """Writes the loop, its settings and the session binding. A new loop is marked as being
    installed first, so an exchange that stops midway leaves the mark for the next one."""
    session = payload['session']
    write(upkeep, 'config.json', json.dumps(payload['config'], indent=1))
    write(upkeep, 'prompt.md', payload['prompt'])
    if (read(upkeep, 'run.sh', len(payload['run'].encode()) + 1) != payload['run'].encode()
            or read(upkeep, 'run.py', len(payload['scheduler'].encode()) + 1) != payload['scheduler'].encode()):
        write(upkeep, 'installing', '')
    write(upkeep, 'run.sh', payload['run'], 0o700)
    write(upkeep, 'run.py', payload['scheduler'])
    # The binding that makes the session a Codex session to the worker's services: its
    # processes may ask for GitHub access and open browser panels for this cloud.
    sessions = owned_folder(SESSIONS)
    try:
        binding = folder(sessions, session)
        try:
            write(binding, 'agent', 'codex\n')
            write(binding, 'revision', payload['revision'] + '\n')
            create(binding, 'shared-checkout')
            create(binding, 'launch-requested')
        finally:
            os.close(binding)
    finally:
        os.close(sessions)
    data = owned_folder(SESSION_DATA)
    try:
        os.close(folder(data, session))
    finally:
        os.close(data)


def remove(parent, name):
    try:
        os.unlink(name, dir_fd=parent)
    except FileNotFoundError:
        pass
    except IsADirectoryError as error:
        raise Refused(f'{name} is a folder') from error


RUN_TOKEN = re.compile(r'[0-9a-f]{32}')


def signal(payload, upkeep):
    """Pause and resume first: Run now while paused is dropped, so a resume never starts a
    run that was asked for long before. A Run now with a token is applied once: Horizon sends
    the same token again when it did not learn whether an earlier exchange applied it."""
    paused = payload.get('paused')
    if paused is True:
        write(upkeep, 'paused', '')
    elif paused is False:
        remove(upkeep, 'paused')
    run_now = payload.get('run_now')
    if read(upkeep, 'paused', 0) is not None:
        remove(upkeep, 'run-now')
    elif run_now:
        token = run_now if isinstance(run_now, str) and RUN_TOKEN.fullmatch(run_now) else None
        if token is None:
            write(upkeep, 'run-now', '')
        elif token.encode() not in (read(upkeep, 'run-now', 64), read(upkeep, 'run-now-token', 64)):
            # The signal holds its token. The loop records the token when it takes the signal,
            # so a token sent again is pending or taken, and never asks for a second run.
            write(upkeep, 'run-now', token)


def tmux(*args):
    return subprocess.run(['tmux', '-L', 'horizon-cloud', *args], capture_output=True, text=True, timeout=20)


def runs_loop(session):
    """Whether the session exists, and whether it runs the loop."""
    start = tmux('display-message', '-p', '-t', '=' + session + ':', '#{pane_start_command}')
    return start.returncode == 0, start.returncode == 0 and 'upkeep/run.sh' in start.stdout


def keep_loop(session, directory):
    """Whether the loop's session runs, and whether it already ran before this exchange."""
    # A reconnect relaunches recorded sessions as interactive agents; only the loop belongs here.
    exists, loop = runs_loop(session)
    # A loop that already ran before this exchange has survived its start; only then is its
    # session ready for a panel to attach.
    settled = loop
    if exists and not loop:
        tmux('kill-session', '-t', '=' + session)
    if not loop:
        command = ['/bin/bash', '--noprofile', '--norc', directory + '/run.sh', session]
        started = tmux('new-session', '-d', '-s', session, '-c', directory,
                       '-e', 'PATH=' + os.environ.get('PATH', '/usr/local/bin:/usr/bin:/bin'),
                       '-e', 'BASH_ENV=', '-e', 'ENV=', *command)
        # Another exchange may have started it a moment ago.
        loop = started.returncode == 0 or runs_loop(session)[1]
    return loop, settled


def moment(value):
    """A time Horizon can read: an integer, not a flag, in range."""
    return value if type(value) is int and -TIME_LIMIT < value < TIME_LIMIT else None


def run_record(line):
    """The fields the board uses from one line of runs.jsonl, or None."""
    try:
        run = json.loads(line)
    except (ValueError, RecursionError):
        return None
    if not isinstance(run, dict) or moment(run.get('started_at')) is None:
        return None
    record = {'started_at': run['started_at']}
    if moment(run.get('finished_at')) is not None:
        record['finished_at'] = run['finished_at']
    if type(run.get('merged')) is int and 0 <= run['merged'] < 2 ** 31:
        record['merged'] = run['merged']
    if isinstance(run.get('outcome'), str):
        # A lone surrogate from a \ud800 escape is not valid JSON for Horizon.
        record['outcome'] = run['outcome'][:40].encode('utf-8', 'replace').decode()
    return record


def refusal_text(entry):
    """The board's line for one refusal, from its fixed fields only, or None."""
    if not isinstance(entry, dict) or type(entry.get('at')) is not int or not 0 < entry['at'] < TIME_LIMIT \
            or not isinstance(entry.get('reason'), str):
        return None
    clean = lambda value: ''.join(c for c in str(value) if c.isprintable())[:200]
    where = ' on ' + clean(entry['repository']) if isinstance(entry.get('repository'), str) else ''
    kind = entry.get('kind')
    what = REFUSED_KINDS.get(kind, 'a GitHub change') if isinstance(kind, str) else 'a GitHub change'
    return 'The worker refused %s%s: %s' % (what, where, clean(entry['reason']))


def refusals():
    """The newest lines of the refusal log that root wrote, as {at, text}, oldest first."""
    found = []
    for path, name in REFUSALS:
        try:
            parent = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        except OSError:
            continue
        try:
            if os.fstat(parent).st_uid != REFUSALS_OWNER:
                continue
            data = read(parent, name, REFUSALS_LIMIT, tail=True) or b''
        finally:
            os.close(parent)
        for line in data.decode('utf-8', 'replace').splitlines():
            try:
                entry = json.loads(line)
            except ValueError:
                continue
            text = refusal_text(entry)
            if text is not None:
                found.append({'at': entry['at'], 'text': text})
    found.sort(key=lambda entry: entry['at'])
    return found[-REFUSALS_KEPT:]


def report(upkeep, loop, settled):
    """What the board shows. Everything here comes from files that the agent can write, so
    only the fields and types that Horizon reads are sent back."""
    status = read(upkeep, 'status.json', STATUS_LIMIT)
    lines = (read(upkeep, 'runs.jsonl', RUNS_LIMIT, tail=True) or b'').splitlines()[-200:]
    runs = [record for record in map(run_record, lines) if record is not None]
    try:
        next_run = moment(int((read(upkeep, 'next-run', 64) or b'').split()[0]))
    except (ValueError, IndexError):
        next_run = None
    answer = {'loop': loop, 'settled': settled, 'runs': runs, 'next_run': next_run,
              'paused': read(upkeep, 'paused', 0) is not None, 'refused': refusals(),
              'status': status.decode('utf-8', 'replace') if status is not None else None}
    text = json.dumps(answer, allow_nan=False)
    if len(text) > OUTPUT_LIMIT:
        answer['status'] = None
        text = json.dumps(answer, allow_nan=False)
    return text


def exchange(payload):
    session = payload['session']
    if not SESSION.fullmatch(session) or not REVISION.fullmatch(payload['revision']):
        raise Refused('the session or revision is not valid')
    home = owned_folder(HOME)
    try:
        upkeep = folder(home, FOLDER)
        try:
            install(payload, upkeep)
            signal(payload, upkeep)
            # Last, after every file is in place: a running loop starts the new one between two
            # runs, and only ever sees a complete install.
            if read(upkeep, 'installing', 0) is not None:
                write(upkeep, 'restart', '')
                remove(upkeep, 'installing')
            loop, settled = keep_loop(session, HOME + '/' + FOLDER)
            return report(upkeep, loop, settled)
        finally:
            os.close(upkeep)
    finally:
        os.close(home)


def main():
    payload = json.loads(base64.b64decode(sys.argv[1]))
    try:
        print(exchange(payload))
    except Refused as error:
        print(f'Upkeep refused this worker: {error}.', file=sys.stderr)
        sys.exit(1)


if __name__ == '__main__':
    main()
