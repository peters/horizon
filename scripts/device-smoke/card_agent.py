#!/usr/bin/env python3
"""A scripted stand-in agent that drives real cards in the Horizon smoke fixture.

Horizon creates a card only for an agent panel that Horizon itself launched: the
panel process gets HORIZON_BROWSER_ACTOR and HORIZON_BROWSER_HOST_INSTANCE
(crates/horizon-core/src/panel/spawn.rs, `agent_env`), and the MCP server reads
that identity from its own environment. This script is the process of such a
panel. It starts Horizon's MCP server as a child, speaks the server's
newline-delimited JSON-RPC 2.0 over the child's stdin/stdout, creates cards with
`block_panel`, and keeps one `next_click` wait on every card that it owns, which
the host answers with the click of whichever card the person pressed. An older
host that refuses a wait without a card gets one wait per card instead.

`--scenario machines` writes no card: it registers one machine, or two with
CARD_AGENT_SSH_HOST_2, with `machine_key` and `machine_set`, prints each record
and the list that `machines` answers, and stays open until its stdin closes.

`--scenario worktree` reads CARD_AGENT_REPO, CARD_AGENT_STATE and
CARD_AGENT_PRESET. It starts one task worktree with real git in the repository
that the fixture owns, opens the preset panel there with `open_panel`, prints the
three refusals of that operation, and draws one card for the worktrees of the
repository: a row for each worktree, the changes of the selected one, and the
buttons Refresh, Show changes, Merge and Remove. Every git command runs with
`-C` and an argument list and never through a shell, and the card removes only a
folder under CARD_AGENT_STATE.

The Connect button of the machine card opens a terminal panel for the person
with `open_terminal`; the SSH host comes from `CARD_AGENT_SSH_HOST` and the
one-line command from `CARD_AGENT_SSH_COMMAND`. `--scenario intruder` creates
no card: it reads the cards of the other agents in the workspace and proves
that the host refuses to change them.

Run it as the `command` of an agent-kind panel of the device smoke fixture, as
scripts/device-smoke/README.md shows. Horizon appends its own launch arguments
to the command of an agent panel, such as `--plugin-dir <path>` and
`--session-id <id>`, so the wrapper script passes `"$@"` through and
`parse_args` ignores every argument it does not know.

Standard library only, Python 3.10+, no network.

Status: Horizon reads an agent as Working when one of the bottom terminal rows
starts with a spinner glyph and holds a working word (crates/horizon-core/src/
agents.rs, `is_agent_working_line`). Every line of this script therefore starts
with `card-agent: ` and avoids the spinner glyphs, the words Working, Thinking,
Compacting, Retrying, Reusing, Summarizing and "Context overflow", and the
phrases "to interrupt", "to stop" and "to cancel". `looks_like_working_line`
below enforces that on every line this script prints.
"""
from __future__ import annotations

import argparse
import collections
import json
import os
import queue
import select
import shlex
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

PROGRAM = 'card-agent'

# Mirrors `WORKING_KEYWORDS` and `WORKING_HINTS` in crates/horizon-core/src/agents.rs.
WORKING_KEYWORDS = ('Working', 'Thinking', 'Compacting', 'Retrying', 'Reusing', 'Summarizing', 'Context overflow')
WORKING_HINTS = ('to interrupt', 'to stop', 'to cancel')

MCP_COMMAND_ENV = 'CARD_AGENT_MCP_COMMAND'
DEFAULT_CLICK_TIMEOUT_SECONDS = 60.0
MIN_CLICK_TIMEOUT_MILLIS = 1_000
MAX_CLICK_TIMEOUT_MILLIS = 3_600_000
# The Connect button of the machine card: the host to reach, and the one line to
# run there after login (SKILL.md, "Connect an SSH terminal").
SSH_HOST_ENV = 'CARD_AGENT_SSH_HOST'
SSH_COMMAND_ENV = 'CARD_AGENT_SSH_COMMAND'
# The `machines` scenario: a second host for a second record, and the user, port
# and provider line that its records carry.
SECOND_SSH_HOST_ENV = 'CARD_AGENT_SSH_HOST_2'
SSH_USER_ENV = 'CARD_AGENT_SSH_USER'
SSH_PORT_ENV = 'CARD_AGENT_SSH_PORT'
MACHINE_PROVIDER = 'Loopback, test'
MACHINE_NAMES = ('build-1', 'build-2')
TERMINAL_TITLE = 'build-1 session'
# The intruder check: wait for a card of another agent to appear in the
# workspace, then read it and try to change it.
INTRUDER_LIST_WAIT_SECONDS = 5.0
INTRUDER_LIST_ATTEMPTS = 12
INTRUDER_CLICK_TIMEOUT_MILLIS = MIN_CLICK_TIMEOUT_MILLIS
INTRUDER_DOCUMENT = {'components': [{'type': 'heading', 'text': 'Not the card of this agent'}]}
# The mock documents a click that a wake line announced: it already waits, so
# one short wait reads it instead of holding the host for a minute per card.
WAKE_READ_TIMEOUT_MILLIS = 1_000
MCP_READ_TIMEOUT_SECONDS = 30.0
# The host answers a next_click at most five seconds after its wait ends.
NEXT_CLICK_GRACE_SECONDS = 20.0
# `next_click` refusals (crates/horizon-browser-control/src/manifest/block.rs,
# `Outcome::Failed`): the wait did not apply, the caller owns no card, or the
# identity cannot reach this host at all.
NEXT_CLICK_RETRY_CODES = ('request_expired', 'host_timeout')
NEXT_CLICK_EMPTY_CODES = ('no_cards',)
# An any-card wait whose agent owns no card left answers panel_unavailable
# (SKILL.md, "Read a click"); the per-card rotation then names every card that
# went away and drops it.
NEXT_CLICK_GONE_CODES = ('panel_unavailable', 'not_block_panel')
NEXT_CLICK_FATAL_CODES = ('caller_unavailable', 'wrong_host')


class Fatal(RuntimeError):
    """A failure that ends the script with a nonzero exit code."""


class Stop(BaseException):
    """Raised from a signal handler to unwind the listening loop."""


class Eof(BaseException):
    """Raised when the panel's stdin closes, which means the panel went away."""


class McpFailure(Fatal):
    """The MCP child refused, closed, or answered something unusable."""


class McpTimeout(McpFailure):
    """No answer arrived inside this script's own read deadline."""


class Refused(Fatal):
    """A `block_panel` operation that the host answered with a stable code."""

    def __init__(self, code: str, message: str):
        super().__init__(f'block_panel answered {code}: {message}')
        self.code = code
        self.message = message


def is_spinner_glyph(character: str) -> bool:
    return '\u2800' <= character <= '\u28ff' or '\u2737' <= character <= '\u273b'


def looks_like_working_line(line: str) -> bool:
    """Whether Horizon would read this line as an agent working indicator."""
    trimmed = line.lstrip()
    if not trimmed or not is_spinner_glyph(trimmed[0]):
        return False
    return (any(word in trimmed for word in WORKING_KEYWORDS)
            or any(hint in trimmed for hint in WORKING_HINTS))


def log(message: str) -> None:
    """Print one event line, and refuse a line that would flip the agent status."""
    line = f'{PROGRAM}: {message}'
    if looks_like_working_line(line):
        raise Fatal(f'this log line would read as an agent working line: {line}')
    print(line, flush=True)


def clock() -> str:
    return time.strftime('%H:%M:%S')


def answer_code(payload: dict) -> str:
    """The one word an answer carries: its status, or the code of a refusal."""
    status = payload.get('status')
    if status == 'failed':
        return payload.get('code') or 'failed'
    return status or 'unknown'


def one_line(text) -> str:
    """One log line of a text that may hold a line break."""
    return ' '.join(str(text).split())


def answer_words(payload: dict) -> str:
    """The panel and the message of an answer, in one line for the log."""
    parts = []
    if payload.get('panel_id'):
        parts.append(f'panel {payload["panel_id"]}')
    if payload.get('message'):
        parts.append(one_line(payload['message']))
    return f' ({"; ".join(parts)})' if parts else ''


# --------------------------------------------------------------------------
# Card documents. The texts follow the approved card mockups
# (docs/design/guidelines.md).
#
# The document model (crates/horizon-browser-control/src/block/model.rs, read on
# 2026-11-10) carries `subtitle` and `badge` on a heading, `detail` on a progress
# bar, `mono` on a kv value, `tone` on a list row's second line and `selected` on
# a row of a single-selection list: that row is the list's default choice, and a
# click reports it until the person selects another row. The `rich` document
# uses those fields exactly as the mockups draw them. The `plain` document is the
# fallback for a Horizon host whose model does not have them yet: the heading
# subtitle becomes its own text, the heading badge its own badge, the progress
# label holds both parts, and no row is the default one.
# --------------------------------------------------------------------------

# Card sizes (crates/horizon-ui/src/block_widget/{metrics,size}.rs, read on
# 2026-11-10). The host opens a new card at this estimate and then clamps the
# height to 140-900 points, so a document above 900 points scrolls its body
# under a title bar and a footer that stay put. `estimated_card_height` mirrors
# the estimate, which is how the `tall` scenario proves that it overflows.
CARD_CHROME_HEIGHT = 34.0 + 2 * 8.0 + 30.0   # title bar, panel padding, footer
CARD_BODY_PADDING = 18.0 + 14.0              # above and below the body
COMPONENT_GAP = 16.0
CARD_MAX_HEIGHT = 900.0
TEXT_CHARS_PER_LINE = 58
TEXT_LINE_HEIGHT = 12.5 * 1.5
HEADING_HEIGHT = 18.0 * 1.35
HEADING_SUBTITLE_HEIGHT = 4.0 + 12.5 * 1.35
BADGE_HEIGHT = 24.0
PROGRESS_HEIGHT = 11.5 * 1.35 + 6.0 + 4.0
KV_ROW_HEIGHT = 34.0
LIST_ROW_PAD_Y = 10.0
LIST_TITLE_HEIGHT = 13.0 * 1.35
LIST_SUBTITLE_HEIGHT = 2.0 + 11.5 * 1.35
LIST_ROW_GAP = 6.0
BUTTON_HEIGHT = 36.0

