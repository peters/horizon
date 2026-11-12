#!/usr/bin/env python3
"""Own a disposable application/X11 lab and read-only noVNC viewer until Ctrl-C."""
import argparse
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time

import sandbox


# The directory the host X server keeps its socket and keymap files in.
TMP_DIR = Path('/tmp')
# The attention stand-in: the panel entry of this fixture, and the name of the
# program that a Claude panel of the fixture resolves on its `PATH`. Horizon
# starts a Claude panel of the default command through that name, gives it its
# own arguments and attaches the work ledger of the panel to it.
ATTENTION_PANEL_ENTRY = Path(__file__).resolve().parent / 'attention_panel.json'
ATTENTION_PROGRAM = 'claude'
ATTENTION_BIN_FOLDER = 'bin'
ATTENTION_STANDIN_FOLDER = 'attention-agent'
ATTENTION_TIMER_FILE = 'stand-in.json'


def bound_port(process, path, pattern):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f'Listener exited before readiness: {path}')
        match = re.search(pattern, path.read_text() if path.exists() else '', re.MULTILINE)
        if match:
            return int(match.group(1))
        time.sleep(0.05)
    raise RuntimeError(f'Listener did not report its bound port: {path}')


WEB_PROXY = """
import socket, sys
from pathlib import Path
from websockify import WebSocketProxy
with socket.socket() as listener:
    listener.bind(('127.0.0.1', 0))
    listener.listen(100)
    proxy = WebSocketProxy(listen_fd=listener.fileno(), web=sys.argv[1],
                          target_host='127.0.0.1', target_port=int(sys.argv[2]))
    Path(sys.argv[3]).write_text(str(listener.getsockname()[1]))
    proxy.start_server()
"""


PRESET_RESUME_MODES = ('fresh', 'last')

# The one file that holds every secret value of the private Horizon home of the
# fixture. The schema is the schema of
# `crates/horizon-core/src/secret_store/schema.sql`, which the store of Horizon
# includes, so a file that this fixture writes is a file that Horizon reads.
SECRETS_FILE_NAME = 'secrets.sqlite'

# The folder of the Horizon home below `HOME`, from
# `crates/horizon-browser-control/src/paths.rs` (`default_horizon_root`): the
# candidate derives every default path of a person from `HOME`, and the fixture
# binds `HOME` to its private home, so the home of the candidate inside the
# fixture is `<private home>/.horizon`.
HORIZON_HOME_FOLDER = '.horizon'


def horizon_secrets_file(private_home):
    """The secrets file that the candidate of the fixture reads.

    The candidate resolves its home from `HOME`, which bwrap binds to the private
    home of the state folder, and `HorizonHome::secrets_path` is
    `<home>/secrets.sqlite` (`crates/horizon-core/src/horizon_home.rs`). A file at
    another path is a file that no panel of the fixture reads, so a seeded value
    would never reach an agent panel.
    """
    return Path(private_home) / HORIZON_HOME_FOLDER / SECRETS_FILE_NAME
SECRETS_SCHEMA_FILE = (Path(__file__).resolve().parent.parent.parent
                       / 'crates' / 'horizon-core' / 'src' / 'secret_store' / 'schema.sql')

# The schema version that `crates/horizon-core/src/secret_store/schema.rs` writes. A
# file of another version is refused by the store, which is what the crate test that
# opens a file of this fixture proves.
SECRETS_SCHEMA_VERSION = 1

# The id rules of `crates/horizon-core/src/secret.rs`: one credential reference is
# 1 to 64 characters of letters, digits, '.', '_' and '-'.
MAX_CREDENTIAL_REFERENCE_CHARS = 64

# The names that Horizon and the system own, from
# `crates/horizon-core/src/named_secrets/name.rs`. A named secret of the fixture
# uses none of them.
RESERVED_SECRET_NAMES = frozenset({
    'PATH', 'HOME', 'USER', 'SHELL', 'TERM', 'LANG', 'DISPLAY', 'WAYLAND_DISPLAY', 'TMPDIR',
})
RESERVED_SECRET_PREFIXES = (
    'LD_', 'DYLD_', 'SSH_', 'HORIZON', 'CLAUDE', 'CODEX', 'ANTHROPIC', 'OPENAI', 'XDG_', 'DBUS_',
)
MAX_NAMED_SECRET_BYTES = 16 * 1024


