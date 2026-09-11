#!/usr/bin/python3 -I
"""Explicit detached setup submission; retained Rust state remains authoritative."""

import io
import json
import os
import selectors
import subprocess
import sys
import time

REQUEST_LIMIT = 128 * 1024
OBSERVATION_LIMIT = 128 * 1024
RESPONSE_LIMIT = 2 * OBSERVATION_LIMIT
GIT_REQUEST_LIMIT = 16 * 1024
GIT_OBSERVATION_LIMIT = 1024
OBSERVE_SECONDS = 30
HANDOFF_SECONDS = 15
HELPER = '/usr/local/bin/horizon-repository'
ENVIRONMENT = {'PATH': '/usr/bin:/bin', 'LC_ALL': 'C'}


class LaunchError(Exception):
    """Static diagnostics never include request content or process output."""


def unique_fields(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise LaunchError('setup observation contains duplicate fields')
        result[key] = value
    return result


def git_observation(value, code):
    if (not isinstance(value, dict) or set(value) != {'version', 'state', 'reason', 'checkout'}
            or type(value['version']) is not int or value['version'] != 1
            or not isinstance(value['state'], str)
            or value['state'] not in ('absent', 'claimed_unknown', 'complete', 'error')):
        raise LaunchError('Git setup observation is invalid')
    reason = value['reason']
    if reason is not None and (not isinstance(reason, str) or reason not in (
            'invalid', 'unsupported', 'unsafe_root', 'conflict', 'storage', 'git',
            'interrupted', 'unsupported_repository')):
        raise LaunchError('Git setup observation reason is invalid')
    checkout = '/workspace/horizon/repository' if value['state'] == 'complete' else None
    if value['checkout'] != checkout or value['state'] == 'error' and reason is None:
        raise LaunchError('Git setup observation is inconsistent')
    expected = (0 if value['state'] in ('complete', 'absent') and reason is None
                else 2 if reason in ('invalid', 'unsupported') else 1)
    if code != expected or value['state'] == 'absent' and reason is not None:
        raise LaunchError('Git setup observation exit status is inconsistent')
    return value, code


def observe(request, git=False):
    try:
        result = subprocess.run([HELPER, 'git-status' if git else 'setup-status'], input=request,
            capture_output=True, check=False, timeout=OBSERVE_SECONDS,
            cwd='/', env=ENVIRONMENT, close_fds=True)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise LaunchError('setup observation did not complete') from error
    if (result.returncode not in (0, 1, 2, 4) or result.stderr
            or len(result.stdout) > (GIT_OBSERVATION_LIMIT if git else OBSERVATION_LIMIT)
            or not result.stdout.endswith(b'\n')):
        raise LaunchError('setup observation is unavailable')
    try:
        value = json.loads(result.stdout.decode('utf-8'), object_pairs_hook=unique_fields)
    except (ValueError, RecursionError) as error:
        raise LaunchError('setup observation is invalid') from error
    if git:
        return git_observation(value, result.returncode)
    if (not isinstance(value, dict) or set(value) != {'version', 'status', 'recording', 'reason', 'execution'}
            or type(value['version']) is not int or value['version'] != 1
            or value['status'] not in ('absent', 'claimed_unknown', 'completed', 'error', 'rejected')
            or value['recording'] not in ('not_acknowledged', 'observed')
            or value['reason'] is not None and not isinstance(value['reason'], str)
            or value['execution'] is not None and not isinstance(value['execution'], dict)):
        raise LaunchError('setup observation is invalid')
    expected = {'absent': 0, 'claimed_unknown': 4, 'error': 1, 'rejected': 2}
    if value['status'] == 'completed':
        execution = value['execution']
        states = {'published': 0, 'rejected': 2, 'unpublished': 1,
                  'published_unsynchronized': 1, 'rename_unconfirmed': 1}
        if (not isinstance(execution, dict) or not isinstance(execution.get('state'), str)
                or execution['state'] not in states or value['recording'] != 'observed'
                or value['reason'] is not None):
            raise LaunchError('setup completion observation is invalid')
        expected['completed'] = states[execution['state']]
    elif value['execution'] is not None or value['recording'] != 'not_acknowledged':
        raise LaunchError('setup observation recording is invalid')
    if result.returncode != expected[value['status']]:
        raise LaunchError('setup observation exit status is inconsistent')
    if value['status'] == 'absent' and value != {
        'version': 1, 'status': 'absent', 'recording': 'not_acknowledged', 'reason': None, 'execution': None
    }:
        raise LaunchError('setup absence was not established')
    return value, result.returncode


def handoff(request, git=False):
    try:
        process = subprocess.Popen([HELPER, 'git-prepare' if git else 'setup'], stdin=subprocess.PIPE,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, bufsize=0,
            close_fds=True, start_new_session=True, cwd='/', env=ENVIRONMENT, umask=0o077)
    except OSError as error:
        raise LaunchError('independent setup could not be spawned') from error
    complete = False
    try:
        descriptor = process.stdin.fileno()
        os.set_blocking(descriptor, False)
        remaining = memoryview(request)
        deadline = time.monotonic() + HANDOFF_SECONDS
        with selectors.DefaultSelector() as ready:
            ready.register(descriptor, selectors.EVENT_WRITE)
            while remaining:
                timeout = deadline - time.monotonic()
                if timeout <= 0 or not ready.select(timeout):
                    break
                try:
                    written = os.write(descriptor, remaining)
                except BlockingIOError:
                    continue
                if written <= 0:
                    break
                remaining = remaining[written:]
            complete = not remaining
    except (OSError, ValueError):
        complete = False
    finally:
        try:
            process.stdin.close()
        except OSError:
            complete = False
        # Reap an already-exited child only. A running child transfers to worker PID1
        # when this one-shot launcher exits; never wait, kill or infer its outcome.
        process.poll()
    return complete


def execute(stream, git=False):
    if stream is None:
        return 'rejected', None, 2
    try:
        limit = GIT_REQUEST_LIMIT if git else REQUEST_LIMIT
        request = stream.read(limit + 1)
    except (OSError, ValueError):
        return 'rejected', None, 2
    if len(request) > limit:
        return 'rejected', None, 2
    observation, code = observe(request, True) if git else observe(request)
    if observation['state' if git else 'status'] != 'absent':
        return 'observed', observation, code
    complete = handoff(request, True) if git else handoff(request)
    return ('submitted', None, 0) if complete else ('handoff_unconfirmed', None, 1)


def run(stream, output, diagnostics, git=False):
    try:
        state, observation, code = execute(stream, True) if git else execute(stream)
    except LaunchError as error:
        state, observation, code = 'error', None, 1
        try:
            diagnostics.write(str(error)+'\n')
            diagnostics.flush()
        except OSError:
            pass
    response = json.dumps({'version': 1, 'state': state, 'observation': observation},
                          ensure_ascii=False, separators=(',', ':')).encode()+b'\n'
    try:
        if len(response) > RESPONSE_LIMIT or output.write(response) != len(response):
            return 3
        output.flush()
    except OSError:
        return 3
    return code


def main():
    arguments = sys.argv[1:]
    if arguments not in ([], ['--git']):
        try:
            if sys.stderr is not None:
                os.write(sys.stderr.fileno(), b'Usage: horizon-setup-launch [--git] < setup-request.json\n')
        except (OSError, ValueError):
            pass
        return 2
    # Do not leave buffered output for interpreter shutdown to retry after exit 3.
    if sys.stdout is None:
        return 3
    try:
        output = io.FileIO(sys.stdout.fileno(), 'wb', closefd=False)
    except (OSError, ValueError):
        return 3
    with output:
        diagnostics = io.StringIO()
        if sys.stderr is not None:
            try:
                diagnostics = io.TextIOWrapper(io.FileIO(sys.stderr.fileno(), 'wb', closefd=False),
                                             encoding='utf-8', write_through=True)
            except (OSError, ValueError):
                pass
        with diagnostics:
            stream = sys.stdin.buffer if sys.stdin is not None else None
            return run(stream, output, diagnostics, True) if arguments else run(stream, output, diagnostics)


if __name__ == '__main__':
    sys.exit(main())