MACHINE_TITLE = 'Machine \u00b7 build-1'
CHOOSE_TITLE = 'New machine'
BROWSER_TITLE = 'Browser machine'
TALL_TITLE = 'Tall card \u00b7 overflow'

# (step label, right-aligned detail, filled percent)
CHOOSE_STEPS = (
    ('Step 1 of 3', 'Provider', 33),
    ('Step 2 of 3', 'Provider chosen', 66),
    ('Step 3 of 3', 'Ready to create', 100),
)
BROWSER_STEPS = (
    ('Step 2 of 4', 'Checked the machine', 50),
    ('Step 3 of 4', 'Browser chosen', 75),
    ('Step 4 of 4', 'Running the setup', 100),
)

# (row id, title, second line, tone of the second line)
CHOOSE_ROWS = (
    ('hetzner', 'Hetzner', 'Signed in \u00b7 Helsinki, Falkenstein, Nuremberg', None),
    ('azure', 'Azure', 'Signed in \u00b7 Norway East', None),
    ('aws', 'AWS', 'Signed in \u00b7 Stockholm', None),
    ('runpod', 'RunPod', 'Signed in \u00b7 containers with a graphics card', None),
    ('gcp', 'Google Cloud', 'gcloud is not installed. I can install it.', 'warn'),
)
BROWSER_ROWS = (
    ('chrome', 'Chrome on this Mac', 'Looks like an ordinary user. Best against blocking.', None),
    ('chromium-docker', 'Chromium in Docker', 'Nothing to install. Some sites can block it here.', None),
)

# The default choice of each single-selection list: the host draws that row
# selected and reports it with the next click, so the card needs no row press
# before its button works.
CHOOSE_DEFAULT_ROW = 'hetzner'
BROWSER_DEFAULT_ROW = 'chrome'

# The tall card: more content than a card can show, so that its body scrolls
# under the title bar and the footer (see CARD_MAX_HEIGHT).
TALL_KV = (
    ('Build', '1148', False),
    ('Target', 'linux/x64', True),
    ('Started', '18:02:11', False),
    ('Elapsed', '9 min 42 s', False),
    ('Stage', 'link', False),
    ('Artifacts', '3 of 3', False),
    ('Binary', 'a41f9c2', True),
    ('Size', '48.2 MB', False),
    ('Tests', '1260 passed', False),
    ('Warnings', 'none', False),
    ('Runner', 'smoke-2', False),
    ('Upload', 'not started', False),
)
TALL_ROWS = (
    ('step.build', 'Compile the workspace', 'Release profile, all targets', None),
    ('step.test', 'Run the test suites', 'Two crates, 1260 cases', None),
    ('step.clippy', 'Check the lint gates', 'Blocking and strict tiers', None),
    ('step.fmt', 'Check the formatting', 'cargo fmt --check', None),
    ('step.package', 'Pack the archives', 'Three platforms', None),
    ('step.upload', 'Upload the artifacts', 'The release draft is open', 'warn'),
    ('step.notes', 'Write the release notes', 'Two sections, one line each', None),
    ('step.announce', 'Announce the build', 'Waits for your message', None),
)
TALL_TEXT = ('The body of this card is longer than a card can take. '
             'The title bar and the footer stay still, and only the body scrolls. '
             'Scroll the body to reach the two buttons at the end.')

# (label, value, drawn in the monospace family)
MACHINE_KV = (
    ('Type', 'cx32 \u00b7 4 vCPU \u00b7 8 GB', False),
    ('Address', '203.0.113.10', True),
    ('Session', 'tmux: build', True),
    ('Up for', '2 h 14 min', False),
)


def row_title(rows, row_id):
    """The visible title of one selectable row id, or None."""
    for candidate, title, _subtitle, _tone in rows:
        if candidate == row_id:
            return title
    return None


def heading(text, subtitle=None, badge=None, rich=True):
    """A heading with its subtitle and badge, or the plain fallback components."""
    if rich:
        component = {'type': 'heading', 'text': text}
        if subtitle:
            component['subtitle'] = subtitle
        if badge:
            component['badge'] = badge
        return [component]
    components = [{'type': 'heading', 'text': text}]
    if subtitle:
        components.append({'type': 'text', 'text': subtitle})
    if badge:
        components.append({'type': 'badge', 'text': badge['text'], 'tone': badge.get('tone', 'neutral')})
    return components


def progress(label, detail, percent, rich=True):
    """A progress bar with its right-aligned detail, or both parts in the label."""
    if rich and detail:
        return [{'type': 'progress', 'label': label, 'percent': percent, 'detail': detail}]
    return [{'type': 'progress', 'label': f'{label} \u00b7 {detail}' if detail else label,
             'percent': percent}]


def kv_rows(rows, rich=True):
    """The label and value rows, with `mono` on the values the mockup sets in mono."""
    return [{'label': label, 'value': value, **({'mono': True} if mono and rich else {})}
            for label, value, mono in rows]


def list_rows(rows, rich=True, selected=None):
    """The selectable rows, with the default row and tones the model supports."""
    built = []
    for row_id, title, subtitle, tone in rows:
        row = {'id': row_id, 'title': title, 'subtitle': subtitle}
        if tone and rich:
            row['tone'] = tone
        if selected is not None and row_id == selected and rich:
            row['selected'] = True
        built.append(row)
    return built


def machine_document(stopped: bool = False, connected_at: str | None = None, rich: bool = True,
                     terminal_note: str | None = None) -> dict:
    """The machine card of Main.dc.html, in its running or stopped state.

    `terminal_note` is the line that says what the Connect button did, and it
    replaces the older note that only names the press.
    """
    badge = {'text': 'Stopped' if stopped else 'Running', 'tone': 'warn' if stopped else 'ok'}
    components = heading('build-1', 'Hetzner \u00b7 Helsinki', badge, rich)
    components.append({'type': 'kv', 'rows': kv_rows(MACHINE_KV, rich)})
    if stopped:
        components.append({'type': 'text',
                           'text': 'The machine is stopped. Start resumes the same session.'})
    else:
        components.append({
            'type': 'text',
            'text': ('The session keeps running when you close Horizon. '
                     'Delete removes the machine and its firewall rule.')})
    if terminal_note:
        components.append({'type': 'text', 'text': terminal_note})
    elif connected_at:
        components.append({'type': 'text', 'text': f'Connect was pressed at {connected_at}'})
    components.append({'type': 'buttons', 'items': [
        {'id': 'connect', 'label': 'Connect', 'tone': 'primary'},
        {'id': 'start', 'label': 'Start'} if stopped else {'id': 'stop', 'label': 'Stop'},
        {'id': 'delete', 'label': 'Delete', 'tone': 'danger'},
    ]})
    return {'components': components}


def choose_document(chosen: str | None = None, step: int = 0, rich: bool = True) -> dict:
    """The provider card of Choose.dc.html, with the chosen provider when known."""
    label, detail, percent = CHOOSE_STEPS[min(step, len(CHOOSE_STEPS) - 1)]
    components = heading('Where should it run?', 'I checked the tools on this computer. Pick one.',
                         None, rich)
    components.append({'type': 'list', 'selection': 'single',
                       'rows': list_rows(CHOOSE_ROWS, rich, CHOOSE_DEFAULT_ROW)})
    components += progress(label, detail, percent, rich)
    if chosen:
        components.append({'type': 'text', 'text': f'You picked {chosen}.'})
    components.append({'type': 'buttons', 'items': [
        {'id': 'continue', 'label': 'Continue', 'tone': 'primary'},
        {'id': 'cancel', 'label': 'Cancel'},
    ]})
    return {'components': components}


def browser_document(chosen: str | None = None, step: int = 0, rich: bool = True) -> dict:
    """The browser machine card of BrowserSetup.dc.html."""
    label, detail, percent = BROWSER_STEPS[min(step, len(BROWSER_STEPS) - 1)]
    components = heading('home-mac', 'macOS \u00b7 arm64 \u00b7 reached over SSH',
                         {'text': 'Needs your choice', 'tone': 'info'}, rich)
    components.append({'type': 'kv', 'rows': kv_rows((
        ('Chrome', 'Installed', False),
        ('Docker', 'Running', False),
        ('Graphics card', 'Yes, but not inside Docker', False),
        ('Free disk', '212 GB', False),
    ), rich)})
    components.append({'type': 'text', 'text': 'Which browser should your panels use?'})
    components.append({'type': 'list', 'selection': 'single',
                       'rows': list_rows(BROWSER_ROWS, rich, BROWSER_DEFAULT_ROW)})
    components += progress(label, detail, percent, rich)
    if chosen:
        components.append({'type': 'text', 'text': f'You picked {chosen}. I am setting it up.'})
    components.append({'type': 'buttons', 'items': [
        {'id': 'setup', 'label': 'Set it up', 'tone': 'primary'},
        {'id': 'cancel', 'label': 'Cancel'},
    ]})
    return {'components': components}