def parse_named_secret(text):
    """The name and the value of one `--named-secret NAME=VALUE`, or ValueError.

    The rule is the rule of a named secret in Horizon: an environment variable name of
    1 to 64 characters of A-Z, 0-9 and _ that starts with a letter, no name that
    Horizon or the system own, and 1 to 16 KiB of text with no line break.
    """
    name, separator, value = text.partition('=')
    if not separator:
        raise ValueError(f'{text!r} is not NAME=VALUE')
    if (not 1 <= len(name) <= 64 or not name[0].isascii() or not name[0].isupper()
            or not all(character.isascii() and (character.isupper() or character.isdigit() or character == '_')
                        for character in name)):
        raise ValueError(f'{name!r} is not 1 to 64 characters of A-Z, 0-9 and _ that starts with A-Z')
    if name in RESERVED_SECRET_NAMES or name.startswith(RESERVED_SECRET_PREFIXES):
        raise ValueError(f'{name!r} belongs to Horizon or to the system')
    if not 1 <= len(value.encode()) <= MAX_NAMED_SECRET_BYTES:
        raise ValueError(f'the value of {name!r} is empty or larger than {MAX_NAMED_SECRET_BYTES} bytes')
    if '\n' in value or '\r' in value or '\0' in value:
        raise ValueError(f'the value of {name!r} holds a line break or a NUL byte')
    return name, value


def named_secret_id(name):
    """The id of one named secret in the secrets file of the fixture."""
    return f'named/{name}'


def parse_secret_row(text):
    """The id and the value of one `--secret-row ID=VALUE`, or ValueError.

    The id is a credential id of Horizon: `credential/<reference>`, where the
    reference is 1 to 64 characters of letters, digits, '.', '_' and '-'. It is not a
    named secret: use `--named-secret` for those.
    """
    object_id, separator, value = text.partition('=')
    if not separator:
        raise ValueError(f'{text!r} is not ID=VALUE')
    prefix = 'credential/'
    if not object_id.startswith(prefix):
        raise ValueError(f'{object_id!r} is not credential/<reference>')
    reference = object_id[len(prefix):]
    if (not 1 <= len(reference) <= MAX_CREDENTIAL_REFERENCE_CHARS
            or not all(character.isascii() and (character.isalnum() or character in '._-')
                       for character in reference)):
        raise ValueError(f'{reference!r} is not 1 to {MAX_CREDENTIAL_REFERENCE_CHARS} characters '
                         'of letters, digits, ".", "_" and "-"')
    if not 1 <= len(value.encode()) <= MAX_NAMED_SECRET_BYTES:
        raise ValueError(f'the value of {object_id!r} is empty or larger than {MAX_NAMED_SECRET_BYTES} bytes')
    return object_id, value


