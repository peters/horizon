#!/usr/bin/env python3
"""Run the candidate a second time in the sandbox and the home of a fixture that runs.

A fixture owns one private home and one display, and its candidate runs until the
fixture stops. A procedure that closes the window of that candidate, or that ends
it, needs a further start of the same home while the fixture still runs: the
candidate then asks the window that runs to show itself, prints one line about it,
and ends.

A run writes the values of its sandbox into its state folder. This script reads
them, enters that sandbox, runs the candidate once with the arguments that it is
given, prints what the candidate printed, and prints the status of the start. It
refuses when no fixture runs at that state folder, and it never reaches the home
or the desktop of the person: the sandbox binds the private home of the fixture
over the home of the person and keeps the display of the fixture only.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

import sandbox

# The file that a running fixture writes for a second start, in its state folder.
DOCUMENT_NAME = 'second-start.json'
# The longest wait for one start of the candidate. A window that runs answers in
# seconds, so a longer wait means that the candidate hangs.
PATIENCE = 120.0


def document_path(state):
    return Path(state) / DOCUMENT_NAME


def document(namespace, environment, base, app, pid):
    """What a second start needs of a fixture: its sandbox and its own values.

    The environment holds only the values that the fixture sets. The values of the
    shell that started the fixture stay in that shell: it can hold a credential.
    """
    changed = {key: value for key, value in environment.items() if base.get(key) != value}
    return {'app': str(app), 'namespace': list(namespace), 'environment': changed, 'pid': pid}


def write(state, values):
    """Write the values of the run that starts, so a second start reads them."""
    path = document_path(state)
    path.write_text(json.dumps(values, indent=2))
    path.chmod(0o600)
    return path


def remove(state):
    """Remove the file of a second start when the run ends."""
    document_path(state).unlink(missing_ok=True)


def process_alive(pid):
    """Whether the process `pid` runs."""
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def values_of(state, alive=process_alive):
    """The values of the fixture that runs at `state`.

    Stops with one sentence when no fixture runs there: only a run that starts
    writes the file, and the end of the run removes it.
    """
    try:
        values = json.loads(document_path(state).read_text())
    except FileNotFoundError:
        raise SystemExit(f'No fixture runs at {state}: it holds no {DOCUMENT_NAME}.') from None
    except (OSError, json.JSONDecodeError) as error:
        raise SystemExit(f'The {DOCUMENT_NAME} of {state} is not readable: {error}')
    if not alive(values.get('pid')):
        raise SystemExit(f'The fixture of {state} ended: its process {values.get("pid")} does not run.')
    return values


def command(values, arguments=()):
    """The command line of one start of the candidate in the sandbox of a fixture."""
    return [*values['namespace'], str(values['app']), *arguments]


def environment(values, base=None):
    """The environment of that start: this shell without the Horizon values, and the values of the fixture."""
    environment = sandbox.without_inherited_horizon(os.environ if base is None else base)
    environment.update(values['environment'])
    return environment


def start(state, arguments=(), run=subprocess.run, alive=process_alive, base=None, patience=PATIENCE):
    """Run the candidate once in the sandbox of the fixture at `state`.

    Answers the status of the candidate and prints what it printed, which a
    procedure reads: a window that runs answers a second start with its line.
    """
    values = values_of(state, alive)
    outcome = run(command(values, arguments), env=environment(values, base), timeout=patience)
    print(f'The candidate ended with status {outcome.returncode}.')
    return outcome.returncode


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--state', type=Path, required=True,
                        help='State folder of the fixture that runs, as --state gave it')
    parser.add_argument('start', nargs=argparse.REMAINDER,
                        help='The arguments of the candidate, after `--`: none shows its window again')
    return parser.parse_args(argv)


def main(argv=None):
    args = arguments(argv)
    return start(args.state.resolve(), [item for item in args.start if item != '--'])


if __name__ == '__main__':
    sys.exit(main())
