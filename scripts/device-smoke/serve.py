#!/usr/bin/env python3
"""Own a disposable application/X11 lab and read-only noVNC viewer until Ctrl-C."""
import argparse
import json
import os
from pathlib import Path
import pwd
import re
import shlex
import shutil
import signal
import importlib.util
import socket
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
# The second shape of the panel entry: a preset whose own command is the stand-in
# itself, and whose work continuation is off. Horizon gives such a panel its
# arguments too, so the sidebar still follows its ledger, and no resume is offered.
ATTENTION_COMMAND_PANEL_ENTRY = Path(__file__).resolve().parent / 'attention_panel_command.json'
# The third shape: a pi panel of the same stand-in. Horizon gives a pi launch the
# report extension of its own home with `-e`, and the extension sends the events of
# the panel to the same work entry. The panel of that entry continues no work, so a
# restored panel shows no card of a continuation.
ATTENTION_PI_PANEL_ENTRY = Path(__file__).resolve().parent / 'attention_panel_pi.json'
# The fourth shape: a codex panel of the same stand-in. Codex writes its state into
# the title of its own terminal, so Horizon reads it through the reader of the panel
# and no file of a launch takes part.
ATTENTION_CODEX_PANEL_ENTRY = Path(__file__).resolve().parent / 'attention_panel_codex.json'
ATTENTION_PROGRAM = 'claude'
# The name that a pi panel of the fixture resolves on its `PATH`: the command of the
# pi preset is `pi`.
ATTENTION_PI_PROGRAM = 'pi'
# The name that a codex panel of the fixture resolves on its `PATH`: the command of
# the codex preset is `codex`.
ATTENTION_CODEX_PROGRAM = 'codex'
# The three agent kinds of the stand-in, their panel entries, the program name of
# each kind and the script that its panel runs.
ATTENTION_KINDS = ('claude', 'pi', 'codex')
ATTENTION_KIND_PROGRAM = {
    'claude': ATTENTION_PROGRAM,
    'pi': ATTENTION_PI_PROGRAM,
    'codex': ATTENTION_CODEX_PROGRAM,
}
ATTENTION_KIND_ENTRY = {
    'claude': ATTENTION_PANEL_ENTRY,
    'pi': ATTENTION_PI_PANEL_ENTRY,
    'codex': ATTENTION_CODEX_PANEL_ENTRY,
}
ATTENTION_BIN_FOLDER = 'bin'
ATTENTION_STANDIN_FOLDER = 'attention-agent'
ATTENTION_TIMER_FILE = 'stand-in.json'
ATTENTION_PANEL_NAME = 'Attention stand-in'
# The most stand-in panels of one run, and where they lie: two columns and two rows
# of the isolated desktop, so a click reaches one panel and not its neighbour.
ATTENTION_MAX_AGENTS = 4
# A run with several stand-in panels puts them in one row of the band above the
# panel of the notifications, so every panel of the procedure is visible and a click
# reaches one of them.
ATTENTION_ROW_LEFT = 20
ATTENTION_ROW_TOP = 60
ATTENTION_ROW_STEP = 360
ATTENTION_ROW_SIZE = (340, 300)
# The panel of the notification log lies below that row and below the single panel
# of a run with one stand-in panel, and inside the window of the fixture.
NOTIFICATIONS_PANEL_POSITION = (40, 560)
NOTIFICATIONS_PANEL_SIZE = (700, 320)
# The desktop notification: a stand-in program of the system that records one
# call, the file that it appends to, and the panel that follows that file.
NOTIFICATIONS_FOLDER = 'notifications'
NOTIFICATIONS_PROGRAM = 'notify-send'
NOTIFICATIONS_LOG = 'notifications.log'
NOTIFICATIONS_PANEL = 'Notifications'
# The window that takes the focus of the isolated desktop away from the candidate,
# and the title that both of its programs show.
PLAIN_WINDOW_TITLE = 'Plain window'
PLAIN_WINDOW_TK = """
import tkinter
window = tkinter.Tk()
window.title('Plain window')
tkinter.Label(window, text='A plain window takes the focus of this desktop.').pack(padx=24, pady=24)
window.geometry('420x140')
window.mainloop()
"""
# The worktree card: one shell preset for the task panel of a worktree, one browser
# preset that `open_panel` refuses, and the throwaway repository that the card
# reads, merges and removes.
WORKTREE_CARD_PRESETS = [
    {'name': 'Task shell', 'alias': 'ts', 'kind': 'shell', 'command': '/bin/sh'},
    {'name': 'Page', 'kind': 'browser'},
]
WORKTREE_CARD_FOLDER = 'repo'
WORKTREE_CARD_PRESET = 'Task shell'
WORKTREE_CARD_LAUNCHER = 'card-agent-worktree.sh'
# The file agent: the program of a panel that runs the tools of the candidate from
# the lines of a command file, and the two files that a person writes to and reads.
FILE_AGENT_PROGRAM = 'file_agent.py'
FILE_AGENT_FOLDER = 'file-agent'
FILE_AGENT_COMMANDS = 'commands.jsonl'
FILE_AGENT_ANSWERS = 'answers.jsonl'
FILE_AGENT_LAUNCHER = 'file-agent.sh'
# The programs of this folder that a panel of the fixture runs. The sandbox binds
# the private home over the home of the person, so a checkout under that home does
# not exist inside it. The state folder is bound on every run, so a panel names a
# copy of its program in the state folder and never a file of the checkout.
FIXTURE_FOLDER = Path(__file__).resolve().parent
WORKTREE_CARD_PROGRAM = 'card_agent.py'
ATTENTION_STANDIN_PROGRAM = 'agent_attention_standin.py'
# The argument that makes the stand-in of a codex panel report the program status
# protocol beside the titles of its turn.
ATTENTION_PROTOCOL_ARGUMENT = '--protocol'
# The argument that makes the same stand-in write the launch of the real program
# first: frames of the spinner while it loads, and then the plain title that says
# it is ready for the person. Every codex panel of this fixture starts with it, so
# a procedure sees that a launch claims no state before that title.
ATTENTION_STARTUP_ARGUMENT = '--startup'
ATTENTION_CODEX_STANDIN_PROGRAM = 'codex_attention_standin.py'
# The script that the panel of each agent kind of the stand-in runs. A pi launch
# receives the same five variables of a work launch, and the stand-in of the
# lifecycle events ignores the arguments of its launch, so it reports for a pi
# panel as it does for a Claude panel. A codex panel reports through the title of
# its own terminal, which no file and no argument takes part in.
ATTENTION_KIND_SCRIPT = {
    'claude': ATTENTION_STANDIN_PROGRAM,
    'pi': ATTENTION_STANDIN_PROGRAM,
    'codex': ATTENTION_CODEX_STANDIN_PROGRAM,
}
PANEL_PROGRAMS = (
    WORKTREE_CARD_PROGRAM,
    ATTENTION_STANDIN_PROGRAM,
    ATTENTION_CODEX_STANDIN_PROGRAM,
    FILE_AGENT_PROGRAM,
)
PANEL_PROGRAM_FOLDER = 'programs'


# The programs that a program of this folder imports, as one copy each: the file
# agent imports the MCP client of the card agent, so a panel that runs a copy of
# the file agent needs that second copy beside it.
PROGRAM_IMPORTS = {FILE_AGENT_PROGRAM: (WORKTREE_CARD_PROGRAM,)}


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


def check_named_secret_name(name):
    """Raise ValueError unless `name` is the name of one named secret of Horizon.

    The rule of a named secret: an environment variable name of 1 to 64 characters
    of A-Z, 0-9 and _ that starts with a letter, and no name that Horizon or the
    system own.
    """
    if (not 1 <= len(name) <= 64 or not name[0].isascii() or not name[0].isupper()
            or not all(character.isascii() and (character.isupper() or character.isdigit() or character == '_')
                        for character in name)):
        raise ValueError(f'{name!r} is not 1 to 64 characters of A-Z, 0-9 and _ that starts with A-Z')
    if name in RESERVED_SECRET_NAMES or name.startswith(RESERVED_SECRET_PREFIXES):
        raise ValueError(f'{name!r} belongs to Horizon or to the system')