def component_height(component: dict) -> float:
    """The height one component needs, as `component_height` in size.rs takes it."""
    kind = component.get('type')
    if kind == 'heading':
        height = HEADING_HEIGHT
        if component.get('subtitle'):
            height += HEADING_SUBTITLE_HEIGHT
        if component.get('badge'):
            height = max(height, BADGE_HEIGHT)
        return height
    if kind == 'text':
        # Rust's `div_ceil`: one line never holds more than TEXT_CHARS_PER_LINE.
        lines = sum(max(1, -(-len(line) // TEXT_CHARS_PER_LINE))
                    for line in component['text'].split('\n'))
        return max(1, lines) * TEXT_LINE_HEIGHT
    if kind == 'kv':
        return len(component['rows']) * KV_ROW_HEIGHT
    if kind == 'badge':
        return BADGE_HEIGHT
    if kind == 'progress':
        return PROGRESS_HEIGHT
    if kind == 'divider':
        return 1.0
    if kind == 'list':
        rows = component['rows']
        heights = sum(2 * LIST_ROW_PAD_Y + LIST_TITLE_HEIGHT
                      + (LIST_SUBTITLE_HEIGHT if row.get('subtitle') else 0.0)
                      for row in rows)
        return heights + max(0, len(rows) - 1) * LIST_ROW_GAP
    if kind == 'buttons':
        return BUTTON_HEIGHT if component['items'] else 0.0
    raise Fatal(f'the document holds the unknown component {kind!r}')


def estimated_card_height(document: dict) -> float:
    """The height a new card of this document needs, before the host clamps it."""
    components = document.get('components') or []
    body = sum(component_height(component) + (0.0 if index == 0 else COMPONENT_GAP)
               for index, component in enumerate(components))
    return CARD_CHROME_HEIGHT + CARD_BODY_PADDING + body


def tall_document(pressed_at: str | None = None, rich: bool = True) -> dict:
    """The tall card: a heading, 12 pairs, a text, 8 rows and two buttons."""
    components = heading('Overflow check', 'Twelve pairs, eight rows and a long body',
                         {'text': 'Tall', 'tone': 'info'}, rich)
    components.append({'type': 'kv', 'rows': kv_rows(TALL_KV, rich)})
    components.append({'type': 'text', 'text': TALL_TEXT})
    components.append({'type': 'list', 'selection': 'single', 'rows': list_rows(TALL_ROWS, rich)})
    if pressed_at:
        components.append({'type': 'text', 'text': f'OK was pressed at {pressed_at}'})
    components.append({'type': 'buttons', 'items': [
        {'id': 'ok', 'label': 'OK', 'tone': 'primary'},
        {'id': 'close', 'label': 'Close'},
    ]})
    return {'components': components}

# --------------------------------------------------------------------------
# MCP over stdio. The server frames one JSON object per line, terminated by a
# newline (crates/horizon-browser-mcp/tests/stdio_protocol.rs, and
# crates/horizon-browser-cli/tests/cli.rs -> `write_all(b"\n")`).
# --------------------------------------------------------------------------


class StdioMcpClient:
    """One MCP server child, spoken to with newline-delimited JSON-RPC 2.0."""

    def __init__(self, command: str):
        argv = shlex.split(command)
        if not argv:
            raise Fatal('--mcp-command is empty')
        self.argv = argv
        self._buffer = b''
        self._next_id = 0
        self._stderr = collections.deque(maxlen=40)
        try:
            # The identity that Horizon injected into this panel is inherited:
            # the server needs HORIZON_BROWSER_ACTOR and
            # HORIZON_BROWSER_HOST_INSTANCE to reach this host.
            self.proc = subprocess.Popen(
                argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                stderr=subprocess.PIPE, bufsize=0, close_fds=True)
        except OSError as error:
            raise Fatal(f'cannot start the MCP server {argv[0]!r}: {error}') from error
        threading.Thread(target=self._pump_stderr, daemon=True).start()

    def _pump_stderr(self) -> None:
        stream = self.proc.stderr
        if stream is None:
            return
        for raw in iter(stream.readline, b''):
            self._stderr.append(raw.decode('utf-8', 'replace').rstrip())

    def stderr_tail(self) -> str:
        return ' | '.join(line for line in list(self._stderr)[-4:] if line)

    def _closed_reason(self) -> str:
        code = self.proc.poll()
        tail = self.stderr_tail()
        reason = f'MCP server {self.proc.pid} closed its output (exit {code})'
        return f'{reason}; stderr: {tail}' if tail else reason

    def _send(self, message: dict) -> None:
        data = json.dumps(message, separators=(',', ':')).encode('utf-8') + b'\n'
        try:
            self.proc.stdin.write(data)
            self.proc.stdin.flush()
        except (BrokenPipeError, ValueError, OSError) as error:
            raise McpFailure(f'cannot write to the MCP server: {error}; {self._closed_reason()}') from error

    def _read_line(self, timeout: float) -> bytes:
        deadline = time.monotonic() + timeout
        while b'\n' not in self._buffer:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise McpTimeout(f'the MCP server sent no answer in {timeout:.0f} s')
            try:
                ready, _, _ = select.select([self.proc.stdout], [], [], remaining)
            except (OSError, ValueError) as error:
                raise McpFailure(f'cannot wait for the MCP server: {error}') from error
            if not ready:
                continue
            chunk = os.read(self.proc.stdout.fileno(), 65536)
            if not chunk:
                raise McpFailure(self._closed_reason())
            self._buffer += chunk
        line, _, self._buffer = self._buffer.partition(b'\n')
        return line

    def request(self, method: str, params: dict | None = None,
                timeout: float = MCP_READ_TIMEOUT_SECONDS) -> dict:
        self._next_id += 1
        request_id = self._next_id
        message = {'jsonrpc': '2.0', 'id': request_id, 'method': method}
        if params is not None:
            message['params'] = params
        self._send(message)
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise McpTimeout(f'{method} answered nothing in {timeout:.0f} s')
            line = self._read_line(remaining)
            if not line.strip():
                continue
            try:
                reply = json.loads(line)
            except ValueError:
                # A stray line must never end the loop silently.
                raise McpFailure(f'{method} answered something that is not JSON: {line[:200]!r}') from None
            if not isinstance(reply, dict) or reply.get('id') != request_id:
                # A notification or a server-to-client request; not our answer.
                continue
            if 'error' in reply:
                error = reply['error'] or {}
                raise McpFailure(f'{method} failed: {error.get("code")} {error.get("message")}')
            result = reply.get('result')
            return result if isinstance(result, dict) else {}

    def notify(self, method: str, params: dict | None = None) -> None:
        message = {'jsonrpc': '2.0', 'method': method}
        if params is not None:
            message['params'] = params
        self._send(message)

    def initialize(self, protocol_version: str = '2025-06-18') -> dict:
        result = self.request('initialize', {
            'protocolVersion': protocol_version,
            'capabilities': {},
            'clientInfo': {'name': 'card-agent', 'version': '1'},
        })
        self.notify('notifications/initialized')
        return result

    def tool_names(self) -> list[str]:
        result = self.request('tools/list', {})
        return [tool.get('name') for tool in result.get('tools', []) if isinstance(tool, dict)]

    def call_tool(self, name: str, arguments: dict,
                  timeout: float = MCP_READ_TIMEOUT_SECONDS) -> dict:
        result = self.request('tools/call', {'name': name, 'arguments': arguments}, timeout)
        text = ' '.join(part.get('text', '') for part in result.get('content', [])
                        if isinstance(part, dict) and part.get('type') == 'text')
        if result.get('isError'):
            raise McpFailure(f'{name} answered an error: {text.strip() or result}')
        structured = result.get('structuredContent')
        if isinstance(structured, dict):
            return structured
        if not text.strip():
            return {}
        try:
            payload = json.loads(text)
        except ValueError:
            raise McpFailure(f'{name} answered text that is not JSON: {text[:300]!r}') from None
        if not isinstance(payload, dict):
            raise McpFailure(f'{name} answered {type(payload).__name__}, not an object')
        return payload

    def close(self) -> None:
        proc = self.proc
        try:
            if proc.stdin is not None:
                proc.stdin.close()
        except OSError:
            pass
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=5)
        for stream in (proc.stdout, proc.stderr):
            try:
                if stream is not None:
                    stream.close()
            except OSError:
                pass


class StdinWatcher(threading.Thread):
    """Read the panel PTY, so a typed wake line reaches the agent loop.

    The bytes are read from the descriptor itself, not through a buffered
    reader: a daemon thread that holds a buffered stream lock while the
    interpreter finalizes makes CPython abort at shutdown, and a terminal
    driver hands the wake line over as CR (icrnl) while a pipe hands it over
    as LF.
    """

    daemon = True

    def __init__(self, descriptor: int, events: queue.Queue):
        super().__init__(name='card-agent-stdin')
        self.descriptor = descriptor
        self.events = events

    def run(self) -> None:
        pending = b''
        try:
            while True:
                chunk = os.read(self.descriptor, 4096)
                if not chunk:
                    break
                pending += chunk
                while b'\n' in pending or b'\r' in pending:
                    at = min(index for index in (pending.find(b'\n'), pending.find(b'\r')) if index >= 0)
                    line, pending = pending[:at], pending[at + 1:]
                    self.events.put(('line', line.decode('utf-8', 'replace')))
        except OSError:
            pass
        finally:
            self.events.put(('eof', None))


# --------------------------------------------------------------------------
# The card agent.
# --------------------------------------------------------------------------


class Card:
    """One card this agent owns and listens to."""

    def __init__(self, key: str, title: str, row_ids=(), steps=()):
        self.key = key
        self.title = title
        self.panel_id: str | None = None
        self.revision = 0
        self.row_ids = tuple(row_ids)
        self.steps = tuple(steps)
        self.step = 0
        self.chosen: str | None = None
        self.stopped = False
        self.connected_at: str | None = None
        self.terminal_note: str | None = None
        self.pressed_at: str | None = None

    def document(self, rich: bool) -> dict:
        raise NotImplementedError


class MachineCard(Card):
    def document(self, rich: bool) -> dict:
        return machine_document(self.stopped, self.connected_at, rich, self.terminal_note)


class ChooseCard(Card):
    def document(self, rich: bool) -> dict:
        return choose_document(self.chosen, self.step, rich)


class BrowserCard(Card):
    def document(self, rich: bool) -> dict:
        return browser_document(self.chosen, self.step, rich)


class TallCard(Card):
    def document(self, rich: bool) -> dict:
        return tall_document(self.pressed_at, rich)


# --------------------------------------------------------------------------
# The `worktree` scenario: one card for the worktrees of a real repository, and
# the task panel that `open_panel` starts in one of them
# (assets/plugins/claude-code/skills/horizon-worktree/SKILL.md).
#
# Every git command runs with `-C` and an argument list, never through a shell,
# and every path that a command removes is checked to be under the state folder
# of the fixture first.
# --------------------------------------------------------------------------

WORKTREE_REPO_ENV = 'CARD_AGENT_REPO'
WORKTREE_STATE_ENV = 'CARD_AGENT_STATE'
WORKTREE_PRESET_ENV = 'CARD_AGENT_PRESET'
DEFAULT_WORKTREE_PRESET = 'Task shell'
WORKTREE_BROWSER_PRESET = 'Page'
WORKTREE_TASK = 'first change'
WORKTREE_BRANCH_PREFIX = 'task/'
MAX_SLUG_CHARS = 40
WORKTREE_TEXT_CHARS = 512
WORKTREE_SUBJECTS = 3
WORKTREE_MAX_ROWS = 32


def is_under(root, path) -> bool:
    """Whether `path` is `root` or a child of it, after both are resolved."""
    root = Path(os.path.realpath(root))
    path = Path(os.path.realpath(path))
    return path == root or root in path.parents


def slug_of(text: str) -> str:
    """The slug of one task name: letters, digits and `-`, at most 40 characters."""
    slug = ''
    for character in text.strip().lower():
        if 'a' <= character <= 'z' or '0' <= character <= '9':
            slug += character
        elif slug and not slug.endswith('-'):
            slug += '-'
    return slug.strip('-')[:MAX_SLUG_CHARS].strip('-') or 'task'


def run_git(repo, arguments) -> subprocess.CompletedProcess:
    """One git command in `repo`: `-C`, an argument list, and no shell."""
    command = ['git', '-C', str(repo), *arguments]
    try:
        return subprocess.run(command, capture_output=True, text=True,
                              encoding='utf-8', errors='replace', check=False)
    except OSError as error:
        raise Fatal(f'cannot run {command[0]}: {error}') from error


def git_text(repo, arguments) -> str:
    """The standard output of one git command that must succeed."""
    result = run_git(repo, arguments)
    if result.returncode != 0:
        reason = (result.stderr or result.stdout).strip().splitlines()
        first = one_line(reason[0]) if reason else f'exit {result.returncode}'
        raise Fatal(f'git {arguments[0]} in {repo} failed: {first}')
    return result.stdout


def git_succeeds(repo, arguments) -> bool:
    """Whether one git command that may fail answered success."""
    return run_git(repo, arguments).returncode == 0


def git_text_or(repo, arguments, fallback: str = '') -> str:
    """The standard output of one git command, or `fallback` when it fails.

    A card reads the state of a worktree that another program may change under it,
    so a folder that went away costs one line of the card and not the card.
    """
    result = run_git(repo, arguments)
    return result.stdout if result.returncode == 0 else fallback


def count_lines(text: str) -> int:
    """The number of the lines of `text` that hold something."""
    return len([line for line in text.splitlines() if line.strip()])


def base_branch(repo) -> str:
    """The branch that a task starts from: the remote default, else this branch."""
    for arguments in (('symbolic-ref', '--short', 'refs/remotes/origin/HEAD'),
                      ('symbolic-ref', '--short', 'HEAD')):
        result = run_git(repo, arguments)
        name = result.stdout.strip() if result.returncode == 0 else ''
        if not name:
            continue
        return name.split('/', 1)[1] if name.startswith('origin/') else name
    return 'main'


def free_slug(repo, task: str) -> str:
    """The slug of the task under a branch name that this repository does not use."""
    stem = slug_of(task)
    candidate = stem
    number = 2
    while git_succeeds(repo, ('rev-parse', '--verify', '--quiet',
                              f'refs/heads/{WORKTREE_BRANCH_PREFIX}{candidate}')):
        suffix = f'-{number}'
        candidate = f'{stem[:MAX_SLUG_CHARS - len(suffix)].strip("-")}{suffix}'
        number += 1
    return candidate


def age_of(seconds: float) -> str:
    """The words of the age of one commit, such as `4 hours ago`."""
    for size, name in ((86400, 'day'), (3600, 'hour'), (60, 'minute')):
        if seconds >= size:
            count = int(seconds // size)
            return f'{count} {name}{"" if count == 1 else "s"} ago'
    return f'{int(seconds)} seconds ago'


def parse_worktrees(text: str) -> list[dict]:
    """The worktrees of `git worktree list --porcelain`, in its own order."""
    worktrees, current = [], None
    for line in text.splitlines():
        if line.startswith('worktree '):
            current = {'path': line[len('worktree '):], 'head': '', 'branch': None,
                       'detached': False}
            worktrees.append(current)
        elif current is None or not line:
            continue
        elif line.startswith('HEAD '):
            current['head'] = line[len('HEAD '):]
        elif line.startswith('branch '):
            reference = line[len('branch '):]
            current['branch'] = (reference[len('refs/heads/'):]
                                 if reference.startswith('refs/heads/') else reference)
        elif line == 'detached':
            current['detached'] = True
    return worktrees


class Worktree:
    """One worktree of the repository, with the numbers that its row shows."""

    def __init__(self, path, branch, main: bool):
        self.path = Path(path)
        self.branch = branch
        self.main = main
        self.ahead = 0
        self.files_changed = 0
        self.age = 'just now'

    @property
    def name(self) -> str:
        return self.path.name

    @property
    def row_id(self) -> str:
        return f'worktree.{slug_of(self.name)}'

    def reference(self) -> str:
        return self.branch or 'HEAD'

    def subtitle(self) -> str:
        parts = [self.branch or 'detached HEAD']
        if self.main:
            parts.append('main worktree')
        parts.extend([
            f'ahead {self.ahead}',
            f'{self.files_changed} file{"" if self.files_changed == 1 else "s"} changed',
            f'last commit {self.age}',
        ])
        return ' \u00b7 '.join(parts)


def read_worktrees(repo, base: str) -> list[Worktree]:
    """The worktrees of `repo`, each with its branch, its count ahead of `base`, its
    changed files and the age of its last commit. The first one is the main worktree."""
    entries = parse_worktrees(git_text(repo, ('worktree', 'list', '--porcelain')))
    now = time.time()
    worktrees = []
    for index, entry in enumerate(entries):
        worktree = Worktree(entry['path'], entry['branch'], index == 0)
        if worktree.branch:
            counted = git_text_or(worktree.path,
                                  ('rev-list', '--count', f'{base}..{worktree.branch}')).strip()
            worktree.ahead = int(counted or 0)
        worktree.files_changed = count_lines(
            git_text_or(worktree.path, ('status', '--porcelain')))
        committed = int(git_text_or(worktree.path, ('log', '-1', '--format=%ct')).strip() or 0)
        worktree.age = age_of(max(0.0, now - committed))
        worktrees.append(worktree)
    return worktrees


def make_task_worktree(repo, state, base: str) -> Worktree:
    """The worktree of one task, on a branch that does not exist yet."""
    slug = free_slug(repo, WORKTREE_TASK)
    branch = f'{WORKTREE_BRANCH_PREFIX}{slug}'
    folder = Path(f'{repo}-wt') / slug
    if not is_under(state, folder):
        raise Fatal(f'the worktree folder {folder} is not under {WORKTREE_STATE_ENV} {state}')
    folder.parent.mkdir(parents=True, exist_ok=True)
    git_text(repo, ('worktree', 'add', '-b', branch, str(folder), base))
    return Worktree(folder, branch, False)


def worktree_environment() -> tuple[Path, Path, str]:
    """The repository, its state folder and the preset of the task panel, checked."""
    repo = os.environ.get(WORKTREE_REPO_ENV, '').strip()
    state = os.environ.get(WORKTREE_STATE_ENV, '').strip()
    preset = os.environ.get(WORKTREE_PRESET_ENV, '').strip() or DEFAULT_WORKTREE_PRESET
    if not repo:
        raise Fatal(f'the worktree scenario needs {WORKTREE_REPO_ENV}')
    if not state:
        raise Fatal(f'the worktree scenario needs {WORKTREE_STATE_ENV}')
    if not os.path.isabs(repo) or not os.path.isabs(state):
        raise Fatal(f'{WORKTREE_REPO_ENV} and {WORKTREE_STATE_ENV} name absolute paths')
    state = Path(os.path.realpath(state))
    repo = Path(os.path.realpath(repo))
    if not is_under(state, repo):
        raise Fatal(f'{WORKTREE_REPO_ENV} {repo} is not under {WORKTREE_STATE_ENV} {state}')
    if not repo.is_dir():
        raise Fatal(f'{WORKTREE_REPO_ENV} {repo} is not a folder')
    root = Path(os.path.realpath(git_text(repo, ('rev-parse', '--show-toplevel')).strip()))
    if root != repo:
        raise Fatal(f'{WORKTREE_REPO_ENV} {repo} is not the root of its repository ({root})')
    return repo, state, preset


def cut_text(text: str, limit: int = WORKTREE_TEXT_CHARS) -> str:
    """`text` cut to the bounds of a card, with one line that says that it was cut."""
    text = text.strip('\n')
    if len(text) <= limit:
        return text
    marker = '\n... the text is cut here'
    return text[:limit - len(marker)].rstrip() + marker


def changed_files(worktree: Worktree, base: str) -> list[str]:
    """The files that the worktree changed against `base`."""
    text = git_text_or(worktree.path, ('diff', '--name-only', f'{base}...{worktree.reference()}'))
    return [line for line in text.splitlines() if line.strip()]


def diff_stat(worktree: Worktree, base: str) -> str:
    """The cut `git diff --stat` of one worktree against `base`."""
    return git_text_or(worktree.path,
                       ('diff', '--stat', f'{base}...{worktree.reference()}')).strip()


def last_subjects(worktree: Worktree) -> str:
    """The subjects of the last commits of one branch."""
    text = git_text_or(worktree.path,
                       ('log', f'-n{WORKTREE_SUBJECTS}', '--format=%s', worktree.reference()))
    return text.strip()


def commits_ahead(repo, worktree: Worktree, base: str) -> str:
    """The commits that a merge of `worktree` brings into `base`."""
    if not worktree.branch:
        return ''
    return git_text_or(repo, ('log', '--oneline', f'{base}..{worktree.branch}')).strip()


def conflicted_files(path) -> list[str]:
    """The files of one merge that did not apply."""
    result = run_git(path, ('diff', '--name-only', '--diff-filter=U'))
    if result.returncode != 0:
        return []
    return [line for line in result.stdout.splitlines() if line.strip()]


class WorktreeCard(Card):
    """One card for the worktrees of a real repository (SKILL.md, `horizon-worktree`).

    The rows select a worktree, the buttons read git again, list the changed files,
    ask before a merge and ask before a remove. A second button that the person does
    not press goes away at the next other click.
    """

    def __init__(self, repo, state, preset: str, base: str, worktrees, task: Worktree):
        super().__init__('worktree', f'Worktrees of {Path(repo).name}')
        self.repo = Path(repo)
        self.state = Path(state)
        self.preset = preset
        self.base = base
        self.task = task
        self.worktrees = worktrees
        self.view = 'worktrees'
        self.selected = task.row_id
        self.files: list[str] = []
        self.file: str | None = None
        self.pending: str | None = None
        self.pending_label: str | None = None
        self.pending_worktree: Worktree | None = None
        self.details: str | None = None
        self.note: str | None = None

    # -- what the card shows ----------------------------------------------

    def selected_worktree(self) -> Worktree | None:
        for worktree in self.worktrees:
            if worktree.row_id == self.selected:
                return worktree
        return None

    def main_worktree(self) -> Worktree | None:
        for worktree in self.worktrees:
            if worktree.main:
                return worktree
        return None

    def worktree_row_ids(self) -> set[str]:
        return {worktree.row_id for worktree in self.worktrees}

    def rows(self, rich: bool) -> list[dict]:
        if self.view == 'changes':
            rows = []
            for index, path in enumerate(self.files):
                row = {'id': f'file.{index}', 'title': cut_text(path, 120)}
                if rich and path == self.file:
                    row['selected'] = True
                rows.append(row)
            return rows
        rows = []
        for worktree in self.worktrees[:WORKTREE_MAX_ROWS]:
            row = {'id': worktree.row_id, 'title': worktree.name,
                   'subtitle': worktree.subtitle()}
            if rich and worktree.row_id == self.selected:
                row['selected'] = True
            rows.append(row)
        return rows

    def shown_text(self) -> str:
        """The text under the list: a question that waits, a diff or the state."""
        if self.details is not None:
            return cut_text(self.details)
        worktree = self.selected_worktree()
        if worktree is None:
            return 'Select one worktree to see its changes.'
        if self.view == 'changes':
            if not self.file:
                return 'This worktree has no changed file.'
            diff = git_text_or(worktree.path,
                               ('diff', f'{self.base}...{worktree.reference()}', '--', self.file))
            return cut_text(diff or 'The file holds no change against the base.')
        stat = diff_stat(worktree, self.base)
        subjects = last_subjects(worktree)
        text = stat or f'No change from {self.base}.'
        return cut_text(f'{text}\n\nLast commits:\n{subjects or "no commit yet"}')

    def buttons(self) -> list[dict]:
        items = []
        if self.pending_label:
            items.append({
                'id': 'merge.confirm' if self.pending == 'merge' else 'remove.confirm',
                'label': self.pending_label,
                'tone': 'primary' if self.pending == 'merge' else 'danger',
            })
        items.extend([
            {'id': 'refresh', 'label': 'Refresh'},
            {'id': 'changes', 'label': 'Show changes'},
        ])
        if self.view == 'changes':
            items.append({'id': 'worktrees', 'label': 'Worktrees'})
        items.extend([
            {'id': 'merge', 'label': 'Merge', 'tone': 'primary'},
            {'id': 'remove', 'label': 'Remove', 'tone': 'danger'},
        ])
        return items

    def document(self, rich: bool) -> dict:
        count = len(self.worktrees)
        subtitle = (f'{count} worktree{"" if count == 1 else "s"} '
                    f'\u00b7 base {self.base}')
        components = heading(f'Worktrees of {self.repo.name}', subtitle, None, rich)
        components.append({'type': 'list', 'selection': 'single', 'rows': self.rows(rich)})
        components.append({'type': 'text', 'text': self.shown_text()})
        if self.view != 'changes' and count > WORKTREE_MAX_ROWS:
            components.append({'type': 'text',
                               'text': f'{count - WORKTREE_MAX_ROWS} more worktree(s) are not shown.'})
        if self.note:
            components.append({'type': 'text', 'text': cut_text(self.note)})
        components.append({'type': 'buttons', 'items': self.buttons()})
        return {'components': components}

    # -- the actions of the buttons ---------------------------------------

    def clear_prompt(self) -> None:
        """Drops a second button that the person did not press."""
        self.pending = None
        self.pending_label = None
        self.pending_worktree = None
        self.details = None

    def refresh(self) -> None:
        """Reads git again, and keeps the selection that still exists."""
        self.worktrees = read_worktrees(self.repo, self.base)
        if self.selected not in self.worktree_row_ids():
            self.selected = self.worktrees[0].row_id if self.worktrees else None
        if self.view == 'changes':
            worktree = self.selected_worktree()
            self.files = changed_files(worktree, self.base) if worktree else []
            if self.file not in self.files:
                self.file = self.files[0] if self.files else None
        else:
            self.files, self.file = [], None

    def select(self, row_id: str) -> None:
        self.view = 'worktrees'
        self.files, self.file = [], None
        self.selected = row_id

    def show_changes(self) -> None:
        worktree = self.selected_worktree()
        if worktree is None:
            self.note = 'Select one worktree first.'
            return
        self.files = changed_files(worktree, self.base)
        if not self.files:
            self.note = f'{worktree.name} has no changed file against {self.base}.'
            return
        self.view = 'changes'
        if self.file not in self.files:
            self.file = self.files[0]
        self.note = f'{len(self.files)} changed file(s) of {worktree.name}.'

    def merge_refusal(self, worktree: Worktree | None) -> str | None:
        if worktree is None:
            return 'Select one worktree first.'
        if worktree.main:
            return (f'{worktree.name} is the main worktree, and a merge arrives there: '
                    f'select the worktree of the task.')
        if not worktree.branch:
            return 'A worktree without a branch has nothing to merge.'
        if worktree.files_changed:
            return (f'{worktree.name} has {worktree.files_changed} item(s) that are not committed, '
                    f'so no merge runs. Commit or remove them first.')
        if worktree.ahead == 0:
            return f'{worktree.branch} is not ahead of {self.base}, so there is nothing to merge.'
        return None

    def ask_merge(self) -> None:
        worktree = self.selected_worktree()
        refusal = self.merge_refusal(worktree)
        if refusal:
            self.note = refusal
            return
        self.details = f'Commits that arrive in {self.base}:\n{commits_ahead(self.repo, worktree, self.base)}'
        self.pending = 'merge'
        self.pending_label = f'Merge {worktree.branch} into {self.base}'
        self.pending_worktree = worktree
        self.note = None

    def confirm_merge(self) -> None:
        worktree = self.pending_worktree
        self.clear_prompt()
        if worktree is None:
            self.note = 'Select one worktree first.'
            return
        base_worktree = self.main_worktree()
        if base_worktree is None:
            self.note = 'This repository has no main worktree, so no merge runs.'
            return
        result = run_git(base_worktree.path, ('merge', '--no-ff', worktree.branch))
        if result.returncode == 0:
            self.note = f'Merged {worktree.branch} into {self.base}. Nothing was pushed.'
        else:
            conflicted = conflicted_files(base_worktree.path)
            aborted = run_git(base_worktree.path, ('merge', '--abort'))
            named = ', '.join(conflicted) if conflicted else 'no reported file'
            outcome = 'aborted' if aborted.returncode == 0 else 'not aborted'
            self.note = f'The merge conflicted in {named}, so the merge was {outcome}. Nothing was pushed.'
        self.refresh()

    def remove_refusal(self, worktree: Worktree | None) -> str | None:
        if worktree is None:
            return 'Select one worktree first.'
        if worktree.main:
            return f'{worktree.name} is the main worktree, so it stays.'
        if not is_under(self.state, worktree.path):
            return (f'{worktree.path} is not under {WORKTREE_STATE_ENV} ({self.state}), '
                    f'so this card removes nothing.')
        return None

    def ask_remove(self) -> None:
        worktree = self.selected_worktree()
        refusal = self.remove_refusal(worktree)
        if refusal:
            self.note = refusal
            return
        uncommitted = git_text_or(worktree.path, ('status', '--porcelain')).strip()
        commits = commits_ahead(self.repo, worktree, self.base)
        self.details = (f'Files that are not committed in {worktree.name}:\n'
                        f'{uncommitted or "none"}\n\n'
                        f'Commits that are on no other branch:\n{commits or "none"}')
        self.pending = 'remove'
        self.pending_label = f'Remove {worktree.name}'
        self.pending_worktree = worktree
        self.note = None

    def confirm_remove(self) -> None:
        worktree = self.pending_worktree
        self.clear_prompt()
        if worktree is None:
            self.note = 'Select one worktree first.'
            return
        # The path is checked again before the command that removes it.
        refusal = self.remove_refusal(worktree)
        if refusal:
            self.note = refusal
            return
        dirty = count_lines(git_text_or(worktree.path, ('status', '--porcelain'))) > 0
        arguments = ['worktree', 'remove', str(worktree.path)]
        if dirty:
            arguments.append('--force')
        git_text(self.repo, tuple(arguments))
        kept = ''
        if worktree.branch:
            if git_succeeds(self.repo, ('merge-base', '--is-ancestor', worktree.branch, self.base)):
                git_text(self.repo, ('branch', '-d', worktree.branch))
                kept = f' Branch {worktree.branch} was merged, so it is deleted.'
            else:
                kept = f' Branch {worktree.branch} is not merged, so it stays.'
        self.note = (f'Removed {worktree.name}.{kept}\n'
                     f'The panel of {worktree.name} is still open; '
                     'its folder is removed. Close it yourself.')
        self.view = 'worktrees'
        self.files, self.file = [], None
        self.refresh()



SCENARIOS = {
    'machine': lambda: MachineCard('machine', MACHINE_TITLE),
    'choose': lambda: ChooseCard('choose', CHOOSE_TITLE,
                                 (row[0] for row in CHOOSE_ROWS), CHOOSE_STEPS),
    'browser': lambda: BrowserCard('browser', BROWSER_TITLE,
                                   (row[0] for row in BROWSER_ROWS), BROWSER_STEPS),
    'tall': lambda: TallCard('tall', TALL_TITLE, (row[0] for row in TALL_ROWS)),
}
# `--scenario all` leaves the tall card out: its scrolling body is a check of
# its own. `--scenario all+tall` adds it to the three cards above.
SCENARIO_ORDER = ('machine', 'choose', 'browser')


def machine_names(second_host: str) -> tuple[str, ...]:
    """The records that the `machines` scenario writes, in creation order."""
    return MACHINE_NAMES if second_host.strip() else MACHINE_NAMES[:1]


def scenario_keys(scenario: str) -> tuple[str, ...]:
    """The cards one `--scenario` name creates, in creation order."""
    if scenario in ('intruder', 'machines', 'worktree'):
        return ()
    if scenario == 'all+tall':
        return SCENARIO_ORDER + ('tall',)
    if scenario == 'all':
        return SCENARIO_ORDER
    return (scenario,)


class CardAgent:
    """Creates the cards and waits for their clicks in one loop."""

    def __init__(self, client: StdioMcpClient, listen: str, click_timeout_seconds: float):
        self.client = client
        self.listen = listen
        self.click_timeout_millis = int(round(click_timeout_seconds * 1000))
        self.cards: list[Card] = []
        self.events: queue.Queue = queue.Queue()
        # One wait on every card, which is what the host answers with the click
        # of whichever card the person pressed. A host that refuses that form
        # gets one wait per card, as older versions of this script did.
        self.any_card_wait = True
        # A host whose model has no heading subtitle, heading badge, kv mono,
        # list row tone or progress detail refuses the rich document; the plain
        # one then serves every card of this run.
        self.rich = True

    # -- card lifecycle ---------------------------------------------------

    def send_document(self, operation: str, card: Card, extra: dict | None = None) -> dict:
        """Send a document, and fall back to the plain model once if refused."""
        arguments = dict(extra or {})
        arguments['operation'] = operation
        if card.panel_id:
            arguments['panel_id'] = card.panel_id
        arguments['document'] = card.document(self.rich)
        payload = self.client.call_tool('block_panel', arguments)
        if (self.rich and payload.get('status') == 'failed'
                and payload.get('code') == 'invalid_document'):
            log(f'{card.key}: the host refused the rich document, so send the plain one '
                f'({payload.get("message")})')
            self.rich = False
            arguments['document'] = card.document(self.rich)
            payload = self.client.call_tool('block_panel', arguments)
        return payload

    def create(self, card: Card) -> None:
        payload = self.send_document('create', card, {'title': card.title})
        self.expect(payload, 'created', f'create {card.key}')
        card.panel_id = payload['panel_id']
        card.revision = payload.get('revision', 1)
        self.cards.append(card)
        log(f'{card.key}: created card {card.panel_id} rev {card.revision} ({card.title})')
        if card.key == 'tall':
            needed = estimated_card_height(card.document(self.rich))
            log(f'{card.key}: the body needs about {needed:.0f} points, above the '
                f'{CARD_MAX_HEIGHT:.0f} point clamp')

    def update(self, card: Card) -> None:
        payload = self.send_document('update', card)
        self.expect(payload, 'updated', f'update {card.key}')
        card.revision = payload.get('revision', card.revision + 1)
        log(f'{card.key}: updated card {card.panel_id} rev {card.revision}')

    def close_card(self, card: Card) -> None:
        payload = self.client.call_tool('block_panel', {
            'operation': 'close', 'panel_id': card.panel_id})
        self.expect(payload, 'closed', f'close {card.key}')
        log(f'{card.key}: closed card {card.panel_id}')

    @staticmethod
    def expect(payload: dict, wanted: str, what: str) -> None:
        status = payload.get('status')
        if status == wanted:
            return
        if status == 'failed':
            raise Refused(payload.get('code') or 'unknown', payload.get('message') or '')
        raise Fatal(f'{what}: block_panel answered {status!r}, expected {wanted!r}')

    # -- the click loop ---------------------------------------------------

    def next_click(self, card: Card, timeout_millis: int) -> dict:
        return self.client.call_tool(
            'block_panel',
            {'operation': 'next_click', 'panel_id': card.panel_id, 'timeout_millis': timeout_millis},
            timeout=timeout_millis / 1000 + NEXT_CLICK_GRACE_SECONDS)

    def handle(self, card: Card, payload: dict, wait_millis: int) -> str:
        """Act on one next_click answer. Returns 'keep' or 'drop'."""
        status = payload.get('status')
        if status == 'timed_out':
            log(f'{card.key}: no click in {wait_millis / 1000:.0f} s')
            return 'keep'
        if status == 'clicked':
            click = payload.get('click') or {}
            component = click.get('component_id')
            log(f'{card.key}: click {component} rev {click.get("revision")} '
                f'stale {click.get("stale")} selected {click.get("selected") or []}')
            return self.react(card, component, click)
        if status == 'failed':
            code = payload.get('code') or 'unknown'
            message = payload.get('message') or ''
            if code in ('panel_unavailable', 'not_block_panel'):
                log(f'{card.key}: card is gone ({code}); stop listening')
                return 'drop'
            if code in ('request_expired', 'host_timeout'):
                log(f'{card.key}: host did not apply the last request ({code}); listen again')
                return 'keep'
            raise Fatal(f'{card.key}: block_panel refused next_click with {code}: {message}')
        raise Fatal(f'{card.key}: next_click answered an unknown status {status!r}')

    def next_click_any(self, timeout_millis: int) -> dict:
        """Wait for the next click on any card this agent owns (SKILL.md)."""
        return self.client.call_tool(
            'block_panel',
            {'operation': 'next_click', 'timeout_millis': timeout_millis},
            timeout=timeout_millis / 1000 + NEXT_CLICK_GRACE_SECONDS)

    def card_by_panel(self, panel_id: str | None) -> Card | None:
        for card in self.cards:
            if card.panel_id == panel_id:
                return card
        return None

    def drop_card(self, card: Card) -> None:
        if card in self.cards:
            self.cards.remove(card)
        log(f'{len(self.cards)} card(s) left')

    def handle_any(self, payload: dict, timeout_line: str) -> str:
        """Act on one wait on every card. Returns 'keep', 'fallback' or 'stop'."""
        status = payload.get('status')
        if status == 'timed_out':
            log(timeout_line)
            return 'keep'
        if status == 'clicked':
            click = payload.get('click') or {}
            card = self.card_by_panel(click.get('panel_id'))
            component = click.get('component_id')
            if card is None:
                log(f'click {component} on card {click.get("panel_id")} that this agent '
                    f'does not own; ignored')
                return 'keep'
            log(f'{card.key}: click {component} rev {click.get("revision")} '
                f'stale {click.get("stale")} selected {click.get("selected") or []}')
            if self.react(card, component, click) == 'drop':
                self.drop_card(card)
            return 'keep'
        if status == 'failed':
            code = payload.get('code') or 'unknown'
            message = payload.get('message') or ''
            if code in NEXT_CLICK_RETRY_CODES:
                log(f'no click read: the host did not apply the wait ({code}); wait again')
                return 'keep'
            if code in NEXT_CLICK_EMPTY_CODES:
                log(f'the host says this agent owns no card ({code}); stop listening')
                return 'stop'
            if code in NEXT_CLICK_FATAL_CODES:
                raise Fatal(f'block_panel refused next_click with {code}: {message}')
            if code in NEXT_CLICK_GONE_CODES:
                log(f'the host reports no reachable card any more ({code}); '
                    f'read each card once to close the books')
                return 'fallback'
            # Any other refusal means this host does not take a wait without a
            # card: an older host needs the panel id and answers one card only.
            log(f'the host refused the wait on every card ({code}: {message}); '
                f'listen on one card at a time instead')
            return 'fallback'
        raise Fatal(f'next_click on every card answered an unknown status {status!r}')

    def wait_on_every_card(self, wait_millis: int, timeout_line: str) -> str:
        """One wait on every card, dispatched to the card the click names."""
        try:
            payload = self.next_click_any(wait_millis)
        except McpTimeout:
            log(f'{timeout_line} (no answer inside the wait)')
            self.drain_stdin()
            return 'keep'
        self.drain_stdin()
        return self.handle_any(payload, timeout_line)

    def react(self, card: Card, component: str | None, click: dict) -> str:
        if card.key == 'worktree':
            return self.react_worktree(card, component)
        selected = [row_id for row_id in (click.get('selected') or []) if isinstance(row_id, str)]
        closes = (component == 'cancel'
                  or (card.key == 'machine' and component == 'delete')
                  or (card.key == 'tall' and component == 'close'))
        if closes:
            self.close_card(card)
            return 'drop'
        if card.key == 'machine':
            if component == 'stop':
                card.stopped = True
                self.update(card)
            elif component == 'start':
                card.stopped = False
                self.update(card)
            elif component == 'connect':
                self.connect_terminal(card)
            else:
                log(f'{card.key}: no action for {component!r}; the card stays as it is')
            return 'keep'
        if component in card.row_ids:
            # A row click only moves the selection; the choice reaches the agent
            # with the button that follows, in `selected`.
            log(f'{card.key}: row {component} selected; the button below decides')
            return 'keep'
        if card.key == 'tall':
            if component == 'ok':
                card.pressed_at = clock()
                self.update(card)
            else:
                log(f'{card.key}: no action for {component!r}; the card stays as it is')
            return 'keep'
        expected_button = 'continue' if card.key == 'choose' else 'setup'
        if component == expected_button:
            rows = CHOOSE_ROWS if card.key == 'choose' else BROWSER_ROWS
            titles = [row_title(rows, row_id) or row_id for row_id in selected]
            card.chosen = ', '.join(titles) if titles else None
            if card.chosen:
                log(f'{card.key}: {expected_button} with {card.chosen}')
            else:
                log(f'{card.key}: {expected_button} with no row selected')
            card.step = min(card.step + 1, len(card.steps) - 1)
            self.update(card)
            return 'keep'
        log(f'{card.key}: no action for {component!r}; the card stays as it is')
        return 'keep'

    def connect_terminal(self, card: Card) -> None:
        """Open an SSH terminal beside the card, and say on the card what happened.

        Horizon builds the `ssh` command line itself, so this hands it the host
        and at most one line to run (SKILL.md, "Connect an SSH terminal").
        """
        host = os.environ.get(SSH_HOST_ENV, '').strip()
        remote_command = os.environ.get(SSH_COMMAND_ENV, '').strip()
        if not host:
            note = f'No SSH host is set in {SSH_HOST_ENV}, so no terminal was opened.'
            card.terminal_note = note
            log(f'{card.key}: connect: {note}')
            self.update(card)
            return
        arguments = {'operation': 'open_terminal', 'ssh': {'host': host},
                     'title': TERMINAL_TITLE}
        if remote_command:
            arguments['remote_command'] = remote_command
        payload = self.client.call_tool('block_panel', arguments)
        at = clock()
        if payload.get('status') == 'opened':
            card.terminal_note = f'A terminal opened beside this panel at {at}.'
            log(f'{card.key}: connect: opened terminal {payload.get("panel_id")} on {host} '
                f'at {at} ({TERMINAL_TITLE})')
        else:
            code = answer_code(payload)
            message = payload.get('message') or ''
            card.terminal_note = f'The host refused the terminal: {code}.'
            log(f'{card.key}: connect: the host refused the terminal with {code} ({message})')
        self.update(card)

    def drain_stdin(self) -> None:
        """Read what the person typed into this panel, without blocking."""
        while True:
            try:
                kind, line = self.events.get_nowait()
            except queue.Empty:
                return
            if kind == 'line':
                self.note_line(line)
            else:
                raise Eof

    def note_line(self, line: str) -> None:
        if 'next_click' in line:
            log('wake line seen: a click waits on one of the cards')
        elif line.strip():
            log(f'panel input ignored: {line.strip()[:60]!r}')

    def run_continuous(self) -> None:
        """One wait on every card, and a rotation only if a host refused it."""
        while self.cards:
            if not self.any_card_wait:
                self.rotate_cards()
                continue
            outcome = self.wait_on_every_card(
                self.click_timeout_millis,
                f'no click on any of the {len(self.cards)} card(s) in '
                f'{self.click_timeout_millis / 1000:.0f} s')
            if outcome == 'fallback':
                self.any_card_wait = False
            elif outcome == 'stop':
                self.cards.clear()

    def rotate_cards(self) -> None:
        """One wait per card, for a host that refuses the wait on every card."""
        for card in list(self.cards):
            try:
                payload = self.next_click(card, self.click_timeout_millis)
            except McpTimeout:
                log(f'{card.key}: no answer inside the wait; listen again')
                self.drain_stdin()
                continue
            self.drain_stdin()
            if self.handle(card, payload, self.click_timeout_millis) == 'drop':
                self.drop_card(card)

    def run_once(self) -> None:
        """Call next_click only after a wake line, as a real agent reads it."""
        while self.cards:
            while True:
                try:
                    kind, line = self.events.get(timeout=0.2)
                    break
                except queue.Empty:
                    continue
            if kind != 'line':
                raise Eof
            self.note_line(line)
            if self.any_card_wait:
                outcome = self.wait_on_every_card(
                    WAKE_READ_TIMEOUT_MILLIS,
                    f'no click waited after the wake line on {len(self.cards)} card(s)')
                if outcome == 'fallback':
                    self.any_card_wait = False
                elif outcome == 'stop':
                    self.cards.clear()
                continue
            for card in list(self.cards):
                try:
                    payload = self.next_click(card, WAKE_READ_TIMEOUT_MILLIS)
                except McpTimeout:
                    log(f'{card.key}: no click waited after the wake line')
                    continue
                if self.handle(card, payload, WAKE_READ_TIMEOUT_MILLIS) == 'drop':
                    self.drop_card(card)

    def run(self, scenario: str) -> int:
        if scenario == 'intruder':
            return self.run_intruder()
        if scenario == 'machines':
            return self.run_machines()
        if scenario == 'worktree':
            self.create(self.prepare_worktree())
            self.refuse_open_panel_requests(self.cards[0])
        else:
            for key in scenario_keys(scenario):
                self.create(SCENARIOS[key]())
        model = 'rich' if self.rich else 'plain'
        log(f'documents use the {model} model of the card document')
        form = 'every card' if self.any_card_wait else 'one card'
        log(f'listening {self.listen} on {form} with a {self.click_timeout_millis // 1000} s wait')
        if self.listen == 'once':
            self.run_once()
        else:
            self.run_continuous()
        if self.cards:
            log(f'{len(self.cards)} card(s) still open; exit')
        else:
            log('every card is closed; exit')
        return 0

    # -- the intruder check -----------------------------------------------

    def foreign_cards(self) -> list[dict]:
        """The cards that another agent owns, waiting for the first to appear."""
        for attempt in range(1, INTRUDER_LIST_ATTEMPTS + 1):
            payload = self.client.call_tool('block_panel', {'operation': 'list'})
            if payload.get('status') != 'panels':
                raise Fatal(f'intruder: list answered {answer_code(payload)}: '
                            f'{payload.get("message") or "no card list"}')
            cards = [card for card in payload.get('panels') or [] if isinstance(card, dict)]
            foreign = [card for card in cards if not card.get('owned_by_caller')]
            if foreign:
                log(f'intruder: list answered {len(cards)} card(s), {len(foreign)} of them of '
                    f'another agent')
                return foreign
            if attempt < INTRUDER_LIST_ATTEMPTS:
                log(f'intruder: list answered {len(cards)} card(s) and none is foreign; look '
                    f'again in {INTRUDER_LIST_WAIT_SECONDS:.0f} s '
                    f'({attempt} of {INTRUDER_LIST_ATTEMPTS})')
                time.sleep(INTRUDER_LIST_WAIT_SECONDS)
        log(f'intruder: no card of another agent appeared in '
            f'{INTRUDER_LIST_ATTEMPTS} looks; read no card')
        return []

    def run_intruder(self) -> int:
        """Prove that this agent may read another agent's card but not change it."""
        for summary in self.foreign_cards():
            panel_id = summary.get('panel_id') or ''
            log(f'intruder: card {panel_id} ({summary.get("title") or "no title"}) is owned by '
                f'another agent')
            for operation, extra in (('inspect', {}),
                                     ('update', {'document': INTRUDER_DOCUMENT}),
                                     ('close', {})):
                payload = self.client.call_tool(
                    'block_panel', {'operation': operation, 'panel_id': panel_id, **extra})
                log(f'intruder: card {panel_id} {operation} answered {answer_code(payload)}')
        payload = self.client.call_tool(
            'block_panel',
            {'operation': 'next_click', 'timeout_millis': INTRUDER_CLICK_TIMEOUT_MILLIS},
            timeout=INTRUDER_CLICK_TIMEOUT_MILLIS / 1000 + NEXT_CLICK_GRACE_SECONDS)
        log(f'intruder: the wait on every card answered {answer_code(payload)}')
        log('intruder check done')
        self.idle_until_eof()
        return 0

    # -- the worktree card -----------------------------------------------

    def prepare_worktree(self) -> WorktreeCard:
        """The task worktree, its panel, and the card of the repository."""
        repo, state, preset = worktree_environment()
        base = base_branch(repo)
        task = make_task_worktree(repo, state, base)
        log(f'worktree: branch {task.branch} in {task.path} from {base}')
        answer = self.client.call_tool('block_panel', {
            'operation': 'open_panel', 'preset': preset,
            'directory': str(task.path), 'title': WORKTREE_TASK})
        log(f'worktree: open_panel with {preset!r} answered {answer_code(answer)}{answer_words(answer)}')
        return WorktreeCard(repo, state, preset, base, read_worktrees(repo, base), task)

    def refuse_open_panel_requests(self, card: WorktreeCard) -> None:
        """Prints the three refusals of `open_panel` once, and draws no card for them."""
        folder = str(card.task.path)
        for label, arguments in (
            ('an unknown preset', {'preset': 'No such preset', 'directory': folder}),
            ('a folder that is not there', {'preset': card.preset,
                                            'directory': str(card.state / 'gone')}),
            (f'the browser preset {WORKTREE_BROWSER_PRESET!r}',
             {'preset': WORKTREE_BROWSER_PRESET, 'directory': folder}),
        ):
            answer = self.client.call_tool('block_panel', {'operation': 'open_panel', **arguments})
            log(f'worktree: open_panel with {label} answered {answer_code(answer)}{answer_words(answer)}')

    def react_worktree(self, card: WorktreeCard, component: str | None) -> str:
        """Act on one press of the worktree card, and draw the result."""
        confirm = {'merge': 'merge.confirm', 'remove': 'remove.confirm'}.get(card.pending)
        if component != confirm:
            # A second button that the person did not press goes away here.
            card.clear_prompt()
        file_row = component and component.startswith('file.')
        if component in card.worktree_row_ids():
            card.select(component)
            card.note = None
        elif file_row:
            try:
                index = int(component.split('.', 1)[1])
            except ValueError:
                index = -1
            if 0 <= index < len(card.files):
                card.file = card.files[index]
            else:
                card.note = f'The file {component!r} is no row of this card.'
        elif component == 'refresh':
            card.refresh()
            card.note = f'Read {len(card.worktrees)} worktree(s) again.'
        elif component == 'changes':
            card.show_changes()
        elif component == 'worktrees':
            card.view = 'worktrees'
            card.files, card.file = [], None
        elif component == 'merge':
            card.ask_merge()
        elif component == 'merge.confirm':
            card.confirm_merge()
            log(f'worktree: {one_line(card.note)}')
        elif component == 'remove':
            card.ask_remove()
        elif component == 'remove.confirm':
            card.confirm_remove()
            log(f'worktree: {one_line(card.note)}')
        else:
            card.note = f'No action for {component!r}; the card stays as it is.'
        self.update(card)
        return 'keep'

    # -- the machine list ------------------------------------------------

    def run_machines(self) -> int:
        """Register one or two machines of this Horizon and print their records."""
        host = os.environ.get(SSH_HOST_ENV, '').strip()
        if not host:
            raise Fatal(f'the machines scenario needs a second host, or {SSH_HOST_ENV}')
        second_host = os.environ.get(SECOND_SSH_HOST_ENV, '').strip()
        user = os.environ.get(SSH_USER_ENV, '').strip()
        port = os.environ.get(SSH_PORT_ENV, '').strip()
        for index, name in enumerate(machine_names(second_host)):
            target = host if index == 0 else second_host
            key = self.client.call_tool('block_panel', {'operation': 'machine_key', 'name': name})
            self.expect(key, 'machine_key', f'machine_key {name}')
            log(f'machines: key {name} created={key.get("created")} '
                f'public line {key.get("public_key")}')
            route = {'host': target}
            if user:
                route['user'] = user
            if port:
                route['port'] = int(port)
            answer = self.client.call_tool('block_panel', {
                'operation': 'machine_set', 'name': name, 'ssh': route, 'key': name,
                'session': name, 'provider': MACHINE_PROVIDER})
            self.expect(answer, 'machine_set', f'machine_set {name}')
            log(f'machines: record {answer.get("name")} created={answer.get("created")} '
                f'at {target} with key {name} and session {name}')
        listed = self.client.call_tool('block_panel', {'operation': 'machines'})
        self.expect(listed, 'machines', 'machines')
        records = [record for record in listed.get('machines') or [] if isinstance(record, dict)]
        for record in records:
            log(f'machines: list holds {record.get("name")} at {record.get("host")} '
                f'open={record.get("terminal_open")} provider={record.get("provider")}')
        log(f'machines: {len(records)} record(s) registered; the section shows them now')
        self.idle_until_eof()
        return 0

    def idle_until_eof(self) -> None:
        """Keep the panel open with nothing to do until its stdin closes."""
        while True:
            try:
                kind, _line = self.events.get(timeout=0.5)
            except queue.Empty:
                continue
            if kind == 'eof':
                raise Eof


def install_signal_handlers() -> None:
    def raise_stop(_signum, _frame):
        raise Stop

    for name in ('SIGTERM', 'SIGINT', 'SIGHUP'):
        number = getattr(signal, name, None)
        if number is not None:
            signal.signal(number, raise_stop)


def parse_args(argv: list[str]) -> tuple[argparse.Namespace, list[str]]:
    parser = argparse.ArgumentParser(
        description='Drive Horizon cards from an agent panel with the block_panel MCP tool.')
    parser.add_argument(
        '--mcp-command', default=os.environ.get(MCP_COMMAND_ENV, ''),
        help='Command line of the Horizon MCP server, such as "<horizon> --browser-mcp". '
             f'Defaults to the {MCP_COMMAND_ENV} environment variable.')
    parser.add_argument(
        '--scenario',
        choices=('machine', 'choose', 'browser', 'tall', 'all', 'all+tall', 'intruder', 'machines',
                 'worktree'),
        default='all',
        help='Which cards to create and drive; default all three. all+tall adds the tall '
             'card, whose body scrolls; tall alone creates only that card. intruder creates '
             'no card: it reads the cards of the other agents and tries to change one, '
             'then stays open until stdin closes. machines creates no card: it registers one '
             'or two machines of this Horizon from CARD_AGENT_SSH_HOST and '
             'CARD_AGENT_SSH_HOST_2, prints the records, then stays open until stdin closes. '
             'worktree starts a task worktree in CARD_AGENT_REPO, opens its panel with '
             'CARD_AGENT_PRESET, prints the three refusals of open_panel, and draws one card '
             'for the worktrees of that repository.')
    parser.add_argument(
        '--listen', choices=('continuous', 'once'), default='continuous',
        help='continuous: keep one next_click wait on every card the agent owns. once: '
             'call next_click only after a wake line that carries "next_click" arrives '
             'on stdin.')
    parser.add_argument(
        '--click-timeout', type=float, default=DEFAULT_CLICK_TIMEOUT_SECONDS,
        help='Seconds one next_click waits, 1-3600; default 60.')
    args, extra = parser.parse_known_args(argv)
    if not (MIN_CLICK_TIMEOUT_MILLIS / 1000 <= args.click_timeout <= MAX_CLICK_TIMEOUT_MILLIS / 1000):
        parser.error(f'--click-timeout takes 1-3600 seconds; received {args.click_timeout}')
    if not args.mcp_command.strip():
        parser.error(f'--mcp-command is required, or set {MCP_COMMAND_ENV}')
    return args, extra


def main(argv: list[str] | None = None) -> int:
    args, extra = parse_args(sys.argv[1:] if argv is None else argv)
    install_signal_handlers()
    actor = os.environ.get('HORIZON_BROWSER_ACTOR')
    host = os.environ.get('HORIZON_BROWSER_HOST_INSTANCE')
    client = None
    try:
        log(f'MCP command: {args.mcp_command}')
        if actor and host:
            log(f'identity {actor} on host {host}')
        else:
            log('no Horizon identity in this environment; block_panel will answer caller_unavailable')
        if extra:
            log(f'ignored launch arguments: {" ".join(extra)}')
        client = StdioMcpClient(args.mcp_command)
        log(f'MCP server started, pid {client.proc.pid}')
        handshake = client.initialize()
        server = (handshake.get('serverInfo') or {}).get('name') or 'unknown'
        log(f'initialized with {server}, protocol {handshake.get("protocolVersion")}')
        tools = client.tool_names()
        if 'block_panel' not in tools:
            log(f'the MCP server offers no block_panel tool; it lists {len(tools)} tools')
            raise Fatal('block_panel is missing from tools/list; this server cannot draw cards')
        log(f'{len(tools)} tools, block_panel present')
        agent = CardAgent(client, args.listen, args.click_timeout)
        StdinWatcher(sys.stdin.fileno(), agent.events).start()
        return agent.run(args.scenario)
    except Stop:
        log('signal received; exit')
        return 0
    except Eof:
        log('stdin closed; the panel went away; exit')
        return 0
    except Fatal as error:
        print(f'{PROGRAM}: {error}', file=sys.stderr, flush=True)
        return 1
    finally:
        if client is not None:
            client.close()


if __name__ == '__main__':
    sys.exit(main())