def seed_secret_rows(path, named=(), rows=()):
    """Write secret values into the secrets file of the private Horizon home.

    The file is `secrets.sqlite` of that home at 0600 in a 0700 directory, with the
    schema that Horizon's own store includes and in the rollback-journal mode that
    the store opens it with, so no `-wal` or `-shm` file stays beside it. A named
    secret is `named/<NAME>`; a credential row is a `credential/<reference>` id. A
    value of an earlier run of the same state stays unless this call names the same
    id again, so a procedure can add one secret to a private home that already holds
    another. Answers the ids that the file holds.
    """
    path = Path(path)
    values = {named_secret_id(name): value for name, value in named}
    values.update({object_id: value for object_id, value in rows})
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    # The file is private from the first byte, as the store of Horizon makes it.
    if path.exists():
        path.chmod(0o600)
    else:
        os.close(os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600))
    connection = sqlite3.connect(path)
    try:
        # A state that an earlier build left in write-ahead mode is converted here,
        # as the store converts it at its next open.
        connection.execute('PRAGMA journal_mode = DELETE')
        present = connection.execute(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'secrets'").fetchone()
        if present is None:
            connection.executescript(SECRETS_SCHEMA_FILE.read_text())
            connection.execute('INSERT INTO meta (key, value) VALUES (?, ?)',
                               ('schema_version', str(SECRETS_SCHEMA_VERSION)))
        now = int(time.time() * 1000)
        for object_id, value in values.items():
            connection.execute(
                'INSERT INTO secrets (id, body, version, updated_millis) VALUES (?, ?, 1, ?) '
                'ON CONFLICT (id) DO UPDATE SET body = excluded.body, '
                'version = secrets.version + 1, updated_millis = excluded.updated_millis',
                (object_id, value.encode(), now))
        connection.commit()
    finally:
        connection.close()
    return read_secret_ids(path)


def read_secret_ids(path):
    """The ids of the values in a secrets file, or none when the file is not there."""
    path = Path(path)
    try:
        connection = sqlite3.connect(path)
    except sqlite3.Error:
        return []
    try:
        rows = connection.execute('SELECT id FROM secrets ORDER BY id').fetchall()
    except sqlite3.Error:
        return []
    finally:
        connection.close()
    return [row[0] for row in rows]


def named_secret_names(ids):
    """The names of the named secret values of the fixture, in name order."""
    prefix = 'named/'
    return sorted(object_id[len(prefix):] for object_id in ids if object_id.startswith(prefix))

# The snake_case names of `PanelKind` (`crates/horizon-core/src/panel.rs`). A file that
# names another kind makes Horizon refuse the whole config, which starts the default
# board instead of the board the procedure tests.
PANEL_KINDS = frozenset({
    'shell', 'ssh', 'codex', 'claude', 'open_code', 'gemini', 'kilo_code', 'pi', 'grok',
    'command', 'editor', 'browser', 'device', 'setup_board',
})


def resume_error(place, resume):
    """Why a `resume` value is not one that Horizon reads, or None.

    `PanelResume` renames its unit variants to `fresh` and `last`, and its `session`
    variant keeps a session id, so only the map form `{"session": {"session_id": …}}`
    loads.
    """
    if resume is None or resume in PRESET_RESUME_MODES:
        return None
    if isinstance(resume, dict) and list(resume) == ['session']:
        session = resume['session']
        session_id = session.get('session_id') if isinstance(session, dict) else None
        if isinstance(session_id, str) and session_id:
            return None
        return f'{place}.resume.session needs a session_id'
    modes = ', '.join(PRESET_RESUME_MODES)
    return (
        f'{place}.resume must be {modes}, or '
        f'{{"session": {{"session_id": "<id>"}}}}; received {resume!r}')


def presets_resume_error(index, resume):
    """Why a `resume` value of a preset is not one that Horizon reads, or None."""
    return resume_error(f'presets[{index}]', resume)


def load_presets(path):
    """The New Workspace presets of the fixture file, checked before Horizon reads them.

    Horizon starts with its own defaults when the config does not load, which would show
    an empty board instead of the presets the procedure tests.
    """
    presets = json.loads(path.read_text())
    if not isinstance(presets, list):
        raise ValueError(f'{path}: the presets must be a JSON list of presets')
    for index, preset in enumerate(presets):
        if not isinstance(preset, dict) or not preset.get('name') or not preset.get('kind'):
            raise ValueError(f'{path}: presets[{index}] needs a name and a kind')
        problem = presets_resume_error(index, preset.get('resume'))
        if problem:
            raise ValueError(f'{path}: {problem}')
    return presets


def panel_error(index, panel):
    """Why Horizon would refuse this panel, or None.

    The fixture earns its place only when the board it intends is the board Horizon
    shows, so the checks follow `TerminalConfig` (`crates/horizon-core/src/config.rs`).
    """
    place = f'panels[{index}]'
    if not isinstance(panel, dict):
        return f'{place} must be an object'
    if not isinstance(panel.get('name'), str) or not panel.get('name'):
        return f'{place} needs a name that is a string'
    kind = panel.get('kind', 'shell')
    if kind not in PANEL_KINDS:
        return f'{place} kind {kind!r} is not a panel kind: {sorted(PANEL_KINDS)}'
    for key in ('command', 'cwd'):
        if panel.get(key) is not None and not isinstance(panel[key], str):
            return f'{place} {key} must be a string'
    args = panel.get('args', [])
    if not isinstance(args, list) or not all(isinstance(arg, str) for arg in args):
        return f'{place} args must be a list of strings'
    for key in ('position', 'size'):
        value = panel.get(key)
        if value is None:
            continue
        if (not isinstance(value, list) or len(value) != 2
                or not all(isinstance(number, (int, float)) and not isinstance(number, bool)
                           for number in value)):
            return f'{place} {key} must be two numbers'
    if 'resume' in panel:
        return resume_error(place, panel['resume'])
    return None


def load_panels(path):
    """Extra workspace panels of the fixture file, checked before Horizon reads them.

    A procedure needs a panel the fixture does not place by default, such as an agent
    panel that runs a scripted client. Horizon starts with its own defaults when the
    config does not load, so the fixture refuses the file first.
    """
    panels = json.loads(path.read_text())
    if not isinstance(panels, list):
        raise ValueError(f'{path}: the panels must be a JSON list of panels')
    for index, panel in enumerate(panels):
        problem = panel_error(index, panel)
        if problem:
            raise ValueError(f'{path}: {problem}')
    return panels


def extra_column(placed):
    """The x where a panel of the file starts, to the right of every placed panel.

    The default board is not always the two default terminals, so the column is worked
    out from the panels the fixture actually places.
    """
    right = 40
    for panel in placed:
        position, size = panel.get('position'), panel.get('size')
        if (isinstance(position, list) and isinstance(size, list)
                and len(position) == 2 and len(size) == 2):
            right = max(right, position[0] + size[0])
    return right + 40


def speech_section(recorder=None, transcriber_command=None, service=None):
    """The `speech` section of the fixture config, or `None` while a run names no speech.

    The section is off until a recorder and a transcriber are set, so a run that
    names neither writes no section and the candidate keeps its defaults. One
    transcriber wins: a service, or a command.
    """
    if not (recorder or transcriber_command or service):
        return None
    section = {}
    if recorder:
        section['recorder'] = recorder
    if service:
        section['transcriber'] = {'kind': 'service', 'endpoint': service}
    elif transcriber_command:
        section['transcriber'] = {'kind': 'command', 'command': transcriber_command}
    return section


def attention_agent(state, timer=6.0):
    """The panel entry and the program folder of the stand-in agent.

    The sidebar takes the state of an agent panel from the work ledger of its
    launch, and Horizon attaches that ledger to the panel of a Claude agent that
    starts with the default command and whose work continuation is on. So the
    panel entry of this fixture names no command, and another `claude` on the
    `PATH` of the candidate is the stand-in: Horizon resolves the name, inserts
    its own arguments after it and keeps the ledger of the launch.

    Answers the panel entry of the fixture config and the folder to put first on
    the `PATH` of the candidate.
    """
    script = (Path(__file__).resolve().parent / 'agent_attention_standin.py').resolve()
    work = Path(state) / ATTENTION_STANDIN_FOLDER
    bin_dir = work / ATTENTION_BIN_FOLDER
    bin_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    program = bin_dir / ATTENTION_PROGRAM
    if program.is_symlink() or program.exists():
        program.unlink()
    # The stand-in is the program itself: it has a `#!` line and the executable
    # mode of the file that the repository keeps.
    program.symlink_to(script)
    (bin_dir / ATTENTION_TIMER_FILE).write_text(json.dumps({'timer': timer or 0}))
    panel = json.loads(ATTENTION_PANEL_ENTRY.read_text())
    panel['cwd'] = str(work)
    return panel, bin_dir


def attention_environment(environment, bin_dir):
    """The environment of the candidate, with the stand-in first on its `PATH`."""
    environment = dict(environment)
    environment['PATH'] = f'{bin_dir}:' + environment.get('PATH', '')
    return environment


def application_document(cwd, device_address=None, firefox_system_access=False, presets=None, extra_panels=None,
                         browser_host=None, browser_host_profile=None, theme='dark', server_target=None,
                         server_command=None, server_address=None, secret_names=None, speech=None):
    """Disposable Horizon config. System access and a browser host stay off unless requested.

    The theme is part of the config, so the first start at a state path imports it. A
    later run in the same private home keeps the theme that its settings store holds.
    """
    fixture = {
        'version': 11, 'window': {'width': 1480, 'height': 900},
        'appearance': {'theme': theme},
        'workspaces': [{'name': 'Disposable VNC debug', 'cwd': cwd,
            'terminals': [
                {'name': 'Live render heartbeat', 'command': '/usr/bin/python3',
                 'args': ['-u', '-c', "import time\nprint('HORIZON DEBUG / noVNC LIVE VIEW')\nfor i in range(3600):\n print('Live frame heartbeat:', i, flush=True); time.sleep(1)"],
                 'position': [40, 60], 'size': [550, 420]},
                {'name': 'Device input test', 'command': '/bin/bash',
                 'args': ['--noprofile', '--norc'],
                 'position': [630, 60], 'size': [550, 420]}]}]}
    # A panel saves its page's downloads into the Downloads folder of its home.
    # The fixture keeps them in its own state directory instead, so a run leaves
    # nothing behind in the Downloads folder of the person.
    browser = {'downloads_root': os.path.join(cwd, 'browser-downloads')}
    if device_address:
        fixture['workspaces'][0]['terminals'] = [
            {'name': 'Native device view', 'kind': 'device', 'command': device_address,
             'position': [40, 60], 'size': [1150, 780]}]
    if firefox_system_access:
        browser.update({'backend': 'firefox', 'firefox_system_access': True})
        if not device_address:
            fixture['workspaces'][0]['terminals'] = [
                {'name': 'Google sign-in', 'kind': 'browser',
                 'command': 'https://accounts.google.com/ServiceLogin?hl=en',
                 'position': [40, 60], 'size': [700, 640]},
                {'name': 'X login', 'kind': 'browser',
                 'command': 'https://x.com/i/flow/login',
                 'position': [760, 60], 'size': [680, 640]},
                {'name': 'Live render heartbeat', 'command': '/usr/bin/python3',
                 'args': ['-u', '-c', "import time\nprint('HORIZON DEBUG / noVNC LIVE VIEW')\nfor i in range(3600):\n print('Live frame heartbeat:', i, flush=True); time.sleep(1)"],
                 'position': [40, 720], 'size': [550, 140]},
            ]
    fixture['browser'] = browser
    if browser_host:
        # A browser host that a browser panel follows, so a procedure proves the
        # panel route without a hand edit of the private settings of the fixture.
        entry = {'ssh_target': browser_host}
        if browser_host_profile:
            entry['profile'] = browser_host_profile
        fixture['browser_host'] = entry
    if server_target or server_address:
        # The server that the machine list of the fixture follows, so a procedure
        # proves two computers with one list without a hand edit of the private
        # settings. The command is only there when the default does not fit, and the
        # address is only there when the fixture reaches a listener instead of `ssh`.
        entry = {}
        if server_target:
            entry['ssh_target'] = server_target
        if server_command:
            entry['command'] = server_command
        if server_address:
            entry['address'] = server_address
        fixture['server'] = entry
    if speech:
        # The hotkey, the recorder line and the transcriber of speech input, so a
        # procedure proves a take with a stand-in recorder and a stand-in service.
        fixture['speech'] = speech
    if secret_names:
        # The named secrets that the candidate reads as the environment of an agent
        # panel. The section holds the names: the values are in the private state of
        # the fixture, at 0600, and never in its settings.
        fixture['secrets'] = {'names': list(secret_names)}
    if presets is not None:
        # The New Workspace menu of the fixture, so a procedure can test an agent that
        # has several presets without changing the real menu of the person.
        fixture['presets'] = presets
    if extra_panels:
        # Panels the fixture opens at start, so a procedure that needs an agent panel or
        # another command keeps one viewer and no manual click.
        placed = fixture['workspaces'][0]['terminals']
        column = extra_column(placed)
        for index, panel in enumerate(extra_panels):
            entry = dict(panel)
            entry.setdefault('position', [column, 60 + 500 * index])
            entry.setdefault('size', [330, 440])
            placed.append(entry)
    return fixture


def application_command(app, config, saved_session, resume_session=False, plain_start=False):
    """The command line of the candidate in the fixture.

    A saved session keeps its session file in the private home of the fixture. The
    default stays ephemeral, which writes no session file. A resumed session starts
    with no session argument, so the candidate loads the board that an earlier run
    of the same private home saved. A plain start gives the binary no argument at
    all, so it reads and writes the default paths of its own home: that is what an
    older release does, and it is how an upgrade starts.
    """
    if plain_start:
        return [str(app)]
    if resume_session:
        return [str(app), '--config', str(config)]
    session = '--new-session' if saved_session else '--ephemeral'
    return [str(app), '--config', str(config), session]


# The options that a plain start refuses: each one writes the settings or the
# session of the candidate, and a plain start proves that a home which an older
# release made keeps its own settings and sessions. The flag comes from the parser
# below, so the sentence that a procedure reads names the option that it typed.
PLAIN_START_SETTINGS_OPTIONS = (
    ('panels', '--panels'), ('attention_agent', '--attention-agent'),
    ('presets', '--presets'), ('theme', '--theme'),
    ('browser_host', '--browser-host'), ('browser_host_profile', '--browser-host-profile'),
    ('server_target', '--server-target'), ('server_command', '--server-command'),
    ('device_address', '--device-address'), ('firefox_system_access', '--firefox-system-access'),
    ('named_secret', '--named-secret'), ('secret_row', '--secret-row'),
    ('speech_recorder', '--speech-recorder'), ('speech_transcriber_command', '--speech-transcriber-command'),
    ('speech_service', '--speech-service'),
    ('saved_session', '--saved-session'), ('resume_session', '--resume-session'),
)


def plain_start_conflicts(args):
    """The options of `args` that `--plain-start` refuses, as the flags that wrote them."""
    return [flag for name, flag in PLAIN_START_SETTINGS_OPTIONS if getattr(args, name)]


def require_writable_tmp(directory=None):
    """Stop before the display starts when the host `/tmp` refuses a write.

    The fixture starts its X server on the host, and Xvfb keeps its keymap files in
    `/tmp`. A full or quota-limited `/tmp` otherwise fails inside xkbcomp with
    "Cannot close ... (not enough space?)", which reads like an X or keyboard defect.
    """
    directory = TMP_DIR if directory is None else Path(directory)
    try:
        # An unnamed file: no path stays behind, and a leftover file or symbolic
        # link under /tmp from another run or user cannot make this fail.
        with tempfile.TemporaryFile(dir=directory) as probe:
            # Larger than the keymap the X server compiles there, so a nearly full
            # filesystem fails here as well as an exhausted quota does.
            probe.write(b'\0' * (64 * 1024))
    except OSError as error:
        raise RuntimeError(
            f'{directory} refused a write ({error.strerror or error}). The fixture starts its X server '
            'on the host, and Xvfb keeps its keymap files there, so the display cannot start. '
            'Free space on that filesystem or check its per-user quota, and keep the fixture '
            'state and any frozen binary on a filesystem with space.'
        ) from None


def arguments():
    """The options of one fixture run, at their default values until a test reads them."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--horizon', type=Path, required=True, help='Horizon debug binary for an isolated smoke')
    parser.add_argument('--tools', type=Path, help='Optional unpacked Debian tools root')
    parser.add_argument('--native-view', action='store_true', help='Expose direct VNC without noVNC/websockify')
    parser.add_argument(
        '--screen', default='1600x1000',
        help='Size of the isolated desktop as WIDTHxHEIGHT, such as 2800x1000 for a wide screen')
    parser.add_argument('--device-address', help='Read-only native Device panel endpoint inside the Horizon fixture')
    parser.add_argument(
        '--firefox-system-access', action='store_true',
        help='Set browser.firefox_system_access in the fixture config. Firefox disclosure procedure only.')
    parser.add_argument('--state', type=Path, required=True, help='New private evidence directory')
    parser.add_argument(
        '--reuse-state', action='store_true',
        help='Reuse the private home and data that an earlier run left at --state, and keep them '
             'when this run stops, so a procedure can quit the candidate and start it again in '
             'the same state. Use only a state path that this person owns.')
    parser.add_argument(
        '--saved-session', action='store_true',
        help='Keep the session of the fixture in its private home')
    parser.add_argument(
        '--plain-start', action='store_true',
        help='Start the binary with no argument at all, in the private home of the state '
             'folder, so it reads and writes the default paths of that home. An older binary '
             'makes the home, and a later run with --reuse-state starts this build on it. '
             'Refuses every option that writes the settings or the session of the fixture.')
    parser.add_argument(
        '--resume-session', action='store_true',
        help='Start with no session argument, so the candidate resumes the board that an earlier '
             'run left in the private home. Use with --reuse-state and without --saved-session')
    parser.add_argument(
        '--named-secret', action='append', metavar='NAME=VALUE',
        help='Write one named secret into the secrets file of the private Horizon home of '
             'the fixture, <private home>/.horizon/secrets.sqlite, so its agent panels read '
             'NAME as an environment variable. Repeat for more than one. The value is in '
             'clear text in that file, which belongs to one private home.')
    parser.add_argument(
        '--secret-row', action='append', metavar='ID=VALUE',
        help='Write one credential value into the secrets file of the private Horizon home '
             'of the fixture, such as credential/HETZNER_API_TOKEN=a-value, so a remote '
             'browser provider of the fixture reads it. Repeat for more than one.')
    parser.add_argument(
        '--speech-recorder',
        metavar='LINE',
        help='Write speech.recorder in the fixture config, one command line with {file}, such as the '
             'stand-in recorder scripts/device-smoke/speech_standin_recorder.py. Speech input also '
             'needs --speech-service or --speech-transcriber-command.')
    parser.add_argument(
        '--speech-transcriber-command',
        metavar='LINE',
        help='Write speech.transcriber as command with this line in the fixture config, one command '
             'with {file} whose standard output is the text')
    parser.add_argument(
        '--speech-service',
        metavar='ENDPOINT',
        help='Write speech.transcriber as service with this endpoint in the fixture config, such as '
             'http://127.0.0.1:40913/v1/audio/transcriptions of the stand-in service. Its key is the '
             'named secret SPEECH_API_KEY of --named-secret.')
    parser.add_argument(
        '--presets',
        type=Path,
        help='JSON list of New Workspace presets for the fixture config, such as two presets of one agent')
    parser.add_argument(
        '--browser-host',
        help='SSH target of a browser host for the fixture, such as an SSH config alias, so the '
             'remote browser panel procedure opens its panels there')
    parser.add_argument(
        '--browser-host-profile',
        help='Profile of that browser host whose pages the fixture opens; needs --browser-host')
    parser.add_argument(
        '--server-target',
        help='SSH target of the Horizon server that the machine list of the fixture follows, such as '
             'an SSH config alias to a second computer')
    parser.add_argument(
        '--server-command',
        help='Command of the link on that server, when the default `horizon --server-link` does not '
             'fit, such as a full path; needs --server-target')
    parser.add_argument(
        '--server-address',
        help='Address of the TCP listener of a Horizon server, such as 127.0.0.1:7000, so the '
             'fixture follows that listener when its secrets file holds the token of its join')
    parser.add_argument(
        '--theme', choices=('auto', 'dark', 'light'), default=None,
        help='Set appearance.theme in the fixture config, default dark. The first start at a '
             'state path imports the value; a run with --reuse-state keeps the theme that the '
             'stored settings of that private home hold')
    parser.add_argument(
        '--attention-agent', type=float, nargs='?', const=6.0, default=None, metavar='SECONDS',
        help='Open an agent panel whose program is scripts/device-smoke/agent_attention_standin.py, '
             'so the four attention states of the sidebar show without a real agent. The panel '
             'starts as a Claude agent with work continuation, so Horizon attaches its work ledger; '
             'the script reports one event for each line that the person types, and SECONDS also '
             'makes it report the events in a sequence of its own, every SECONDS (default 6). '
             '0 waits for the typed lines alone')
    parser.add_argument(
        '--panels',
        type=Path,
        help='JSON list of extra workspace panels the fixture opens at start, such as an '
             'agent panel that runs a scripted MCP client')
    parser.add_argument(
        '--ssh-config',
        type=Path,
        help='SSH configuration of the fixture, copied to ~/.ssh/config in its private home at '
             '0600, so a host that the fixture names over SSH is reachable. Needs --ssh-identity')
    parser.add_argument(
        '--ssh-identity',
        type=Path,
        help='Private key of the fixture, copied to ~/.ssh/<its own file name> in its private '
             'home at 0600. Needs --ssh-config, which names it as IdentityFile')
    return parser


def main():
    require_writable_tmp()
    parser = arguments()
    args = parser.parse_args()
    try:
        ssh_config, ssh_identity = sandbox.ssh_sources(args.ssh_config, args.ssh_identity)
    except ValueError as problem:
        parser.error(str(problem))
    if args.browser_host_profile and not args.browser_host:
        parser.error('--browser-host-profile needs --browser-host')
    if args.server_command and not args.server_target:
        parser.error('--server-command needs --server-target')
    if args.speech_service and args.speech_transcriber_command:
        parser.error('--speech-service and --speech-transcriber-command name one transcriber; give one')
    if (args.speech_service or args.speech_transcriber_command) and not args.speech_recorder:
        parser.error('--speech-service and --speech-transcriber-command need --speech-recorder')
    if args.speech_recorder and not (args.speech_service or args.speech_transcriber_command):
        parser.error('--speech-recorder needs --speech-service or --speech-transcriber-command')
    if args.resume_session and args.saved_session:
        parser.error('--resume-session starts the saved board; do not give --saved-session as well')
    if args.plain_start:
        refused = plain_start_conflicts(args)
        if refused:
            parser.error(
                '--plain-start starts the binary with no argument, so the home of the fixture '
                'holds its own settings and its own session; remove '
                + ', '.join(refused))
    args.theme = args.theme or 'dark'
    try:
        width, height = (int(part) for part in args.screen.lower().split('x', 1))
    except ValueError:
        parser.error('--screen takes WIDTHxHEIGHT, such as 2800x1000')
    if not 640 <= width <= 8192 or not 480 <= height <= 8192:
        parser.error('--screen takes 640-8192 by 480-8192 pixels')
    args.screen = f'{width}x{height}'
    try:
        named_secrets = [parse_named_secret(text) for text in args.named_secret or []]
    except ValueError as problem:
        parser.error(f'--named-secret {problem}')
    try:
        secret_rows = [parse_secret_row(text) for text in args.secret_row or []]
    except ValueError as problem:
        parser.error(f'--secret-row {problem}')
    args.state = sandbox.prepare_state(args.state, reuse=args.reuse_state).resolve()
    app = args.horizon.resolve(strict=True)
    extra_ro_binds = [app]
    if args.tools:
        extra_ro_binds.append(args.tools.resolve())
    box = sandbox.prepare(args.state, extra_ro_binds=extra_ro_binds, reuse=args.reuse_state)
    sandbox.seed_ssh(box.private_home, ssh_config, ssh_identity)
    data = box.data
    namespace = box.namespace
    host_env = box.host_env
    env = box.sandbox_env
    if args.tools:
        root = args.tools.resolve()
        path = f'{root}/usr/bin:' + host_env.get('PATH', '')
        ld_library = f'{root}/usr/lib/x86_64-linux-gnu'
        pythonpath = f'{root}/usr/lib/python3/dist-packages'
        for child_env in (host_env, env):
            child_env['PATH'] = path
            child_env['LD_LIBRARY_PATH'] = ld_library
            child_env['PYTHONPATH'] = pythonpath
        web = root / 'usr/share/novnc'
    else:
        web = Path('/usr/share/novnc')
    for tool in ('Xvfb', 'openbox', 'x11vnc', 'bwrap', 'dbus-daemon'):
        if not shutil.which(tool, path=host_env['PATH']):
            raise SystemExit(f'Missing prerequisite: {tool}')
    if not args.native_view and not (web / 'vnc.html').is_file():
        raise SystemExit('Missing noVNC assets')
    children, logs = [], []

    def log_path(name):
        """The log file of this run: a reused state keeps the logs of the earlier runs.

        The first run writes `<name>.log`; a later run in the same state writes
        `<name>.2.log`, `<name>.3.log` and so on, so no run truncates the evidence of
        another.
        """
        path = args.state / f'{name}.log'
        run = 2
        while path.exists():
            path = args.state / f'{name}.{run}.log'
            run += 1
        return path

    def spawn(name, command, child_env=env):
        log = log_path(name).open('wb')
        logs.append(log)
        process = sandbox.start(namespace + command, env=child_env, stdout=log, stderr=log)
        children.append(process)
        return process

    def spawn_with_log(name, command, log, child_env=env):
        """Spawn with a log file that the caller keeps, such as one it reads from."""
        logs.append(log)
        process = sandbox.start(namespace + command, env=child_env, stdout=log, stderr=log)
        children.append(process)
        return process

    def stop(_signal, _frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, stop)
    try:
        # Xvfb chooses and locks an unused display atomically.
        read_fd, write_fd = os.pipe()
        log = log_path('xvfb').open('wb')
        logs.append(log)
        xvfb = sandbox.start(['Xvfb', '-displayfd', str(write_fd), '-screen', '0',
                                 f'{args.screen}x24', '-nolisten', 'tcp', '-extension', 'MIT-SHM'],
                                pass_fds=(write_fd,), stdout=log, stderr=log, env=host_env)
        children.append(xvfb)
        os.close(write_fd)
        import select
        if not select.select([read_fd], [], [], 10)[0]:
            raise RuntimeError('Xvfb startup timed out')
        with os.fdopen(read_fd) as pipe:
            number = pipe.readline().strip()
        if not number.isdigit():
            raise RuntimeError('Xvfb did not report a display')
        for child_env in (host_env, env):
            child_env['DISPLAY'] = ':' + number
            child_env['XDG_SESSION_TYPE'] = 'x11'
            child_env['LIBGL_ALWAYS_SOFTWARE'] = '1'
        spawn('dbus', sandbox.dbus_daemon_command(box.bus_address))
        sandbox.wait_for_unix_socket(box.host_socket, children[-1])
        spawn('openbox', ['openbox'])
        config = data / 'horizon.json'
        presets = load_presets(args.presets) if args.presets else None
        extra_panels = load_panels(args.panels) if args.panels else None
        if args.attention_agent is not None:
            # The stand-in replaces the default board, so the panel of the
            # procedure is the first one that the person sees.
            panel, attention_bin = attention_agent(args.state, args.attention_agent)
            extra_panels = [panel]
            # The children of this run keep this object, so the value is changed
            # here and not by a second dictionary.
            env.update(attention_environment(env, attention_bin))
        # The values of this run, and a value that an earlier run of the same private
        # home wrote, are both rows of the secrets file that the candidate reads,
        # which is the file of the Horizon home below its masked `HOME`.
        secrets_file = horizon_secrets_file(box.private_home)
        if named_secrets or secret_rows:
            seed_secret_rows(secrets_file, named_secrets, secret_rows)
        held = read_secret_ids(secrets_file) if secrets_file.is_file() else []
        speech = speech_section(args.speech_recorder, args.speech_transcriber_command, args.speech_service)
        fixture = application_document(
            str(data.resolve()), args.device_address, args.firefox_system_access, presets, extra_panels,
            args.browser_host, args.browser_host_profile, args.theme, args.server_target,
            args.server_command, args.server_address, named_secret_names(held), speech)
        if not args.plain_start:
            # A plain start keeps the home as its own settings writer: the fixture
            # writes no settings file into that home.
            config.write_text(json.dumps(fixture, indent=2))
        # All client helpers see the same masked home and private XDG paths.
        application_process = spawn('horizon', application_command(
            app, config, args.saved_session, args.resume_session, args.plain_start))
        vnc_log = log_path('vnc').open('wb')
        vnc = spawn_with_log('vnc', ['x11vnc', '-norc', '-no6', '-display', env['DISPLAY'],
                                    '-localhost', '-autoport', '40000', '-viewonly', '-forever',
                                    '-shared', '-nopw', '-noxdamage', '-noshm'], vnc_log)
        vnc_port = bound_port(vnc, Path(vnc_log.name), r'^PORT=(\d+)$')
        url = None
        if not args.native_view:
            port_file = args.state / 'web-port'
            viewer = spawn('viewer', ['/usr/bin/python3', '-c', WEB_PROXY, str(web),
                                     str(vnc_port), str(port_file)])
            web_port = bound_port(viewer, port_file, r'^(\d+)$')
            url = f'http://127.0.0.1:{web_port}/vnc.html?autoconnect=true&resize=scale&view_only=true'
        manifest = {'display': env['DISPLAY'], 'viewer_url': url,
                    'vnc_address': f'127.0.0.1:{vnc_port}', 'app': str(app),
                    'pids': [p.pid for p in children], 'fixture': 'horizon-debug',
                    'session': ('plain' if args.plain_start
                                else ('resumed' if args.resume_session
                                      else ('saved' if args.saved_session else 'ephemeral'))),
                    'session_bus': {'address': box.bus_address, 'apparmor': box.apparmor}}
        (args.state / 'target.json').write_text(json.dumps({'id': args.state.name, 'endpoint': {'kind': 'local_x11', 'display': env['DISPLAY']}}))
        (args.state / 'lab.json').write_text(json.dumps(manifest, indent=2))
        print(json.dumps(manifest), flush=True)
        while True:
            application_done = False
            for child in children:
                if child.poll() is not None:
                    if child is application_process and child.returncode == 0:
                        application_done = True
                    else:
                        raise RuntimeError(f'Lab process {child.pid} exited: {child.returncode}; inspect logs')
            if application_done:
                return
            time.sleep(0.5)
    except KeyboardInterrupt:
        pass
    finally:
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        # Expire the endpoint before its display number can be reused.
        (args.state / 'target.json').unlink(missing_ok=True)
        # Each child's whole process group, since ending bwrap alone leaves the
        # program it runs, and a bus daemon's activated services, running.
        for child in reversed(children):
            sandbox.end(child)
        for log in logs:
            log.close()
        # A reused state is what the next run starts from; a fresh one leaves nothing.
        if not args.reuse_state:
            shutil.rmtree(args.state / 'data', ignore_errors=True)


if __name__ == '__main__':
    sys.exit(main())