def check_named_secret_value(name, value):
    """Raise ValueError unless `value` is one value of a named secret of Horizon."""
    if not 1 <= len(value.encode()) <= MAX_NAMED_SECRET_BYTES:
        raise ValueError(f'the value of {name!r} is empty or larger than {MAX_NAMED_SECRET_BYTES} bytes')
    if '\n' in value or '\r' in value or '\0' in value:
        raise ValueError(f'the value of {name!r} holds a line break or a NUL byte')


def parse_named_secret(text):
    """The name and the value of one `--named-secret NAME=VALUE`, or ValueError.

    The rule is the rule of a named secret in Horizon: an environment variable name of
    1 to 64 characters of A-Z, 0-9 and _ that starts with a letter, no name that
    Horizon or the system own, and 1 to 16 KiB of text with no line break.
    """
    name, separator, value = text.partition('=')
    if not separator:
        raise ValueError(f'{text!r} is not NAME=VALUE')
    check_named_secret_name(name)
    check_named_secret_value(name, value)
    return name, value


def named_secret_from_env(name, environ):
    """The name and the value of one `--named-secret-env NAME`, or ValueError.

    The value is the value of the environment variable NAME of the fixture, so it is
    in no argument list of the fixture and of no process that the fixture starts. A
    variable that is not set, or that is empty, is a refusal that names the variable
    and no value.
    """
    check_named_secret_name(name)
    value = environ.get(name)
    if value is None:
        raise ValueError(f'the environment variable {name} is not set')
    if not value:
        raise ValueError(f'the environment variable {name} is empty')
    check_named_secret_value(name, value)
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


def host_audio_bind(environ=None):
    """The sound socket of the person for `--host-audio`, or stop with one line.

    A run that names the host sound binds the PipeWire socket into the private
    runtime folder, so a recorder that a panel starts reaches the sound server of
    the session. Without that socket the candidate would start with speech input
    that cannot record, so the fixture stops before it creates its state and
    before it starts the candidate.
    """
    socket = sandbox.host_audio_socket(environ)
    if socket is None:
        raise SystemExit(
            f'--host-audio needs {sandbox.HOST_AUDIO_SOCKET_NAME} in the runtime folder of this '
            'computer, and it is absent: start the sound server of the session, or start the run '
            'without --host-audio')
    return socket


