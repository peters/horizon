"""Print the state of the given agent sessions as one JSON object.

Horizon sends this script over SSH when panels of a cloud are parked, so it
needs nothing on the worker beyond Python 3 and tmux. It only reads: it never
takes the session lock, attaches, starts or stops anything.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

LINES = 8
LINE_CHARS = 240
SESSIONS = 64


def tmux_command():
    # The same server the sessions use; see horizon-worker-idle.
    if os.geteuid() == 0 and Path('/run/horizon-tailnet/agent-isolation').exists():
        return ['env', '-i', 'PATH=/usr/local/bin:/usr/bin:/bin', 'HOME=/workspace/home',
                'setpriv', '--reuid=10001', '--regid=10001', '--clear-groups',
                '--no-new-privs', '--bounding-set=-all', '--', 'tmux', '-L', 'horizon-cloud']
    return ['tmux', '-L', 'horizon-cloud']


def tmux(*args):
    try:
        result = subprocess.run(tmux_command() + list(args), capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.SubprocessError):
        return None
    return result.stdout if result.returncode == 0 else None


def exit_status(session):
    try:
        return int(Path('/workspace/sessions', session, 'exit-status').read_text().strip())
    except (OSError, ValueError):
        return None


def status(session, now):
    target = '=' + session + ':'
    activity = tmux('display-message', '-p', '-t', target, '#{window_activity}')
    if activity is None:
        return {'id': session, 'state': 'missing'}
    screen = tmux('capture-pane', '-p', '-J', '-t', target) or ''
    lines = [line.rstrip()[:LINE_CHARS] for line in screen.splitlines() if line.strip()]
    code = exit_status(session)
    stamp = activity.strip()
    return {
        'id': session,
        'state': 'running' if code is None else 'exited',
        'exit_status': code,
        'activity_age_seconds': max(0, int(now - int(stamp))) if stamp.isdigit() else None,
        'lines': lines[-LINES:],
    }


def main():
    now = time.time()
    sessions = sys.argv[1:SESSIONS + 1]
    print(json.dumps({'sessions': [status(session, now) for session in sessions]}))


if __name__ == '__main__':
    main()