def worktree_card_repository(state):
    """The throwaway repository of the worktree card, with one commit and no remote.

    The run makes it under its own state folder, and the card agent works only
    inside it: a merge and a remove of a procedure touch no repository of the
    person.
    """
    repo = Path(state).resolve() / 'data' / WORKTREE_CARD_FOLDER
    if (repo / '.git').is_dir():
        return repo
    if not shutil.which('git'):
        raise SystemExit('Missing prerequisite: git')
    repo.mkdir(parents=True)
    (repo / 'app.txt').write_text('one\n')
    for arguments in (['init', '-b', 'main'], ['config', 'user.name', 'Horizon fixture'],
                      ['config', 'user.email', 'fixture@example.invalid'],
                      ['add', 'app.txt'], ['commit', '-m', 'first commit']):
        subprocess.run(['git', '-C', str(repo), *arguments], check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    return repo


def worktree_card_panel(app, repo, state, program_folder=None):
    """The panel of the scripted card agent, with the scenario of the worktree card.

    The panel runs one launcher script that the fixture writes, so Horizon's own
    launch arguments, such as `--session-id` and `--plugin-dir`, reach the script
    as `"$@"` and the card agent ignores what it does not know.
    """
    data = Path(state).resolve() / 'data'
    script = program_in_state(fixture_program(WORKTREE_CARD_PROGRAM, program_folder),
                              data / WORKTREE_CARD_PROGRAM, state)
    launcher = data / WORKTREE_CARD_LAUNCHER
    environment = (f'CARD_AGENT_REPO={shlex.quote(str(repo))} '
                   f'CARD_AGENT_STATE={shlex.quote(str(Path(state).resolve()))} '
                   f'CARD_AGENT_PRESET={shlex.quote(WORKTREE_CARD_PRESET)}')
    launcher.write_text(
        '#!/bin/sh\n'
        f'{environment} exec /usr/bin/python3 -u {shlex.quote(str(script))} '
        f'--mcp-command {shlex.quote(str(app) + " --browser-mcp")} '
        '--scenario worktree "$@"\n')
    launcher.chmod(0o700)
    return {'name': 'Worktree card agent', 'kind': 'claude', 'command': str(launcher),
            'args': [], 'cwd': str(data)}


def file_agent_files(state):
    """The command file and the answer file of the file agent, in the state folder.

    The sandbox binds the state folder at its own path, so the person writes the
    commands from outside the run and the agent writes its answers inside it. The
    command file starts empty, because this run has read no line of it: a run of a
    reused state must not repeat the commands of the earlier run. An answer file
    of an earlier run stays, and this run appends to it.
    """
    folder = Path(state).resolve() / 'data' / FILE_AGENT_FOLDER
    folder.mkdir(mode=0o700, parents=True, exist_ok=True)
    commands = folder / FILE_AGENT_COMMANDS
    commands.write_text('')
    return commands, folder / FILE_AGENT_ANSWERS


def file_agent_panel(app, state, commands, answers, program_folder=None):
    """The panel of the file agent, which runs the copies of its two programs.

    The program imports the MCP client of `card_agent.py`, so the copy of the card
    agent lies beside it in the state folder. The launcher takes Horizon's launch
    arguments as `"$@"`, as the panel of the card agent does.
    """
    data = Path(state).resolve() / 'data'
    folder = data / FILE_AGENT_FOLDER
    script = program_in_state(fixture_program(FILE_AGENT_PROGRAM, program_folder),
                              folder / FILE_AGENT_PROGRAM, state)
    program_in_state(fixture_program(WORKTREE_CARD_PROGRAM, program_folder),
                     folder / WORKTREE_CARD_PROGRAM, state)
    launcher = folder / FILE_AGENT_LAUNCHER
    launcher.write_text(
        '#!/bin/sh\n'
        f'exec /usr/bin/python3 -u {shlex.quote(str(script))} '
        f'--mcp-command {shlex.quote(str(app) + " --browser-mcp")} '
        f'--commands {shlex.quote(str(commands))} --answers {shlex.quote(str(answers))} "$@"\n')
    launcher.chmod(0o700)
    return {'name': 'File agent', 'kind': 'claude', 'command': str(launcher),
            'args': [], 'cwd': str(data)}


def file_agent_manifest(commands, answers):
    """The manifest entry that names the two files of the file agent."""
    return {'commands': str(commands), 'answers': str(answers)}


def body_in_state(destination, body, state):
    """Write `body` as the program that a panel of this fixture runs.

    The sandbox binds the private home over the home of the person, so a checkout
    under the home of the person does not exist inside the sandbox and a link to
    it points nowhere. The state folder is bound on every run, so the program of a
    panel is a copy inside it and never a link into the checkout.
    """
    state = Path(state).resolve()
    destination = Path(destination)
    folder = destination.parent.resolve()
    if folder != state and state not in folder.parents:
        raise RuntimeError(f'A program of a panel lies outside the state folder: {destination}')
    if destination.is_symlink() or destination.exists():
        destination.unlink()
    destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    destination.write_text(body)
    destination.chmod(0o700)
    return destination


def program_in_state(source, destination, state):
    """Write a copy of `source` as the program that a panel of this fixture runs."""
    return body_in_state(destination, Path(source).read_text(), state)


def fixture_program(name, folder=None):
    """The file of one program of this fixture, for `program_in_state`.

    `folder` names the folder in place of the folder of this file, so a test builds
    the panels of the fixture from a checkout under a folder of its own.
    """
    return (Path(folder) if folder is not None else FIXTURE_FOLDER) / name


def panel_programs_in_state(panels, state, folder=None):
    """Give each panel that names a program of this fixture the copy in the state folder.

    `--panels` entries come from a procedure, which names the path of the checkout.
    The sandbox binds the private home over the home of the person, so the checkout
    is not inside it and the panel would start nothing: the fixture copies the
    program into the state folder, which every run binds, and names the copy.
    """
    state = Path(state).resolve()
    copies = {}

    def copy_of(name):
        if name not in copies:
            destination = state / 'data' / PANEL_PROGRAM_FOLDER / name
            copies[name] = program_in_state(fixture_program(name, folder), destination, state)
            for imported in PROGRAM_IMPORTS.get(name, ()):
                program_in_state(fixture_program(imported, folder),
                                 destination.parent / imported, state)
        return str(copies[name])

    for panel in panels:
        texts = [text for text in [panel.get('command')] + list(panel.get('args') or [])
                 if isinstance(text, str)]
        for name in PANEL_PROGRAMS:
            source = str(fixture_program(name, folder).resolve())
            if not any(source in text for text in texts):
                continue
            replacement = copy_of(name)
            if isinstance(panel.get('command'), str):
                panel['command'] = panel['command'].replace(source, replacement)
            if panel.get('args'):
                panel['args'] = [text.replace(source, replacement) if isinstance(text, str) else text
                                 for text in panel['args']]
    return panels


def attention_agent(state, timer=6.0, program_folder=None, count=1, command=False,
                    kind='claude', protocol=False):
    """The panel entries and the program folder of the stand-in agent.

    The sidebar takes the state of an agent panel from the work ledger of its
    launch, and Horizon attaches that ledger to a Claude panel that it gives its
    own arguments to, whether the panel starts the default command or a command
    of its preset. Two shapes of the panel entry show both: one names no command,
    so another `claude` on the `PATH` of the candidate is the stand-in, and one
    names the stand-in itself as the command of its preset with the work
    continuation off.

    `count` panels of the same stand-in show a change of two panels at the same
    time, which the notification of several changes counts. Each panel of a run
    with more than one lies in its own cell of the isolated desktop and owns its
    folder, so a click reaches one of them and the two transcripts stay apart.

    `command` adds one panel of the second shape, so a procedure sees the state
    of a panel that no policy of its own supports.

    `kind` chooses the agent of the panels: `claude` attaches the plugin folder of
    the work hooks to the launch, and `pi` attaches the state report extension. Both
    report through the work entry of the candidate, and the stand-in is the program
    of the panel either way: `claude` or `pi` on the `PATH` of the candidate.

    `protocol` starts the stand-in of a codex panel with `--protocol`, so it writes
    the reports of the program status protocol beside the titles of its turn, as
    the real program does with no configuration. Every codex panel starts with
    `--startup`, so the run shows the launch of the real program: it claims no state
    of the sidebar until the launch is ready for the person.

    Answers the panel entries of the fixture config and the folder to put first on
    the `PATH` of the candidate.
    """
    if kind not in ATTENTION_KINDS:
        raise SystemExit(f'--attention-agent-kind takes one of {", ".join(ATTENTION_KINDS)}, not {kind}')
    program_name = ATTENTION_KIND_PROGRAM[kind]
    total = count + (1 if command else 0)
    if not 1 <= total <= ATTENTION_MAX_AGENTS:
        raise SystemExit(f'--attention-agent-count and --attention-agent-command take 1 to '
                         f'{ATTENTION_MAX_AGENTS} panels together, not {total}')
    script = fixture_program(ATTENTION_KIND_SCRIPT[kind], program_folder)
    work = Path(state) / ATTENTION_STANDIN_FOLDER
    bin_dir = work / ATTENTION_BIN_FOLDER
    bin_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    # The stand-in is the program itself: it has a `#!` line and the executable
    # mode of the file that the repository keeps.
    program_in_state(script, bin_dir / program_name, state)
    (bin_dir / ATTENTION_TIMER_FILE).write_text(json.dumps({'timer': timer or 0}))
    default_entry = ATTENTION_KIND_ENTRY[kind]
    entry = json.loads(default_entry.read_text())
    panels = []
    for index in range(total):
        own_command = command and index == count
        if own_command and kind != 'claude':
            # The second shape of a pi or a codex panel: the preset command is the
            # stand-in itself.
            panel = dict(entry)
            panel['name'] = f'{entry["name"]} with its own command'
        else:
            panel = json.loads(ATTENTION_COMMAND_PANEL_ENTRY.read_text()) if own_command else dict(entry)
        folder = work if total == 1 else work / f'panel-{index + 1}'
        folder.mkdir(mode=0o700, parents=True, exist_ok=True)
        panel['cwd'] = str(folder)
        if kind == 'codex':
            panel['args'] = [ATTENTION_STARTUP_ARGUMENT]
            if protocol:
                panel['args'].append(ATTENTION_PROTOCOL_ARGUMENT)
        if own_command:
            # The preset runs the stand-in itself, so Horizon gives it no name to
            # resolve on the `PATH`; the copy in the state folder is the program.
            panel['command'] = str(bin_dir / program_name)
        elif total > 1:
            panel['name'] = f'{entry["name"]} {index + 1}'
        if total > 1:
            panel['position'] = [ATTENTION_ROW_LEFT + index * ATTENTION_ROW_STEP, ATTENTION_ROW_TOP]
            panel['size'] = list(ATTENTION_ROW_SIZE)
        panels.append(panel)
    return panels, bin_dir


def path_with_first(environment, folder):
    """`environment`, with `folder` as the first folder of its `PATH`.

    Horizon finds the program of the system by the folders of the `PATH`, so only a
    folder can come first. A caller that names a file writes the path of that file
    into the `PATH`, where no program answers it: the lookup then falls through to
    the program of the computer of the person.
    """
    folder = Path(folder)
    if not folder.is_dir():
        raise RuntimeError(f'The first entry of the PATH is not a folder: {folder}')
    environment = dict(environment)
    environment['PATH'] = f'{folder}:' + environment.get('PATH', '')
    return environment


def attention_environment(environment, bin_dir):
    """The environment of the candidate, with the stand-in first on its `PATH`."""
    return path_with_first(environment, bin_dir)


def notification_standin(state):
    """The stand-in `notify-send` of this fixture and the folder to find it in.

    Horizon runs the program of the system that shows a desktop notification, and
    nobody runs that program for a test: this stand-in appends the arguments of one
    call as one line of JSON to the log of the state, and the panel "Notifications"
    follows that file. Horizon looks the program up in the folders of the `PATH`, so
    this answers the folder that holds the program and not the path of the program.
    """
    work = Path(state) / NOTIFICATIONS_FOLDER
    log = Path(state) / NOTIFICATIONS_LOG
    body = ('#!/bin/sh\n'
            'exec /usr/bin/python3 -c \'import json, sys\n'
            'with open(sys.argv[1], "a", encoding="utf-8") as log:\n'
            '    log.write(json.dumps(sys.argv[2:]) + "\\n")\' '
            f'{shlex.quote(str(log.resolve()))} "$@"\n')
    program = body_in_state(work / 'bin' / NOTIFICATIONS_PROGRAM, body, state)
    return program.parent


def notification_panel(state):
    """The shell panel that follows the log of the notifications of this run.

    The panel has its own place below the stand-in panels, so a procedure reads it
    while the panels above stay in view.
    """
    log = shlex.quote(str((Path(state) / NOTIFICATIONS_LOG).resolve()))
    return {'name': NOTIFICATIONS_PANEL, 'kind': 'shell', 'command': '/bin/sh',
            'args': ['-c', f'touch {log}; exec tail -f -n +1 {log}'],
            'position': list(NOTIFICATIONS_PANEL_POSITION), 'size': list(NOTIFICATIONS_PANEL_SIZE)}


# The folder and the program of the stand-in `ssh` of the watch of the machines, the
# private tmux server that its text reads, and the machine record that the candidate
# shows in the sidebar.
MACHINE_WATCH_FOLDER = 'machine-watch'
MACHINE_WATCH_PROGRAM = 'ssh'
MACHINE_WATCH_SOCKET = 'hz-watch'
MACHINE_WATCH_MACHINE = 'build-1'
# The field of a machine record that says the setup of the cards of that machine ran:
# without it the candidate serves no card socket for its sessions. It is the field
# that `crates/horizon-core/src/machines/record.rs` reads, so a rename there is a
# record that no card of the fixture can reach.
MACHINE_RECORD_CARDS = 'cards'
# The folder of the card sockets of the machine, which the sandbox sees as `/tmp`.
HORIZON_CARDS_FOLDER = 'horizon-cards'
# The two agents of that server: the session, the folder of its pane, which names the
# agent, and the words that the stand-in of a codex panel receives.
MACHINE_WATCH_AGENTS = (
    ('hz-shop', 'shop', ('start', 'ask')),
    ('hz-notes', 'notes', ('start',)),
)
# The session that draws a card through the card socket of that session, and the
# program of its agent, which is the client of `machine_card_agent.py`.
MACHINE_CARD_SESSION = 'hz-night'
MACHINE_CARD_AGENT = 'machine_card_agent.py'
# The program of the stand-in forward of one machine: `sshd` binds the path of each
# `-R` on the machine and copies every connection of it to the path here. The
# stand-in names the same pairs, so a card of that session reaches Horizon.
MACHINE_CARD_BRIDGE = 'card-bridge.py'
# The provider line of each machine: the stand-in of the watch, or the real sshd of
# `--machine-sshd`.
MACHINE_WATCH_PROVIDER = 'Stand-in, this fixture'
MACHINE_SSHD_PROVIDER = 'A real sshd, this fixture'
# The relay program of the machines skill: the fixture runs one session through it, so a
# person sees a card that came the way a real agent program's card comes.
MACHINE_RELAY_PROGRAM = 'relay.py'
# The folder of the relay of a machine: the setup of a machine writes the program into
# `~/.horizon-cards/` of its home, and a run with `--machine-sshd` writes it there too.
MACHINE_RELAY_FOLDER = '.horizon-cards'
MACHINE_RELAY_SESSION = 'hz-relay'
MACHINE_RELAY_TITLE = 'Night shift by relay'
MACHINE_RELAY_SKILL = 'assets/plugins/claude-code/skills/horizon-machines/SKILL.md'
# The private machine of `--machine-sshd`: one real `sshd` of the user of this run,
# inside a namespace of its own, so the machine has its own `/tmp` and its own home.
# A run with it puts no stand-in on the `PATH`: the candidate uses the real `ssh` of
# this computer for the watch, for the card sockets, for the removal and for a session.
MACHINE_SSHD_FOLDER = 'machine-sshd'
MACHINE_SSHD_CONFIG = 'sshd_config'
MACHINE_SSHD_SSH_CONFIG = 'ssh_config'
MACHINE_SSHD_LOG = 'sshd.log'
MACHINE_SSHD_PID = 'sshd.pid'
MACHINE_SSHD_HOST_KEY = 'host_key'
MACHINE_SSHD_IDENTITY = 'fixture_key'
MACHINE_SSHD_KNOWN_HOSTS = 'known_hosts'
MACHINE_SSHD_AUTHORIZED = 'authorized_keys'
MACHINE_SSHD_HOME = 'home'
MACHINE_SSHD_TMP = 'tmp'
# The session programs of a split `sshd`, which a build puts beside the program.
MACHINE_SSHD_PROGRAMS = ('sshd-session', 'sshd-auth')
# The folder of the card sockets as the machine sees it: its own `/tmp`.
MACHINE_CARDS_DIR = f'/tmp/{HORIZON_CARDS_FOLDER}'
# What the end of a run with `--machine-sshd` ends, in order: the sshd of the machine,
# and then the tmux server of its sessions, which `new-session -d` leaves behind it.
MACHINE_SSHD_STOP = ('sshd', 'tmux')
MACHINE_CARD_BRIDGE_BODY = """#!/usr/bin/env python3
import os, socket, sys, threading

def pump(source, target):
    while True:
        data = source.recv(65536)
        if not data:
            break
        target.sendall(data)
    try:
        target.shutdown(socket.SHUT_WR)
    except OSError:
        pass

def serve(remote, local):
    os.makedirs(os.path.dirname(remote), mode=0o700, exist_ok=True)
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    # sshd with its default StreamLocalBindUnlink=no: a path that a file still holds is
    # not bound, and the socket file of the child before this one stays there. Horizon
    # removes that file before it starts the child again, and this stand-in proves it:
    # a restart of the child binds only after that removal.
    try:
        listener.bind(remote)
    except OSError as error:
        print(f'card-bridge: {remote} is taken ({error.strerror})', flush=True)
        return
    listener.listen(4)
    print(f'card-bridge: {remote} -> {local}', flush=True)
    while True:
        held, _ = listener.accept()
        near = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            near.connect(local)
        except OSError:
            held.close()
            continue
        for source, target in ((held, near), (near, held)):
            threading.Thread(target=pump, args=(source, target), daemon=True).start()

pairs = [argument.split(':', 1) for argument in sys.argv[1:]]
for remote, local in pairs:
    threading.Thread(target=serve, args=(remote, local), daemon=True).start()
threading.Event().wait()
"""


def machine_watch_environment(private_tmp):
    """The environment of a tmux command of this run, outside the sandbox.

    tmux places its socket in the folder that `TMUX_TMPDIR` names and does not read
    `TMPDIR`, so this names the folder that the sandbox sees as `/tmp`.
    """
    return dict(os.environ, TMUX_TMPDIR=str(private_tmp))


def machine_watch_stop(private_tmp):
    """Stop the private tmux server of this run, which is no child of the run."""
    subprocess.run(['tmux', '-L', MACHINE_WATCH_SOCKET, 'kill-server'],
                   env=machine_watch_environment(private_tmp), check=False,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def machine_watch_server(state, private_tmp, python):
    """A private tmux server with two Codex stand-ins, and the text of the watch.

    The candidate runs the text of the watch through the stand-in `ssh` of
    [`machine_watch_standin`], which names this server by `-L`. The client of the
    sandbox reads that name under `/tmp`, which the namespace of the fixture binds
    to its private folder, so the server starts with [`machine_watch_environment`].

    The two sessions show the two states of a row: the first one asks for an
    approval in a turn, which waits for the person, and the second one works
    through a turn of the stand-in. A third session runs an agent that draws a card
    of its own through the card socket of that session.
    """
    work = Path(state) / MACHINE_WATCH_FOLDER
    work.mkdir(mode=0o700, parents=True, exist_ok=True)
    program = program_in_state(fixture_program(ATTENTION_KIND_SCRIPT['codex']),
                               work / 'bin' / ATTENTION_CODEX_PROGRAM, state)
    environment = machine_watch_environment(private_tmp)
    for session, folder, words in MACHINE_WATCH_AGENTS:
        pane = work / folder
        pane.mkdir(mode=0o700, exist_ok=True)
        # A word that starts with two dashes is an argument of the stand-in. Every
        # other word is a line that a person types into it: tmux reads a word with
        # a dash as a flag of `send-keys`, so the line goes in as literal text.
        arguments = [word for word in words if word.startswith('--')]
        command = ' '.join(shlex.quote(part) for part in (python, str(program), *arguments))
        subprocess.run(['tmux', '-L', MACHINE_WATCH_SOCKET, '-f', '/dev/null', 'new-session', '-d',
                        '-s', session, '-c', str(pane), '-x', '100', '-y', '20', command],
                       env=environment, check=True)
        for word in words:
            if word.startswith('--'):
                continue
            subprocess.run(['tmux', '-L', MACHINE_WATCH_SOCKET, 'send-keys', '-t', session, '-l', word],
                           env=environment, check=True)
            subprocess.run(['tmux', '-L', MACHINE_WATCH_SOCKET, 'send-keys', '-t', session, 'Enter'],
                           env=environment, check=True)
    machine_watch_card_agent(state, work, python, private_tmp)
    machine_watch_relay_session(state, work, python, private_tmp)
    return program


def machine_watch_card_agent(state, work, python, private_tmp):
    """One more session of the private server: an agent that draws a card of its own.

    The session runs `machine_card_agent.py`, which speaks MCP to the socket of this
    session. `sshd` binds that path on a machine; here the stand-in `ssh` of
    [`machine_watch_standin`] binds it, so the card of this session reaches the
    workspace of the machine while no terminal of the session is open.

    The session reports `idle` in the pane option `@horizon_state`, as the hook of an
    agent of a machine does, because the watch keeps a row for a pane whose program is
    no agent program only while that option holds a word.
    """
    environment = machine_watch_environment(private_tmp)
    agent = program_in_state(fixture_program(MACHINE_CARD_AGENT), work / 'bin' / MACHINE_CARD_AGENT, state)
    socket_path = machine_card_socket(private_tmp, MACHINE_CARD_SESSION)
    pane = work / 'night'
    pane.mkdir(mode=0o700, exist_ok=True)
    tmux = ['tmux', '-L', MACHINE_WATCH_SOCKET, '-f', '/dev/null']
    subprocess.run([*tmux, 'new-session', '-d', '-s', MACHINE_CARD_SESSION, '-c', str(pane),
                    '-x', '100', '-y', '20', python, '-u', str(agent), '--socket', str(socket_path)],
                   env=environment, check=True)
    reported = subprocess.run([*tmux, 'list-panes', '-t', MACHINE_CARD_SESSION, '-F', '#{pane_id}'],
                              env=environment, check=True, capture_output=True, text=True)
    pane_id = reported.stdout.strip().splitlines()[0]
    subprocess.run([*tmux, 'set-option', '-p', '-t', pane_id, '@horizon_state', 'idle'],
                   env=environment, check=True)
    return socket_path


def machine_relay_text():
    """The relay program of the machines skill: the python block of its card section.

    The fixture runs the text that the skill ships, so the relay of a card of this
    fixture is the relay that a person writes on their own machine.
    """
    skill = FIXTURE_FOLDER.parents[1] / MACHINE_RELAY_SKILL
    for block in skill.read_text(encoding='utf-8').split('```python\n')[1:]:
        body = block.split('\n```\n', 1)[0]
        if body.startswith('#!/usr/bin/env python3') and 'HORIZON_CARDS_DIR' in body:
            return body + '\n'
    raise SystemExit(f'{skill} holds no relay program')


def machine_relay_program(state):
    """The relay program in the state folder, which the relay session of this fixture runs."""
    work = Path(state) / MACHINE_WATCH_FOLDER
    return body_in_state(work / 'bin' / MACHINE_RELAY_PROGRAM, machine_relay_text(), state)


def machine_watch_relay_session(state, work, python, private_tmp):
    """One more session of the private server: the shipped relay program of the skill.

    The session runs the stand-in agent with `--relay`, so the agent speaks MCP to the
    standard input and output of the relay program that the setup of a machine writes.
    The relay learns its own session from `tmux`, meets the socket of that session, and
    the card reaches the workspace of the machine the way a real agent program's card
    does. `HORIZON_CARDS_DIR` names the folder that the sandbox sees as `/tmp`.
    """
    environment = machine_watch_environment(private_tmp)
    cards = str((Path(private_tmp) / HORIZON_CARDS_FOLDER).resolve())
    agent = program_in_state(fixture_program(MACHINE_CARD_AGENT), work / 'bin' / MACHINE_CARD_AGENT, state)
    relay = machine_relay_program(state)
    pane = work / 'relay'
    pane.mkdir(mode=0o700, exist_ok=True)
    tmux = ['tmux', '-L', MACHINE_WATCH_SOCKET, '-f', '/dev/null']
    # A pane takes the environment of the tmux SERVER, which runs already, and not the
    # one of this client: the folder of the cards reaches the relay through `env`.
    command = ['env', f'HORIZON_CARDS_DIR={cards}', python, '-u', str(agent), '--relay', str(relay),
               MACHINE_WATCH_MACHINE, '--title', MACHINE_RELAY_TITLE]
    subprocess.run([*tmux, 'new-session', '-d', '-s', MACHINE_RELAY_SESSION, '-c', str(pane),
                    '-x', '100', '-y', '20', *command], env=environment, check=True)
    return command


def machine_card_socket(private_tmp, session):
    """The path of the card socket of `session` of the machine of this fixture.

    Horizon forwards `/tmp/horizon-cards/<machine>/<session>.sock` of the machine,
    and the namespace of the fixture binds `/tmp` to its private folder, so the agent
    of the session reaches the same path from outside that namespace.
    """
    return Path(private_tmp).resolve() / HORIZON_CARDS_FOLDER / MACHINE_WATCH_MACHINE / f'{session}.sock'


def machine_watch_standin(state):
    """The stand-in `ssh` of the watch, and the folder to find it in.

    The candidate starts `ssh` for each machine of its list. The watch of the agents
    asks for the one word `sh` and writes its text to the input of that call, because
    `sshd` runs a remote command with the login shell of the user and a login shell
    that is not POSIX reads no POSIX text. A panel that opens an agent asks for a
    command of several words. This stand-in runs the word or the command with a
    `tmux` that names the private server of the fixture, as the `tmux` of a machine
    names the server of its user, so a procedure sees and opens the agents of a
    machine without a machine. It answers no other call: a run with this stand-in
    first on the `PATH` reads no machine of the person.
    """
    work = Path(state) / MACHINE_WATCH_FOLDER
    tmux = shutil.which('tmux')
    if tmux is None:
        raise SystemExit('--machine-watch needs tmux on this computer')
    remote = body_in_state(work / 'remote-bin' / 'tmux',
                           '#!/bin/sh\n'
                           f'exec {shlex.quote(tmux)} -L {shlex.quote(MACHINE_WATCH_SOCKET)} '
                           '-f /dev/null "$@"\n', state)
    bridge = body_in_state(work / 'bin' / MACHINE_CARD_BRIDGE, MACHINE_CARD_BRIDGE_BODY, state)
    bridge.chmod(0o700)
    body = ('#!/bin/sh\n'
            f'PATH={shlex.quote(str(remote.parent))}:$PATH\n'
            'export PATH\n'
            'forward=\n'
            'last=\n'
            'while [ $# -gt 0 ]; do\n'
            '  case $1 in\n'
            '    -R) forward="$forward $2"; shift 2; continue ;;\n'
            '    -o|-p|-i|-J) shift 2; continue ;;\n'
            '    -*) shift; continue ;;\n'
            '  esac\n'
            '  last=$1; shift\n'
            'done\n'
            '# The child of the cards of a machine forwards a socket of each session: the\n'
            '# stand-in of that forward binds the path of the machine and copies both ways.\n'
            f'[ -z "$forward" ] || exec {shlex.quote(sys.executable)} -u {shlex.quote(str(bridge))} $forward\n'
            'case $last in\n'
            '  *[!A-Za-z0-9_./-]*) exec sh -c "$last" ;;\n'
            '  *) exec "$last" ;;\n'
            'esac\n')
    program = body_in_state(work / 'bin' / MACHINE_WATCH_PROGRAM, body, state)
    return program.parent


def machine_watch_record(private_home, provider=MACHINE_WATCH_PROVIDER):
    """The machine list of the fixture: one machine whose route is the stand-in.

    The candidate reads `<home>/machines/registry.json`, and a record of a route
    that no `ssh` reaches is still one row of the sidebar: the watch of that row
    runs the stand-in and reads the server of this fixture. A run with
    `--machine-sshd` writes the same record: the route is the alias `build-1`, and
    the seeded `~/.ssh/config` of the private home sends it to the real machine.
    """
    root = Path(private_home) / HORIZON_HOME_FOLDER / 'machines'
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    record = {'name': MACHINE_WATCH_MACHINE, 'host': MACHINE_WATCH_MACHINE,
              'provider': provider, MACHINE_RECORD_CARDS: True, 'added_at_millis': 0}
    (root / 'registry.json').write_text(json.dumps({'machines': [record]}, indent=2), encoding='utf-8')
    return root / 'registry.json'


def machine_sshd_programs(sshd):
    """The session programs of a split `sshd`, in the prefix of the given program.

    OpenSSH 9.8 and later runs a session from `sshd-session` and a check of a key
    from `sshd-auth`, both in `<prefix>/lib/openssh/` of `usr/sbin/sshd`.
    """
    folder = Path(sshd).resolve().parents[1] / 'lib' / 'openssh'
    return tuple(folder / name for name in MACHINE_SSHD_PROGRAMS)


def machine_sshd_requirement(sshd, which=shutil.which):
    """One sentence for the piece of `--machine-sshd` that this computer lacks.

    A computer usually has the client of `ssh` and no server, so a procedure names
    a server that it unpacked into a folder of its own, and this run needs it with
    the two session programs beside it.
    """
    absent = [name for name in ('bwrap', 'ssh', 'ssh-keygen', 'tmux') if not which(name)]
    if absent:
        return ('--machine-sshd needs ' + ', '.join(absent)
                + ' on this computer, and this run finds no such program')
    if not Path(sshd).is_file():
        return f'--machine-sshd must name the sshd program as a file that exists: {sshd}'
    missing = [str(program) for program in machine_sshd_programs(sshd) if not program.is_file()]
    if missing:
        return ('--machine-sshd needs sshd-session and sshd-auth in the prefix of the program it'
                ' names, and these are no files: ' + ', '.join(missing))
    return None


def free_loopback_port():
    """A port of the loopback address that this run can give its sshd."""
    with socket.socket() as held:
        held.bind(('127.0.0.1', 0))
        return held.getsockname()[1]


class MachineSshd:
    """The private machine of `--machine-sshd`: folders, files and the sshd of this run.

    The machine is one real `sshd` of the user of this run, inside a namespace of its
    own: its own `/tmp`, so the sockets of the cards and the tmux server of the
    machine never touch the `/tmp` of this computer, and its own home, so no
    `~/.codex`, `~/.ssh` or tmux server of the person is read or written. The
    sessions of the machine are a tmux server inside that namespace, which the
    sshd's own login shell starts, so plain `tmux` finds them as on a real machine.
    """

    def __init__(self, state, sshd, port=None):
        self.state = Path(state).resolve()
        self.sshd = Path(sshd).resolve()
        self.session, self.auth = machine_sshd_programs(self.sshd)
        self.port = free_loopback_port() if port is None else port
        entry = pwd.getpwuid(os.getuid())
        self.user = entry.pw_name
        self.user_home = Path(entry.pw_dir)
        self.work = self.state / MACHINE_SSHD_FOLDER
        self.home = self.work / MACHINE_SSHD_HOME
        self.tmp = self.work / MACHINE_SSHD_TMP
        self.config = self.work / MACHINE_SSHD_CONFIG
        self.ssh_config = self.work / MACHINE_SSHD_SSH_CONFIG
        self.log = self.work / MACHINE_SSHD_LOG
        self.pid_file = self.work / MACHINE_SSHD_PID
        self.host_key = self.work / MACHINE_SSHD_HOST_KEY
        self.identity = self.work / MACHINE_SSHD_IDENTITY
        self.authorized_keys = self.work / MACHINE_SSHD_AUTHORIZED
        self.process = None

    def prepare(self):
        """Make the folders, the two keys and the two configurations of the machine.

        `sshd -t` reads the configuration before a run starts, so a text that a real
        `sshd` refuses ends the fixture here and not behind a window.
        """
        for folder in (self.work, self.home, self.tmp):
            folder.mkdir(mode=0o700, parents=True, exist_ok=True)
            folder.chmod(0o700)
        for key in (self.host_key, self.identity):
            if key.exists():
                continue
            subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-C', key.name,
                            '-f', str(key)], check=True)
        self.authorized_keys.write_text(self.public_key.read_text(encoding='utf-8'), encoding='utf-8')
        self.authorized_keys.chmod(0o600)
        self.config.write_text(machine_sshd_config(self), encoding='utf-8')
        self.ssh_config.write_text(machine_sshd_ssh_config(self), encoding='utf-8')
        checked = subprocess.run([str(self.sshd), '-t', '-f', str(self.config)],
                                 capture_output=True, text=True)
        if checked.returncode != 0:
            raise SystemExit(f'the sshd of the fixture refuses its own configuration: {checked.stderr.strip()}')

    @property
    def public_key(self):
        """The public half of the client key, which `ssh-keygen` writes beside it."""
        return self.identity.with_name(self.identity.name + '.pub')

    def ssh_sources(self):
        """The two files that `sandbox.seed_ssh` copies into the private home."""
        return self.ssh_config, self.identity

    def namespace(self):
        """The namespace of the machine: its own `/tmp`, and its own home.

        The whole computer is read-only and the state folder is bound at its own
        path, so the programs of the fixture are found where the fixture wrote them.
        """
        return ['bwrap', '--die-with-parent',
                '--ro-bind', '/', '/',
                '--dev-bind', '/dev', '/dev',
                '--bind', str(self.state), str(self.state),
                '--bind', str(self.tmp), '/tmp',
                '--bind', str(self.home), str(self.user_home),
                '--chdir', str(self.state), '--']

    def command(self, *words):
        """One command inside the namespace of the machine, such as its sshd."""
        return self.namespace() + list(words)

    def start(self):
        """Run the sshd of the machine and wait until it listens on the port of the run."""
        self.process = sandbox.start(
            self.command(str(self.sshd), '-D', '-f', str(self.config), '-E', str(self.log)),
            env=machine_sshd_environment(),
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        listened = bound_port(self.process, self.log, r'^Server listening on .* port (\d+)')
        if listened != self.port:
            raise SystemExit(f'the sshd of the machine listens on {listened} and not on {self.port}')

    def describe(self):
        """What a procedure reads about the machine of this run, and what its end ends."""
        return {'sshd': str(self.sshd), 'port': self.port, 'log': str(self.log),
                'home': str(self.home), 'tmp': str(self.tmp), 'stop': list(MACHINE_SSHD_STOP)}


def machine_sshd_config(machine):
    """The configuration of the sshd of the private machine.

    `StrictModes no`, because the state folder has none of the parents that `sshd`
    asks of the home of a person. `StreamLocalBindUnlink no` is the default of
    `sshd`: the socket file of a card child that ended stays there, and a new child
    binds only after the removal of Horizon. `SetEnv` gives every session the folder
    of the card sockets, which is the `/tmp` of the machine itself.
    """
    return (f'Port {machine.port}\n'
            'ListenAddress 127.0.0.1\n'
            f'HostKey {machine.host_key}\n'
            f'AuthorizedKeysFile {machine.authorized_keys}\n'
            f'PidFile {machine.pid_file}\n'
            'UsePAM no\n'
            'PasswordAuthentication no\n'
            'KbdInteractiveAuthentication no\n'
            'StrictModes no\n'
            'AllowStreamLocalForwarding yes\n'
            'StreamLocalBindUnlink no\n'
            f'SshdSessionPath {machine.session}\n'
            f'SshdAuthPath {machine.auth}\n'
            f'SetEnv HORIZON_CARDS_DIR={MACHINE_CARDS_DIR}\n'
            'LogLevel VERBOSE\n')


def machine_sshd_ssh_config(machine):
    """The `ssh` configuration that sends the alias of the record to the machine.

    It lands in `~/.ssh/config` of the private home of the candidate, as the file of
    `--ssh-config` does, and the identity lands beside it: the real `ssh` of this
    computer then reaches the machine with the record alone. The known-hosts file
    lies in the state, so no run writes the key of its own machine into the
    `known_hosts` of the person.
    """
    return (f'Host {MACHINE_WATCH_MACHINE}\n'
            '  HostName 127.0.0.1\n'
            f'  Port {machine.port}\n'
            f'  User {machine.user}\n'
            f'  IdentityFile ~/.ssh/{MACHINE_SSHD_IDENTITY}\n'
            '  IdentitiesOnly yes\n'
            f'  UserKnownHostsFile {machine.work / MACHINE_SSHD_KNOWN_HOSTS}\n'
            '  StrictHostKeyChecking accept-new\n')


def machine_sshd_environment(environ=None):
    """The environment of the private machine: no `TMUX` of the shell that started it.

    A fixture that a person starts inside tmux would otherwise hand the machine the
    socket of the server of that person.
    """
    return {key: value for key, value in
            sandbox.without_inherited_horizon(os.environ if environ is None else environ).items()
            if not key.startswith('TMUX')}


def machine_sshd_ssh(box, command):
    """One command of the login shell of the machine, from the sandbox of the run.

    The client is the real `ssh` of this computer and the private home of the
    fixture is its home, so it reads the configuration that the candidate reads.
    """
    result = subprocess.run([*box.namespace, 'ssh', '-o', 'BatchMode=yes', MACHINE_WATCH_MACHINE, command],
                            capture_output=True, text=True, errors='replace')
    if result.returncode != 0:
        raise SystemExit(f'the machine of --machine-sshd did not run `{command}`: '
                         f'{result.returncode} {result.stderr.strip()}')
    return result.stdout


def machine_sshd_tmux(*words):
    """One `tmux` command of the machine, in the words that its login shell reads."""
    return ' '.join(['tmux -f /dev/null', *(shlex.quote(word) for word in words)])


def machine_sshd_sessions(machine, box, python):
    """The sessions of the machine, in a tmux server that the sshd itself started.

    The two Codex stand-ins of the watch, one agent that draws a card through the
    card socket of its session, and one agent that reaches it through the relay
    program of the machines skill at `~/.horizon-cards/relay.py` of the home of the
    machine. Every command names plain `tmux`: the `/tmp` of the namespace is the
    `/tmp` of the machine, so this server is the server of that machine.
    """
    work = machine.state / MACHINE_WATCH_FOLDER
    codex = work / 'bin' / ATTENTION_CODEX_PROGRAM
    for session, folder, words in MACHINE_WATCH_AGENTS:
        pane = work / folder
        pane.mkdir(mode=0o700, exist_ok=True)
        arguments = [word for word in words if word.startswith('--')]
        machine_sshd_ssh(box, machine_sshd_tmux(
            'new-session', '-d', '-s', session, '-c', str(pane), python, str(codex), *arguments))
        for word in [word for word in words if not word.startswith('--')]:
            machine_sshd_ssh(box, machine_sshd_tmux('send-keys', '-t', session, '-l', word))
            machine_sshd_ssh(box, machine_sshd_tmux('send-keys', '-t', session, 'Enter'))
    agent = work / 'bin' / MACHINE_CARD_AGENT
    socket_path = f'{MACHINE_CARDS_DIR}/{MACHINE_WATCH_MACHINE}/{MACHINE_CARD_SESSION}.sock'
    machine_sshd_ssh(box, machine_sshd_tmux(
        'new-session', '-d', '-s', MACHINE_CARD_SESSION, '-c', str(work / 'night'),
        python, '-u', str(agent), '--socket', socket_path))
    pane_id = machine_sshd_ssh(box, machine_sshd_tmux(
        'list-panes', '-t', MACHINE_CARD_SESSION, '-F', '#{pane_id}')).strip().splitlines()[0]
    machine_sshd_ssh(box, machine_sshd_tmux('set-option', '-p', '-t', pane_id, '@horizon_state', 'idle'))
    relay = machine.user_home / MACHINE_RELAY_FOLDER / MACHINE_RELAY_PROGRAM
    machine_sshd_ssh(box, machine_sshd_tmux(
        'new-session', '-d', '-s', MACHINE_RELAY_SESSION, '-c', str(work / 'relay'),
        python, '-u', str(agent), '--relay', str(relay), MACHINE_WATCH_MACHINE,
        '--title', MACHINE_RELAY_TITLE))


def machine_sshd_start(machine, box, python):
    """Start the machine of the fixture, and the sessions of its user."""
    work = machine.state / MACHINE_WATCH_FOLDER
    program_in_state(fixture_program(ATTENTION_KIND_SCRIPT['codex']),
                     work / 'bin' / ATTENTION_CODEX_PROGRAM, machine.state)
    program_in_state(fixture_program(MACHINE_CARD_AGENT), work / 'bin' / MACHINE_CARD_AGENT, machine.state)
    # The file is written at its place in the state: the namespace of the machine binds
    # that folder over the home of the user, where the session reads it.
    body_in_state(machine.home / MACHINE_RELAY_FOLDER / MACHINE_RELAY_PROGRAM,
                  machine_relay_text(), machine.state)
    machine.start()
    machine_sshd_sessions(machine, box, python)


def machine_sshd_stop(machine, end=sandbox.end, run=subprocess.run, environment=None):
    """End the processes of the stop list of the private machine, in its order.

    The tmux server of the machine is no child of the run after `new-session -d`, so
    the end of the sshd alone leaves it behind.
    """
    if machine.process is None:
        return []
    ended = []
    for step in MACHINE_SSHD_STOP:
        if step == 'sshd':
            end(machine.process)
        else:
            run(machine.command('tmux', 'kill-server'), check=False,
                env=machine_sshd_environment() if environment is None else environment,
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        ended.append(step)
    return ended


def plain_window_command(which=shutil.which, python=sys.executable, has_tk=None):
    """The command of a plain window on the isolated desktop.

    A person who works in another window does not see Horizon, so a second plain
    window of this desktop takes the focus of the desktop away from the candidate.
    `xterm` is that window when this computer has it, and `xmessage` and a window
    of Tk are the fallbacks. A computer with none of the three answers the reason.
    """
    if which('xterm'):
        return ['xterm', '-title', PLAIN_WINDOW_TITLE]
    if which('xmessage'):
        return ['xmessage', '-title', PLAIN_WINDOW_TITLE, '-buttons', 'OK', 'A plain window takes the focus.']
    tk = has_tk if has_tk is not None else (importlib.util.find_spec('tkinter') is not None)
    if tk and python:
        return [python, '-c', PLAIN_WINDOW_TK]
    raise SystemExit('Missing prerequisite for --plain-window: xterm, xmessage or a window of Tk')


def application_document(cwd, device_address=None, firefox_system_access=False, presets=None, extra_panels=None,
                         browser_host=None, browser_host_profile=None, browser_host_engine=None,
                         browser_host_command=None, theme='dark', server_target=None,
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
        if browser_host_engine:
            entry['engine'] = browser_host_engine
        if browser_host_command:
            entry['command'] = browser_host_command
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
    ('notifications_log', '--notifications-log'),
    ('worktree_card', '--worktree-card'),
    ('file_agent', '--file-agent'),
    ('presets', '--presets'), ('theme', '--theme'),
    ('browser_host', '--browser-host'), ('browser_host_profile', '--browser-host-profile'),
    ('browser_host_engine', '--browser-host-engine'),
    ('browser_host_command', '--browser-host-command'),
    ('server_target', '--server-target'), ('server_command', '--server-command'),
    ('device_address', '--device-address'), ('firefox_system_access', '--firefox-system-access'),
    ('named_secret', '--named-secret'), ('named_secret_env', '--named-secret-env'),
    ('secret_row', '--secret-row'),
    ('speech_recorder', '--speech-recorder'), ('speech_transcriber_command', '--speech-transcriber-command'),
    ('speech_service', '--speech-service'),
    ('saved_session', '--saved-session'), ('resume_session', '--resume-session'),
)


def check_browser_host_arguments(parser, args):
    """The options of a browser host name one host: a profile, an engine and a command need it."""
    for value, flag in ((args.browser_host_profile, '--browser-host-profile'),
                        (args.browser_host_engine, '--browser-host-engine'),
                        (args.browser_host_command, '--browser-host-command')):
        if value and not args.browser_host:
            parser.error(f'{flag} needs --browser-host')


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
        '--named-secret-env', action='append', metavar='NAME',
        help='Write one named secret into the secrets file of the private Horizon home of '
             'this run, with the value of the environment variable NAME of this fixture. Use '
             'it for a secret, because the value is in no argument of a process. Repeat for '
             'more than one. The fixture refuses a NAME that is not set or empty.')
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
        '--host-audio', action='store_true',
        help='Bind the PipeWire socket of this computer into the private runtime folder of the '
             'fixture, so a recorder that a panel starts reaches the sound server of the session. '
             'It binds that one socket and nothing else of the folder. Without it the sandbox '
             'keeps its own runtime folder, which holds no sound socket')
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
        '--browser-host-engine', choices=('chromium', 'firefox'),
        help='Engine of that browser host, when the procedure follows its Firefox instead of its '
             'Chromium; needs --browser-host')
    parser.add_argument(
        '--browser-host-command',
        help='Command that starts the driver of that browser host, when its horizon is not on the '
             'PATH of a non-interactive ssh, such as a full path; needs --browser-host')
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
        '--attention-agent-command', action='store_true',
        help='Open one more panel of the stand-in whose preset has a command of its own: the '
             'stand-in program itself, with the work continuation off. Horizon attaches the work '
             'ledger of that panel too, so the sidebar shows its state while no resume is offered. '
             'Needs --attention-agent, and both flags open at most four panels')
    parser.add_argument(
        '--attention-agent-protocol', action='store_true',
        help='Start the stand-in of a codex panel with --protocol, so it writes the reports of '
             'the program status protocol beside the titles of its turn, as the real program '
             'does. Needs --attention-agent with the codex kind')
    parser.add_argument(
        '--attention-agent-kind', choices=ATTENTION_KINDS, default=ATTENTION_KINDS[0],
        help='The agent kind of the stand-in panels: claude attaches the plugin folder of the '
             'work hooks to the launch, pi attaches the state report extension of the candidate, '
             'and codex writes the state of its turn into the title of its own terminal, which '
             'needs no argument of the launch. All three reach the same four words of the '
             'sidebar. Needs --attention-agent')
    parser.add_argument(
        '--attention-agent-count', type=int, default=None, metavar='N',
        help='Open N panels of the stand-in agent, 1 to 4, so that a change of two panels at '
             'the same time shows the count of a notification; needs --attention-agent. Each panel '
             'of a run with more than one lies in its own cell of the isolated desktop')
    parser.add_argument(
        '--machine-watch', action='store_true',
        help='Put a stand-in ssh first on the PATH of the candidate and start a private tmux '
             'server with two Codex stand-ins, behind one machine of the machine list, so the '
             'sidebar shows the agents of a machine with no machine. It answers no other ssh '
             'call of the run, so a run with it reads no machine of the person')
    parser.add_argument(
        '--machine-sshd', type=Path, metavar='SSHD',
        help='With --machine-watch: make the machine of the fixture one real sshd of this '
             'computer on a loopback port, with a home and a /tmp of its own, so the candidate '
             'uses the real ssh for the watch, the card sockets, the removal and a session, and '
             'no stand-in. SSHD is a program that this run can start as its own user; the README '
             'names the two commands that unpack one into a folder')
    parser.add_argument(
        '--notifications-log', action='store_true',
        help='Put a stand-in notify-send first on the PATH of the candidate, which appends the '
             'arguments of one call as one line of JSON to notifications.log of the state, and '
             'follow that file in a panel named Notifications. Use --plain-window to take the '
             'focus of the desktop away from Horizon')
    parser.add_argument(
        '--plain-window', action='store_true',
        help='Open a plain window of the isolated desktop, so that a person works outside '
             'Horizon: xterm when this computer has it, and xmessage or a window of Tk '
             'otherwise')
    parser.add_argument(
        '--worktree-card', action='store_true',
        help='Make the throwaway repository, the presets and the card agent panel of the '
             'worktree card procedure: one task worktree, the card of the worktrees of that '
             'repository, and its merge and remove buttons, with real git and no login')
    parser.add_argument(
        '--file-agent', action='store_true',
        help='Open a panel of scripts/device-smoke/file_agent.py, which runs the MCP tools of '
             'the candidate from the lines of a command file and appends each answer to an '
             'answer file, so a computer that sends no input stays testable. The manifest '
             'names the two files')
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
    if args.machine_sshd is not None:
        if not args.machine_watch:
            parser.error('--machine-sshd makes the machine of --machine-watch a real sshd, '
                         'which this run lacks')
        if ssh_config is not None:
            parser.error('--machine-sshd writes the SSH configuration of its own machine; do not '
                         'give --ssh-config or --ssh-identity as well')
        problem = machine_sshd_requirement(args.machine_sshd)
        if problem:
            parser.error(problem)
    check_browser_host_arguments(parser, args)
    if args.worktree_card and args.presets:
        parser.error('--worktree-card writes the presets of the worktree card; do not give '
                     '--presets as well')
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
        secrets_from_env = [named_secret_from_env(name, os.environ)
                            for name in args.named_secret_env or []]
    except ValueError as problem:
        parser.error(f'--named-secret-env {problem}')
    try:
        secret_rows = [parse_secret_row(text) for text in args.secret_row or []]
    except ValueError as problem:
        parser.error(f'--secret-row {problem}')
    host_audio = host_audio_bind() if args.host_audio else None
    args.state = sandbox.prepare_state(args.state, reuse=args.reuse_state).resolve()
    machine = None
    if args.machine_sshd is not None:
        machine = MachineSshd(args.state, args.machine_sshd)
        machine.prepare()
        ssh_config, ssh_identity = machine.ssh_sources()
    app = args.horizon.resolve(strict=True)
    extra_ro_binds = [app]
    if args.tools:
        extra_ro_binds.append(args.tools.resolve())
    box = sandbox.prepare(args.state, extra_ro_binds=extra_ro_binds, reuse=args.reuse_state,
                          host_audio=host_audio)
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
        if extra_panels:
            # A procedure names the program of its checkout, which the sandbox does
            # not hold: the panel runs the copy in the state folder instead.
            extra_panels = panel_programs_in_state(extra_panels, args.state)
        if args.worktree_card:
            # The throwaway repository comes first: the panel of the card agent reads
            # it at start, and the presets are those that its task panel names.
            repo = worktree_card_repository(args.state)
            presets = WORKTREE_CARD_PRESETS
            extra_panels = list(extra_panels or []) + [worktree_card_panel(app, repo, args.state)]
        if args.attention_agent is not None:
            # The stand-in replaces the default board, so the panels of the
            # procedure are the first ones that the person sees.
            panels, attention_bin = attention_agent(
                args.state, args.attention_agent, count=args.attention_agent_count or 1,
                command=args.attention_agent_command, kind=args.attention_agent_kind,
                protocol=args.attention_agent_protocol)
            extra_panels = panels
            # The children of this run keep this object, so the value is changed
            # here and not by a second dictionary.
            env.update(attention_environment(env, attention_bin))
        elif args.attention_agent_count is not None or args.attention_agent_command:
            raise SystemExit('--attention-agent-count and --attention-agent-command open panels of '
                             '--attention-agent, which this run lacks')
        elif args.attention_agent_kind != ATTENTION_KINDS[0]:
            raise SystemExit('--attention-agent-kind names the agent of --attention-agent, '
                             'which this run lacks')
        if args.machine_watch:
            if machine is not None:
                # The real machine of the run: no stand-in on the `PATH`, and the sessions of
                # its user in the tmux server of its own namespace.
                machine_sshd_start(machine, box, sys.executable)
                machine_watch_record(box.private_home, MACHINE_SSHD_PROVIDER)
            else:
                # The stand-in goes first on the `PATH` of the candidate, and the record of
                # the machine comes from the private home of this run.
                machine_watch_server(args.state, box.private_tmp, sys.executable)
                env.update(attention_environment(env, machine_watch_standin(args.state)))
                machine_watch_record(box.private_home)
        if args.notifications_log:
            notification_bin = notification_standin(args.state)
            env.update(attention_environment(env, notification_bin))
            extra_panels = list(extra_panels or []) + [notification_panel(args.state)]
        file_agent_files_in_run = None
        if args.file_agent:
            # A panel of the file agent, which runs the tools of the candidate from
            # the lines of a command file: the fixture writes the two files and names
            # them in the manifest, so a person drives a computer that sends no input.
            file_agent_files_in_run = file_agent_files(args.state)
            commands, answers = file_agent_files_in_run
            extra_panels = list(extra_panels or []) + [
                file_agent_panel(app, args.state, commands, answers)]
        if args.plain_window:
            spawn('plain-window', plain_window_command())
        # The values of this run, and a value that an earlier run of the same private
        # home wrote, are both rows of the secrets file that the candidate reads,
        # which is the file of the Horizon home below its masked `HOME`.
        secrets_file = horizon_secrets_file(box.private_home)
        if named_secrets or secrets_from_env or secret_rows:
            seed_secret_rows(secrets_file, named_secrets + secrets_from_env, secret_rows)
        held = read_secret_ids(secrets_file) if secrets_file.is_file() else []
        speech = speech_section(args.speech_recorder, args.speech_transcriber_command, args.speech_service)
        fixture = application_document(
            str(data.resolve()), args.device_address, args.firefox_system_access, presets, extra_panels,
            args.browser_host, args.browser_host_profile, args.browser_host_engine,
            args.browser_host_command, args.theme, args.server_target,
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
        if file_agent_files_in_run is not None:
            # The two files of the file agent, so the person reads the answers and
            # writes the commands at the paths of this run.
            manifest['file_agent'] = file_agent_manifest(*file_agent_files_in_run)
        if machine is not None:
            # What a procedure reads of the real machine, and what the end ends.
            manifest['machine_sshd'] = machine.describe()
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
        if machine is not None:
            machine_sshd_stop(machine)
        elif args.machine_watch:
            machine_watch_stop(box.private_tmp)
        # A reused state is what the next run starts from; a fresh one leaves nothing.
        if not args.reuse_state:
            shutil.rmtree(args.state / 'data', ignore_errors=True)
        elif host_audio is not None:
            # bwrap made the mount point of the sound socket in the private runtime
            # folder, which a reused state keeps. A later run without --host-audio
            # must find no name there, so only that plain file goes.
            placeholder = box.runtime / sandbox.HOST_AUDIO_SOCKET_NAME
            if placeholder.is_file() and not placeholder.is_socket():
                placeholder.unlink(missing_ok=True)


if __name__ == '__main__':
    sys.exit(main())
