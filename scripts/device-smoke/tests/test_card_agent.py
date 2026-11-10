#!/usr/bin/env python3
"""Self test for card_agent.py, with a tiny fake MCP server in this same file.

The repository runs it with the other device smoke tests:

    python3 -B -m unittest discover -s scripts/device-smoke/tests -v

It starts `card_agent.py` as a child, gives it this file as its MCP server
(`<python> test_card_agent.py --serve-fake`), and proves:

  * the initialize handshake, notifications/initialized and tools/list;
  * three `block_panel` creates, each with a document that follows every rule of
    crates/horizon-browser-control/src/block/{model,validate}.rs;
  * one `next_click` on every card of the agent, dispatched to the scenario of the
    card that the answer names, with the default 60 s wait of --click-timeout;
  * a host that refuses a wait without a card gets one wait per card instead;
  * the choose and browser lists mark their default row `selected: true`, and the
    plain fallback marks none;
  * a click leads to an `update` that carries the stopped badge and a Start
    button, and the next click on that card leads to a `close`;
  * the `tall` scenario needs more than the 900 point card height, and its close
    button closes it; `--scenario all` does not create it;
  * `--listen once` calls next_click only after a wake line that carries
    "next_click", then reads every card at once, and exits 0 on EOF;
  * the Connect button of the machine card opens a terminal with `open_terminal`
    for the host in CARD_AGENT_SSH_HOST and the line in CARD_AGENT_SSH_COMMAND,
    says on the card when it opened, says when no host is set, and shows the
    refusal code of a host that refused it;
  * `--scenario intruder` creates no card, reads a card of another agent, is
    refused with not_owner when it changes it, waits for a card to appear and
    keeps the panel open until stdin closes;
  * every line the agent prints is safe for Horizon's agent status scrape.

Standard library only. Nothing is written outside the folder of this file.
"""
from __future__ import annotations

import sys

# Importing card_agent.py for its document rules must not leave a __pycache__
# beside the two files of this folder.
sys.dont_write_bytecode = True

from pathlib import Path
import contextlib
import io
import json
import os
import queue
import re
import shlex
import signal
import subprocess
import threading
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import card_agent  # noqa: E402

HERE = str(Path(__file__).resolve().parent)
AGENT = str(Path(card_agent.__file__).resolve())
THIS_FILE = str(Path(__file__).resolve())

# The wake line that Horizon types into an owner's terminal
# (crates/horizon-core/src/block_panel/wake.rs, WAKE_LINE).
WAKE_LINE = ('A button was pressed on your Horizon card. '
             'Call the block_panel tool with operation next_click to read it.')

ID_PATTERN = re.compile(r'^[a-z0-9_.-]{1,64}$')
TONES = {'neutral', 'info', 'ok', 'warn', 'danger', 'primary'}
SELECTIONS = {'none', 'single'}
MAX_TEXT_CHARS = 512
MAX_COMPONENTS = 64
MAX_KV_ROWS = 24
MAX_LIST_ROWS = 32
MAX_BUTTONS = 8
# The two names that card_agent.py reads the SSH route of the Connect button from.
SSH_HOST_ENV = 'CARD_AGENT_SSH_HOST'
SSH_COMMAND_ENV = 'CARD_AGENT_SSH_COMMAND'


# --------------------------------------------------------------------------
# Document rules: a Python mirror of block/model.rs and block/validate.rs.
# --------------------------------------------------------------------------

class DocumentError(AssertionError):
    pass


def check_text(value, place, multiline=False):
    if not isinstance(value, str):
        raise DocumentError(f'{place} is not a string: {value!r}')
    if not value and not multiline:
        raise DocumentError(f'{place} is empty')
    if len(value) > MAX_TEXT_CHARS:
        raise DocumentError(f'{place} has {len(value)} characters')
    for character in value:
        if character == '\n' and multiline:
            continue
        if ord(character) < 0x20 or ord(character) == 0x7F:
            raise DocumentError(f'{place} holds control character U+{ord(character):04X}')


def check_fields(component, place, required, optional=()):
    allowed = set(required) | set(optional) | {'type'}
    unknown = set(component) - allowed
    if unknown:
        raise DocumentError(f'{place} has unknown fields {sorted(unknown)}')
    missing = set(required) - set(component)
    if missing:
        raise DocumentError(f'{place} misses fields {sorted(missing)}')


def check_id(value, place, seen):
    if not isinstance(value, str) or not ID_PATTERN.match(value):
        raise DocumentError(f'{place} is not a valid id: {value!r}')
    if value in seen:
        raise DocumentError(f'{place} repeats the id {value}')
    seen.add(value)


def check_tone(tone, place):
    if tone not in TONES:
        raise DocumentError(f'{place} is not a tone: {tone!r}')


def rich_fields(document) -> list[str]:
    """The places that use a field the first version of the model does not have."""
    found = []
    for index, component in enumerate(document.get('components', [])):
        at = f'components[{index}]'
        if component.get('type') == 'heading':
            if 'subtitle' in component:
                found.append(f'{at}.subtitle')
            if 'badge' in component:
                found.append(f'{at}.badge')
        if component.get('type') == 'progress' and 'detail' in component:
            found.append(f'{at}.detail')
        if component.get('type') == 'kv':
            found += [f'{at}.rows[{row}].mono' for row, entry in enumerate(component['rows'])
                      if 'mono' in entry]
        if component.get('type') == 'list':
            found += [f'{at}.rows[{row}].tone' for row, entry in enumerate(component['rows'])
                      if 'tone' in entry]
            found += [f'{at}.rows[{row}].selected' for row, entry in enumerate(component['rows'])
                      if entry.get('selected')]
    return found


def default_rows(document) -> list[str]:
    """The row ids the document draws as the default choice, in draw order."""
    rows = []
    for component in document['components']:
        if component['type'] == 'list':
            rows += [row['id'] for row in component['rows'] if row.get('selected')]
    return rows


def document_text(document) -> str:
    """Every text of the document, joined, so a test can look for a phrase."""
    parts = []
    for component in document['components']:
        for field in ('text', 'label', 'subtitle'):
            value = component.get(field)
            if isinstance(value, str):
                parts.append(value)
        badge = component.get('badge')
        if isinstance(badge, dict):
            parts.append(badge.get('text') or '')
    return ' '.join(parts)


def badge_texts(document) -> list[str]:
    """Every badge text, whether it sits on a heading or in its own component."""
    texts = []
    for component in document['components']:
        if component['type'] == 'badge':
            texts.append(component['text'])
        if component['type'] == 'heading' and component.get('badge'):
            texts.append(component['badge']['text'])
    return texts


def validate_document(document):
    """Raise DocumentError unless this document is one Horizon would draw."""
    if not isinstance(document, dict):
        raise DocumentError(f'the document is not an object: {type(document).__name__}')
    if set(document) != {'components'}:
        raise DocumentError(f'the document has fields {sorted(document)}')
    components = document['components']
    if not isinstance(components, list):
        raise DocumentError('components is not a list')
    if len(components) > MAX_COMPONENTS:
        raise DocumentError(f'components has {len(components)} items')
    seen = set()
    for index, component in enumerate(components):
        at = f'components[{index}]'
        if not isinstance(component, dict):
            raise DocumentError(f'{at} is not an object')
        kind = component.get('type')
        if kind == 'heading':
            check_fields(component, at, ('text',), ('subtitle', 'badge'))
            check_text(component['text'], f'{at}.text')
            if 'subtitle' in component:
                check_text(component['subtitle'], f'{at}.subtitle')
            if 'badge' in component:
                badge = component['badge']
                if not isinstance(badge, dict) or set(badge) - {'text', 'tone'}:
                    raise DocumentError(f'{at}.badge must hold text and tone: {badge!r}')
                check_text(badge['text'], f'{at}.badge.text')
                check_tone(badge.get('tone', 'neutral'), f'{at}.badge.tone')
        elif kind == 'text':
            check_fields(component, at, ('text',))
            check_text(component['text'], f'{at}.text', multiline=True)
        elif kind == 'kv':
            check_fields(component, at, ('rows',))
            rows = component['rows']
            if not isinstance(rows, list) or len(rows) > MAX_KV_ROWS:
                raise DocumentError(f'{at}.rows is not a bounded list')
            for row_index, row in enumerate(rows):
                place = f'{at}.rows[{row_index}]'
                if not isinstance(row, dict) or set(row) - {'label', 'value', 'mono'}:
                    raise DocumentError(f'{place} must hold label and value: {row!r}')
                check_text(row['label'], f'{place}.label')
                check_text(row['value'], f'{place}.value')
                if 'mono' in row and not isinstance(row['mono'], bool):
                    raise DocumentError(f'{place}.mono is not a bool: {row["mono"]!r}')
        elif kind == 'badge':
            check_fields(component, at, ('text',), ('tone',))
            check_text(component['text'], f'{at}.text')
            check_tone(component.get('tone', 'neutral'), f'{at}.tone')
        elif kind == 'progress':
            check_fields(component, at, ('label', 'percent'), ('detail',))
            check_text(component['label'], f'{at}.label')
            if 'detail' in component:
                check_text(component['detail'], f'{at}.detail')
            percent = component['percent']
            if not isinstance(percent, int) or isinstance(percent, bool) or not 0 <= percent <= 100:
                raise DocumentError(f'{at}.percent is not 0-100: {percent!r}')
        elif kind == 'divider':
            check_fields(component, at, ())
        elif kind == 'list':
            check_fields(component, at, ('selection', 'rows'))
            if component['selection'] not in SELECTIONS:
                raise DocumentError(f'{at}.selection is {component["selection"]!r}')
            rows = component['rows']
            if not isinstance(rows, list) or len(rows) > MAX_LIST_ROWS:
                raise DocumentError(f'{at}.rows is not a bounded list')
            selected_row = None
            for row_index, row in enumerate(rows):
                place = f'{at}.rows[{row_index}]'
                if not isinstance(row, dict):
                    raise DocumentError(f'{place} is not an object')
                check_fields(row, place, ('id', 'title'), ('subtitle', 'tone', 'selected'))
                check_id(row['id'], f'{place}.id', seen)
                check_text(row['title'], f'{place}.title')
                if 'subtitle' in row:
                    check_text(row['subtitle'], f'{place}.subtitle')
                if 'tone' in row:
                    check_tone(row['tone'], f'{place}.tone')
                if 'selected' in row and not isinstance(row['selected'], bool):
                    raise DocumentError(f'{place}.selected is not a bool: {row["selected"]!r}')
                if row.get('selected'):
                    if component['selection'] != 'single':
                        raise DocumentError(f'{place}.selected is set, but the list does '
                                            f'not let the person select a row')
                    if selected_row is not None:
                        raise DocumentError(f'{place}.selected is the second selected row of '
                                            f'its list; {selected_row} is already selected')
                    selected_row = f'{place}.selected'
        elif kind == 'buttons':
            check_fields(component, at, ('items',))
            items = component['items']
            if not isinstance(items, list) or len(items) > MAX_BUTTONS:
                raise DocumentError(f'{at}.items is not a bounded list')
            for item_index, item in enumerate(items):
                place = f'{at}.items[{item_index}]'
                if not isinstance(item, dict):
                    raise DocumentError(f'{place} is not an object')
                check_fields(item, place, ('id', 'label'), ('tone',))
                check_id(item['id'], f'{place}.id', seen)
                check_text(item['label'], f'{place}.label')
                check_tone(item.get('tone', 'neutral'), f'{place}.tone')
        else:
            raise DocumentError(f'{at} has an unknown type {kind!r}')
    # The serialized size rule counts the JSON bytes the agent sent.
    encoded = len(json.dumps(document).encode('utf-8'))
    if encoded > 32 * 1024:
        raise DocumentError(f'the document is {encoded} bytes')


# --------------------------------------------------------------------------
# The fake MCP server. `python3 test_card_agent.py --serve-fake`
# --------------------------------------------------------------------------

# The clicks the fake host holds, per mode and per panel id: the cards that the
# agent creates, in creation order, and the clicks that wait on each of them.
MODE_CLICKS = {
    # One press on every card of `--scenario all`, so that a single wait on every
    # card must dispatch each click to the card that the event names.
    'full': {
        'p1': [{'component_id': 'stop'}, {'component_id': 'delete'}],
        'p2': [{'component_id': 'continue', 'selected': ['hetzner']},
               {'component_id': 'cancel'}],
        'p3': [{'component_id': 'setup', 'selected': ['chrome']},
               {'component_id': 'cancel'}],
    },
    'wake': {'p1': [{'component_id': 'stop'}]},
    'old': {'p1': [{'component_id': 'stop'}, {'component_id': 'delete'}]},
    'oldwait': {'p1': [{'component_id': 'stop'}, {'component_id': 'delete'}]},
    'tall': {'p1': [{'component_id': 'ok'}, {'component_id': 'close'}]},
    # The person presses Connect, which opens a terminal, and then Delete.
    'connect': {'p1': [{'component_id': 'connect'}, {'component_id': 'delete'}]},
    # The same press on a host that refuses the route: the card says so, and the
    # person can still delete the machine.
    'connect-refused': {'p1': [{'component_id': 'connect'}, {'component_id': 'delete'}]},
    # The person closes the card in Horizon while the agent waits on it.
    'closed': {},
    # `--scenario all+tall`: the three cards above, and the tall card as the fourth.
    'alltall': {
        'p1': [{'component_id': 'stop'}, {'component_id': 'delete'}],
        'p2': [{'component_id': 'continue', 'selected': ['hetzner']},
               {'component_id': 'cancel'}],
        'p3': [{'component_id': 'setup', 'selected': ['chrome']},
               {'component_id': 'cancel'}],
        'p4': [{'component_id': 'ok'}, {'component_id': 'close'}],
    },
}


# The card that another agent owns, as `list` reports it to the intruder: it is
# readable, and every change of it is refused with not_owner (SKILL.md, "Update a
# card").
INTRUDER_PANEL = {'panel_id': 'p9', 'title': 'Machine \u00b7 build-1', 'revision': 4,
                  'owned_by_caller': False, 'pending_clicks': 1, 'listening': True}
INTRUDER_MODE_LIST = ('intruder', 'intruder-empty')


class FakeServer:
    """A newline-delimited JSON-RPC MCP server that plays the host's answers.

    FAKE_MCP_MODE=full: one press on each of the three cards of `--scenario all`:
    stop and delete on the machine card, continue and cancel on the choose card,
    setup and cancel on the browser card. Every card is closed, so the agent
    exits.
    FAKE_MCP_MODE=wake: the machine card gets one "stop" click, then timeouts.
    FAKE_MCP_MODE=old: any document that uses a field of the newer model is
    refused as invalid_document, so the agent must fall back to the plain one.
    FAKE_MCP_MODE=oldwait: a `next_click` without `panel_id` is refused, as an
    older host refuses it, so the agent must wait on one card at a time.
    FAKE_MCP_MODE=tall: the tall card gets an "ok" click and then a "close".
    FAKE_MCP_MODE=closed: the person closes the card at once, so the wait on
    every card and the wait on that card answer panel_unavailable.
    FAKE_MCP_MODE=notool: tools/list does not hold block_panel.

    A wait without `panel_id` answers the oldest waiting click of the cards the
    agent owns, oldest first, as Horizon does (SKILL.md, "Read a click").
    """

    def __init__(self, log_path: str, mode: str):
        self.log = open(log_path, 'a', buffering=1, encoding='utf-8')
        self.mode = mode
        self.panels: dict[str, dict] = {}
        self.order: list[str] = []
        self.created = 0
        self.events = 0
        # `intruder-empty` answers an empty list first, as a workspace whose
        # other agent has not drawn its card yet does.
        self.lists = 0

    def record(self, entry: dict) -> None:
        self.log.write(json.dumps(entry, sort_keys=True) + '\n')

    def click(self, panel_id: str, panel: dict) -> dict:
        click = panel['clicks'].pop(0)
        self.events += 1
        return {'status': 'clicked', 'click': {
            'event_id': self.events,
            'panel_id': panel_id,
            'component_id': click['component_id'],
            'revision': panel['revision'],
            'selected': click.get('selected', []),
            'at_millis': int(time.time() * 1000),
            'stale': False}}

    def no_click(self) -> dict:
        # A real host waits; a short pause keeps this loop from spinning.
        time.sleep(0.1)
        return {'status': 'timed_out', 'timed_out': True}

    def summary(self, panel_id: str, panel: dict) -> dict:
        return {'panel_id': panel_id, 'title': panel.get('title') or 'card',
                'revision': panel['revision'], 'owned_by_caller': True,
                'pending_clicks': len(panel['clicks']), 'listening': True}

    def card_list(self) -> dict:
        """The `list` answer: this agent's own cards, or the card of another agent."""
        if self.mode in INTRUDER_MODE_LIST:
            self.lists += 1
            if self.mode == 'intruder-empty' and self.lists < 2:
                return {'status': 'panels', 'panels': []}
            return {'status': 'panels', 'panels': [dict(INTRUDER_PANEL)]}
        panels = [self.summary(panel_id, self.panels[panel_id])
                  for panel_id in self.order if panel_id in self.panels]
        return {'status': 'panels', 'panels': panels}

    def open_terminal(self, arguments: dict) -> dict:
        """The answer to `open_terminal`: a terminal panel, or a refusal."""
        if self.mode == 'connect-refused':
            return {'status': 'failed', 'code': 'invalid_ssh_route',
                    'message': 'ssh.host is not a host that Horizon may reach'}
        return {'status': 'opened', 'panel_id': 't1'}

    def any_click(self) -> dict:
        """The host's order for a wait without a card: oldest click first."""
        if self.mode in INTRUDER_MODE_LIST:
            return {'status': 'failed', 'code': 'no_cards',
                    'message': 'the caller owns no card in this workspace'}
        for panel_id in self.order:
            panel = self.panels.get(panel_id)
            if panel is not None and panel['clicks']:
                return self.click(panel_id, panel)
        if self.mode == 'closed':
            return {'status': 'failed', 'code': 'panel_unavailable', 'message': 'no card left'}
        return self.no_click()

    def call(self, name: str, arguments: dict) -> dict:
        operation = arguments.get('operation')
        if name != 'block_panel':
            return {'status': 'failed', 'code': 'unknown_tool', 'message': name}
        if self.mode == 'old' and 'document' in arguments:
            extra = rich_fields(arguments['document'])
            if extra:
                return {'status': 'failed', 'code': 'invalid_document',
                        'message': f'{extra[0]} is not a field of this model'}
        if operation == 'next_click':
            timeout = arguments.get('timeout_millis')
            if timeout is not None and (not isinstance(timeout, int) or isinstance(timeout, bool)
                                        or not 1000 <= timeout <= 3600000):
                return {'status': 'failed', 'code': 'invalid_arguments',
                        'message': f'timeout_millis is {timeout!r}, not 1000-3600000'}
            if arguments.get('panel_id') is None:
                if self.mode == 'oldwait':
                    return {'status': 'failed', 'code': 'invalid_arguments',
                            'message': 'next_click needs panel_id on this host'}
                return self.any_click()
        if operation == 'list':
            return self.card_list()
        if operation == 'open_terminal':
            return self.open_terminal(arguments)
        if operation == 'create':
            self.created += 1
            panel_id = f'p{self.created}'
            self.panels[panel_id] = {
                'title': arguments.get('title') or 'card',
                'revision': 1,
                'clicks': [dict(click) for click in MODE_CLICKS.get(self.mode, {}).get(panel_id, [])]}
            self.order.append(panel_id)
            answer = {'status': 'created', 'panel_id': panel_id, 'revision': 1}
            if self.mode == 'closed':
                # The card exists for the create and is gone before the first wait.
                del self.panels[panel_id]
            return answer
        panel = self.panels.get(arguments.get('panel_id'))
        if arguments.get('panel_id') == INTRUDER_PANEL['panel_id']:
            # A card of another agent: readable, never changeable.
            if operation == 'inspect':
                return {'status': 'inspected', 'panel_id': INTRUDER_PANEL['panel_id'],
                        'title': INTRUDER_PANEL['title'], 'revision': INTRUDER_PANEL['revision'],
                        'document': {'components': [{'type': 'heading', 'text': 'build-1'}]},
                        'visible': True, 'agent_present': True, 'owned_by_caller': False,
                        'pending_clicks': 1, 'listening': True}
            return {'status': 'failed', 'code': 'not_owner',
                    'message': 'another agent created this card'}
        if panel is None:
            return {'status': 'failed', 'code': 'panel_unavailable', 'message': 'no such card'}
        if operation == 'update':
            panel['revision'] += 1
            return {'status': 'updated', 'panel_id': arguments['panel_id'], 'revision': panel['revision']}
        if operation == 'close':
            del self.panels[arguments['panel_id']]
            return {'status': 'closed', 'panel_id': arguments['panel_id']}
        if operation == 'next_click':
            if panel['clicks']:
                return self.click(arguments['panel_id'], panel)
            return self.no_click()
        return {'status': 'failed', 'code': 'unsupported', 'message': str(operation)}

    def handle(self, message: dict):
        method = message.get('method')
        request_id = message.get('id')
        if method != 'tools/call':
            self.record({'method': method, 'params': message.get('params')})
        if method == 'initialize':
            return {'jsonrpc': '2.0', 'id': request_id, 'result': {
                'protocolVersion': '2025-06-18',
                'capabilities': {'tools': {}},
                'serverInfo': {'name': 'horizon-browser', 'version': 'fake'},
                'instructions': 'fake server for the card agent self test'}}
        if method == 'tools/list':
            tools = [{'name': 'browser_list', 'inputSchema': {'type': 'object'}}]
            if self.mode != 'notool':
                tools.insert(0, {'name': 'block_panel', 'inputSchema': {'type': 'object'}})
            return {'jsonrpc': '2.0', 'id': request_id, 'result': {'tools': tools}}
        if method == 'tools/call':
            params = message.get('params') or {}
            payload = self.call(params.get('name'), params.get('arguments') or {})
            self.record({'method': 'tools/call', 'params': params, 'answer': payload})
            return {'jsonrpc': '2.0', 'id': request_id, 'result': {
                'content': [{'type': 'text', 'text': json.dumps(payload)}],
                'structuredContent': payload,
                'isError': False}}
        if request_id is None:
            return None
        return {'jsonrpc': '2.0', 'id': request_id,
                'error': {'code': -32601, 'message': f'no method {method}'}}

    def serve(self) -> int:
        for raw in iter(sys.stdin.buffer.readline, b''):
            line = raw.strip()
            if not line:
                continue
            message = json.loads(line)
            reply = self.handle(message)
            if reply is None:
                continue
            sys.stdout.write(json.dumps(reply) + '\n')
            sys.stdout.flush()
        return 0


# --------------------------------------------------------------------------
# The test driver.
# --------------------------------------------------------------------------

class AgentRun:
    """One card_agent.py child, its stdout, and the fake server's request log."""

    def __init__(self, test: unittest.TestCase, mode: str, extra_args=(), env=None):
        self.test = test
        self.log_path = os.path.join(HERE, f'.fake-mcp-{os.getpid()}-{int(time.time() * 1000)}.jsonl')
        test.addCleanup(self._remove_log)
        command = f'{sys.executable} {shlex.quote(THIS_FILE)} --serve-fake'
        self.env = dict(os.environ)
        self.env.update({'FAKE_MCP_LOG': self.log_path, 'FAKE_MCP_MODE': mode})
        # The Connect button reads these two from the environment; a test that
        # does not set them proves the run of a panel with no host configured,
        # whatever this shell holds.
        for name in (SSH_HOST_ENV, SSH_COMMAND_ENV):
            self.env.pop(name, None)
        self.env.update(env or {})
        self.proc = subprocess.Popen(
            [sys.executable, AGENT, '--mcp-command', command, *extra_args],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env=self.env, cwd=HERE)
        self.lines: queue.Queue = queue.Queue()
        threading.Thread(target=self._pump_stdout, daemon=True).start()
        self.stdout_text = ''

    def _remove_log(self) -> None:
        try:
            os.unlink(self.log_path)
        except FileNotFoundError:
            pass

    def _pump_stdout(self) -> None:
        for raw in iter(self.proc.stdout.readline, b''):
            self.lines.put(raw.decode('utf-8', 'replace').rstrip('\r\n'))

    def finish(self, timeout: float = 40.0) -> tuple[int, str, str]:
        try:
            code = self.proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
            self.test.fail(f'the card agent did not exit; stdout so far: {self.snapshot()}')
        errors = self.proc.stderr.read().decode('utf-8', 'replace')
        for stream in (self.proc.stdin, self.proc.stdout, self.proc.stderr):
            try:
                if stream is not None:
                    stream.close()
            except (OSError, ValueError):
                pass
        return code, self.snapshot(), errors

    def snapshot(self) -> str:
        drained = []
        while True:
            try:
                drained.append(self.lines.get_nowait())
            except queue.Empty:
                break
        self.stdout_text += ''.join(f'{line}\n' for line in drained)
        return self.stdout_text

    def requests(self) -> list[dict]:
        if not os.path.exists(self.log_path):
            return []
        with open(self.log_path, encoding='utf-8') as handle:
            return [json.loads(line) for line in handle if line.strip()]

    def wait_for(self, predicate, timeout: float = 15.0) -> list[dict]:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            entries = self.requests()
            if predicate(entries):
                return entries
            if self.proc.poll() is not None:
                return entries
            time.sleep(0.05)
        return self.requests()


def shlex_quote(text: str) -> str:
    import shlex
    return shlex.quote(text)

def calls(entries: list[dict]) -> list[dict]:
    """The block_panel `tools/call` entries, with their arguments and answer."""
    found = []
    for entry in entries:
        if entry.get('method') != 'tools/call':
            continue
        params = entry.get('params') or {}
        if params.get('name') != 'block_panel':
            continue
        found.append({'arguments': params.get('arguments') or {}, 'answer': entry.get('answer') or {}})
    return found


def operations(entries: list[dict]) -> list[tuple[str, str]]:
    return [(call['arguments'].get('operation'), call['answer'].get('status', '')) for call in calls(entries)]


def load_agent():
    """card_agent.py itself, to test its documents, names and estimate."""
    return card_agent


class StubClient:
    """A block_panel client that answers from a list and records each request."""

    def __init__(self, answers):
        self.answers = list(answers)
        self.calls = []

    def call_tool(self, name, arguments, timeout=None):
        self.calls.append((name, arguments))
        if not self.answers:
            raise AssertionError(f'the agent asked for more than the test answers: {arguments}')
        return self.answers.pop(0)


def unsafe_lines(stdout: str, module) -> list[str]:
    """The printed lines that Horizon could read as an agent working line."""
    unsafe = []
    for line in stdout.splitlines():
        if not line.strip():
            continue
        if module.looks_like_working_line(line):
            unsafe.append(line)
            continue
        if any(word in line for word in module.WORKING_KEYWORDS + module.WORKING_HINTS):
            unsafe.append(line)
    return unsafe


class CardAgentTest(unittest.TestCase):
    def assert_status_scrape_safe(self, stdout: str) -> None:
        """No printed line may read as a working line of the agent panel."""
        module = load_agent()
        printed = [line for line in stdout.splitlines() if line.strip()]
        self.assertGreater(len(printed), 5, f'the agent must log its events:\n{stdout}')
        self.assertEqual(unsafe_lines(stdout, module), [],
                         f'every printed line must stay clear of the status scrape:\n{stdout}')

    def wait_for_line(self, run: 'AgentRun', needle: str, timeout: float = 20.0) -> str:
        """The stdout of the agent, once it holds a line with this text."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            text = run.snapshot()
            if needle in text:
                return text
            time.sleep(0.05)
        self.fail(f'the agent did not print {needle!r}; stdout so far:\n{run.snapshot()}')

    def test_handshake_creates_cards_and_reacts_to_clicks(self):
        run = AgentRun(self, 'full', ('--scenario', 'all'))
        code, stdout, stderr = run.finish()
        self.assertEqual(code, 0, f'card agent exit {code}\nstdout:\n{stdout}\nstderr:\n{stderr}')
        entries = run.requests()

        methods = [entry.get('method') for entry in entries]
        self.assertIn('initialize', methods)
        self.assertIn('tools/list', methods)
        self.assertIn('notifications/initialized', methods)
        self.assertLess(methods.index('initialize'), methods.index('tools/list'),
                        f'the handshake must come first: {methods}')

        creates = [call for call in calls(entries) if call['arguments'].get('operation') == 'create']
        self.assertEqual(len(creates), 3, f'expected three creates, log:\n{json.dumps(entries, indent=1)}')
        titles = []
        for call in creates:
            arguments = call['arguments']
            self.assertEqual(set(arguments), {'operation', 'title', 'document'})
            document = arguments['document']
            validate_document(document)
            titles.append(arguments['title'])
            self.assertEqual(call['answer']['status'], 'created')
        self.assertEqual(titles, ['Machine \u00b7 build-1', 'New machine', 'Browser machine'])

        # One wait on every card: it names no card, and it asks for the default 60 s.
        waits = [call for call in calls(entries)
                 if call['arguments'].get('operation') == 'next_click']
        self.assertTrue(waits, f'the agent must read clicks: {operations(entries)}')
        for call in waits:
            self.assertNotIn('panel_id', call['arguments'],
                             'continuous mode waits on every card of the agent')
            self.assertEqual(call['arguments']['timeout_millis'], 60_000,
                             'every wait asks for the wait of --click-timeout')
        answered = [(call['answer'].get('click') or {}).get('panel_id') for call in waits]
        self.assertEqual(answered, ['p1', 'p1', 'p2', 'p2', 'p3', 'p3'],
                         f'each wait answers the click of the card that has one: {answered}')

        flow = [(call['arguments'].get('panel_id'), call['arguments'].get('operation'),
                 call['answer'].get('status')) for call in calls(entries)]
        self.assertEqual(flow, [
            (None, 'create', 'created'),
            (None, 'create', 'created'),
            (None, 'create', 'created'),
            (None, 'next_click', 'clicked'), ('p1', 'update', 'updated'),
            (None, 'next_click', 'clicked'), ('p1', 'close', 'closed'),
            (None, 'next_click', 'clicked'), ('p2', 'update', 'updated'),
            (None, 'next_click', 'clicked'), ('p2', 'close', 'closed'),
            (None, 'next_click', 'clicked'), ('p3', 'update', 'updated'),
            (None, 'next_click', 'clicked'), ('p3', 'close', 'closed'),
        ], f'one wait on every card per click, dispatched to the card it names: {flow}')

        updates = {}
        for call in calls(entries):
            arguments = call['arguments']
            if arguments.get('operation') != 'update':
                continue
            validate_document(arguments['document'])
            updates[arguments['panel_id']] = arguments['document']
        self.assertEqual(sorted(updates), ['p1', 'p2', 'p3'], f'one update per card: {sorted(updates)}')

        # The machine card: Stop leads to the stopped document with Start in place.
        self.assertEqual(badge_texts(updates['p1']), ['Stopped'])
        button_ids = [item['id'] for component in updates['p1']['components']
                      if component['type'] == 'buttons' for item in component['items']]
        self.assertEqual(button_ids, ['connect', 'start', 'delete'])

        # The default row is the choice that the button below reports.
        created = {call['arguments']['title']: call['arguments']['document'] for call in creates}
        self.assertEqual(default_rows(created['New machine']), ['hetzner'])
        self.assertEqual(default_rows(created['Browser machine']), ['chrome'])
        self.assertIn('You picked Hetzner.', document_text(updates['p2']))
        self.assertIn('You picked Chrome on this Mac.', document_text(updates['p3']))
        self.assertIn('choose: continue with Hetzner', stdout)
        self.assertIn('browser: setup with Chrome on this Mac', stdout)

        # Every printed line is safe for the status scrape.
        self.assert_status_scrape_safe(stdout)

    def test_once_listens_only_after_the_wake_line(self):
        run = AgentRun(self, 'wake', ('--scenario', 'machine', '--listen', 'once'))
        entries = run.wait_for(lambda log: any(call['arguments'].get('operation') == 'create'
                                              for call in calls(log)))
        creates = [call for call in calls(entries) if call['arguments'].get('operation') == 'create']
        self.assertEqual(len(creates), 1, f'the machine card must be created first: {entries}')

        time.sleep(0.4)
        self.assertEqual(operations(run.requests()), [('create', 'created')],
                         'no next_click may run before a wake line arrives')

        self.assertIsNotNone(run.proc.stdin)
        # The wake line arrives without a newline: the terminal driver turns its
        # CR into the line end, so a lone CR proves that path as well.
        run.proc.stdin.write(f'{WAKE_LINE}\r'.encode('utf-8'))
        run.proc.stdin.flush()

        entries = run.wait_for(lambda log: ('next_click', 'clicked') in operations(log))
        self.assertIn(('next_click', 'clicked'), operations(entries),
                      f'the wake line must cause a next_click: {operations(entries)}')
        waits = [call['arguments'] for call in calls(entries)
                 if call['arguments'].get('operation') == 'next_click']
        self.assertNotIn('panel_id', waits[0],
                         'a wake line reads every card of the agent at once')
        self.assertEqual(waits[0]['timeout_millis'], 1_000,
                         'the script reads at once, it does not hold the host for a minute')
        entries = run.wait_for(lambda log: any(call['arguments'].get('operation') == 'update'
                                              for call in calls(log)))
        self.assertIn(('update', 'updated'), operations(entries), f'expected an update: {operations(entries)}')

        run.proc.stdin.close()
        code, stdout, stderr = run.finish()
        self.assertEqual(code, 0, f'exit {code}\nstdout:\n{stdout}\nstderr:\n{stderr}')
        self.assertIn('wake line seen', stdout)
        self.assertIn('stdin closed', stdout)
        self.assert_status_scrape_safe(stdout)

    def test_sigterm_ends_the_agent_and_its_server(self):
        run = AgentRun(self, 'wake', ('--scenario', 'machine'))
        pid = None
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline and pid is None:
            match = re.search(r'MCP server started, pid (\d+)', run.snapshot())
            if match:
                pid = int(match.group(1))
            else:
                time.sleep(0.05)
        self.assertIsNotNone(pid, f'the agent must log the server pid:\n{run.snapshot()}')
        run.wait_for(lambda log: ('next_click', 'clicked') in operations(log))
        run.proc.send_signal(signal.SIGTERM)
        code, stdout, stderr = run.finish()
        self.assertEqual(code, 0, f'exit {code}\nstdout:\n{stdout}\nstderr:\n{stderr}')
        self.assertIn('signal received', stdout)
        for _ in range(100):
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                break
            time.sleep(0.05)
        else:
            self.fail(f'the MCP server {pid} outlived the agent')

    def test_missing_block_panel_is_a_clear_failure(self):
        run = AgentRun(self, 'notool', ('--scenario', 'machine'))
        code, stdout, stderr = run.finish()
        self.assertEqual(code, 1, f'exit {code} (a server without the tool is a failure)')
        self.assertIn('block_panel', stderr, f'a clear message on stderr:\n{stderr}')
        self.assertNotIn('created card', stdout, 'no card may be created without the tool')
        self.assertEqual(calls(run.requests()), [])

    def test_plain_fallback_for_an_older_host(self):
        run = AgentRun(self, 'old', ('--scenario', 'machine'))
        code, stdout, stderr = run.finish()
        self.assertEqual(code, 0, f'exit {code}\nstdout:\n{stdout}\nstderr:\n{stderr}')
        creates = [call for call in calls(run.requests()) if call['arguments'].get('operation') == 'create']
        self.assertEqual(len(creates), 2, f'one refused create and one plain create: {creates}')
        self.assertEqual(creates[0]['answer']['status'], 'failed')
        self.assertEqual(creates[0]['answer']['code'], 'invalid_document')
        self.assertTrue(rich_fields(creates[0]['arguments']['document']))
        self.assertEqual(creates[1]['answer']['status'], 'created')
        self.assertEqual(rich_fields(creates[1]['arguments']['document']), [],
                         'the fallback document may use no field of the newer model')
        validate_document(creates[1]['arguments']['document'])
        self.assertIn('the host refused the rich document', stdout)
        self.assertIn('documents use the plain model', stdout)

    def test_a_host_that_refuses_the_wait_on_every_card_gets_one_wait_per_card(self):
        run = AgentRun(self, 'oldwait', ('--scenario', 'machine'))
        code, stdout, stderr = run.finish()
        self.assertEqual(code, 0, f'exit {code}\nstdout:\n{stdout}\nstderr:\n{stderr}')
        waits = [call for call in calls(run.requests())
                 if call['arguments'].get('operation') == 'next_click']
        self.assertTrue(waits, 'the fallback must still read clicks')
        self.assertNotIn('panel_id', waits[0]['arguments'], 'the wait on every card comes first')
        self.assertEqual(waits[0]['answer']['status'], 'failed')
        self.assertEqual(waits[0]['answer']['code'], 'invalid_arguments')
        self.assertGreater(len(waits), 1, f'the rotation must follow the refusal: {waits}')
        for call in waits[1:]:
            self.assertEqual(call['arguments']['panel_id'], 'p1',
                             'every later wait names one card')
        self.assertIn('refused the wait on every card', stdout)
        self.assertIn('listen on one card at a time instead', stdout)
        self.assertIn(('close', 'closed'), operations(run.requests()))
        self.assertIn('every card is closed', stdout)
        self.assert_status_scrape_safe(stdout)

    def test_a_person_who_closed_the_card_ends_the_loop(self):
        run = AgentRun(self, 'closed', ('--scenario', 'machine'))
        code, stdout, stderr = run.finish()
        self.assertEqual(code, 0, f'exit {code}\nstdout:\n{stdout}\nstderr:\n{stderr}')
        self.assertIn('the host reports no reachable card any more', stdout)
        self.assertIn('machine: card is gone (panel_unavailable)', stdout)
        self.assertIn('every card is closed', stdout)
        self.assert_status_scrape_safe(stdout)

    def test_tall_scenario_overflows_the_card_height_clamp(self):
        module = load_agent()
        document = module.tall_document()
        validate_document(document)
        self.assertEqual([component['type'] for component in document['components']],
                         ['heading', 'kv', 'text', 'list', 'buttons'])
        self.assertEqual(len(document['components'][1]['rows']), 12, 'the kv box holds twelve pairs')
        self.assertEqual(document['components'][2]['text'].count('.'), 3,
                         'the text holds three sentences')
        self.assertEqual(len(document['components'][3]['rows']), 8, 'the list holds eight rows')
        self.assertEqual([item['id'] for item in document['components'][4]['items']], ['ok', 'close'])
        height = module.estimated_card_height(document)
        self.assertEqual(module.CARD_MAX_HEIGHT, 900.0)
        self.assertGreater(height, module.CARD_MAX_HEIGHT,
                           f'the tall card needs {height:.0f} points, above the 900 point clamp')
        # The plain fallback of the same card is drawn by an older host as well.
        plain = module.tall_document(None, False)
        validate_document(plain)
        self.assertEqual(rich_fields(plain), [])
        self.assertGreater(module.estimated_card_height(plain), module.CARD_MAX_HEIGHT)

        run = AgentRun(self, 'tall', ('--scenario', 'tall'))
        code, stdout, stderr = run.finish()
        self.assertEqual(code, 0, f'exit {code}\nstdout:\n{stdout}\nstderr:\n{stderr}')
        entries = run.requests()
        creates = [call for call in calls(entries) if call['arguments'].get('operation') == 'create']
        self.assertEqual(len(creates), 1, f'--scenario tall creates one card: {operations(entries)}')
        self.assertEqual(creates[0]['arguments']['title'], module.TALL_TITLE)
        self.assertGreater(module.estimated_card_height(creates[0]['arguments']['document']),
                           module.CARD_MAX_HEIGHT)
        self.assertEqual([operation for operation, _status in operations(entries)],
                         ['create', 'next_click', 'update', 'next_click', 'close'])
        self.assertIn('tall: the body needs', stdout)
        self.assertIn('above the 900 point clamp', stdout)
        self.assertIn('tall: updated card p1 rev 2', stdout)
        self.assertIn('tall: closed card p1', stdout)
        self.assertIn('every card is closed', stdout)
        update = [call['arguments'] for call in calls(entries)
                  if call['arguments'].get('operation') == 'update'][0]
        self.assertIn('OK was pressed at', document_text(update['document']))
        self.assert_status_scrape_safe(stdout)

    def test_all_plus_tall_adds_the_tall_card_to_the_three(self):
        run = AgentRun(self, 'alltall', ('--scenario', 'all+tall'))
        code, stdout, stderr = run.finish()
        self.assertEqual(code, 0, f'exit {code}\nstdout:\n{stdout}\nstderr:\n{stderr}')
        entries = run.requests()
        creates = [call for call in calls(entries) if call['arguments'].get('operation') == 'create']
        self.assertEqual([call['arguments']['title'] for call in creates], [
            'Machine \u00b7 build-1', 'New machine', 'Browser machine', 'Tall card \u00b7 overflow'],
            f'--scenario all+tall creates the tall card last: {operations(entries)}')
        for call in creates:
            validate_document(call['arguments']['document'])
        self.assertEqual([operation for operation, _status in operations(entries)], [
            'create', 'create', 'create', 'create',
            'next_click', 'update', 'next_click', 'close',
            'next_click', 'update', 'next_click', 'close',
            'next_click', 'update', 'next_click', 'close',
            'next_click', 'update', 'next_click', 'close',
        ], f'one loop drives every card of all+tall: {operations(entries)}')
        waited = [call['arguments'] for call in calls(entries)
                  if call['arguments'].get('operation') == 'next_click']
        self.assertTrue(all('panel_id' not in arguments for arguments in waited),
                        'one wait on every card serves four cards as well')
        self.assertIn('tall: updated card p4 rev 2', stdout)
        self.assertIn('every card is closed', stdout)
        self.assert_status_scrape_safe(stdout)

    def test_connect_opens_a_terminal_and_says_so_on_the_card(self):
        run = AgentRun(self, 'connect', ('--scenario', 'machine'),
                       env={SSH_HOST_ENV: 'lab-1', SSH_COMMAND_ENV: 'tmux new -A -s build'})
        code, stdout, stderr = run.finish()
        self.assertEqual(code, 0, f'exit {code}\nstdout:\n{stdout}\nstderr:\n{stderr}')
        entries = run.requests()
        self.assertEqual([operation for operation, _status in operations(entries)], [
            'create', 'next_click', 'open_terminal', 'update', 'next_click', 'close'],
            f'Connect opens the terminal and only then updates the card: {operations(entries)}')
        opened = [call for call in calls(entries)
                  if call['arguments'].get('operation') == 'open_terminal'][0]
        self.assertEqual(opened['arguments'], {
            'operation': 'open_terminal', 'ssh': {'host': 'lab-1'},
            'remote_command': 'tmux new -A -s build', 'title': 'build-1 session'},
            'the host builds the ssh command line; the agent names only the route')
        self.assertEqual(opened['answer'], {'status': 'opened', 'panel_id': 't1'})

        update = [call for call in calls(entries)
                  if call['arguments'].get('operation') == 'update'][0]
        validate_document(update['arguments']['document'])
        text = document_text(update['arguments']['document'])
        self.assertRegex(text, r'A terminal opened beside this panel at \d\d:\d\d:\d\d\.',
                         f'the card says when the terminal opened: {text}')
        self.assertNotIn('Connect was pressed at', text)
        self.assertIn('connect: opened terminal t1 on lab-1', stdout)
        self.assertIn('(build-1 session)', stdout)
        self.assert_status_scrape_safe(stdout)

    def test_connect_without_a_host_opens_nothing_and_says_so(self):
        run = AgentRun(self, 'connect', ('--scenario', 'machine'))
        code, stdout, stderr = run.finish()
        self.assertEqual(code, 0, f'exit {code}\nstdout:\n{stdout}\nstderr:\n{stderr}')
        entries = run.requests()
        self.assertEqual([operation for operation, _status in operations(entries)],
                         ['create', 'next_click', 'update', 'next_click', 'close'],
                         f'without a host the agent asks for no terminal: {operations(entries)}')
        update = [call for call in calls(entries)
                  if call['arguments'].get('operation') == 'update'][0]
        validate_document(update['arguments']['document'])
        text = document_text(update['arguments']['document'])
        self.assertIn('No SSH host is set in CARD_AGENT_SSH_HOST', text)
        self.assertIn('machine: connect: No SSH host is set in CARD_AGENT_SSH_HOST', stdout)
        self.assert_status_scrape_safe(stdout)

    def test_connect_refusal_shows_the_code_on_the_card(self):
        run = AgentRun(self, 'connect-refused', ('--scenario', 'machine'),
                       env={SSH_HOST_ENV: 'lab-1'})
        code, stdout, stderr = run.finish()
        self.assertEqual(code, 0, f'a refused terminal must not end the agent: {stdout}\n{stderr}')
        entries = run.requests()
        opened = [call for call in calls(entries)
                  if call['arguments'].get('operation') == 'open_terminal'][0]
        self.assertNotIn('remote_command', opened['arguments'],
                         'no line runs on the host unless CARD_AGENT_SSH_COMMAND is set')
        self.assertEqual(opened['answer']['code'], 'invalid_ssh_route')
        update = [call for call in calls(entries)
                  if call['arguments'].get('operation') == 'update'][0]
        validate_document(update['arguments']['document'])
        text = document_text(update['arguments']['document'])
        self.assertIn('The host refused the terminal: invalid_ssh_route.', text)
        self.assertIn('connect: the host refused the terminal with invalid_ssh_route', stdout)
        self.assertIn(('close', 'closed'), operations(entries),
                      'the person can still close the card after a refusal')
        self.assert_status_scrape_safe(stdout)

    def test_intruder_reads_a_foreign_card_and_is_refused_every_change(self):
        run = AgentRun(self, 'intruder-empty', ('--scenario', 'intruder'))
        stdout = self.wait_for_line(run, 'intruder check done')
        self.assertIsNone(run.proc.poll(),
                          f'the panel must stay open after the check:\n{stdout}')
        entries = run.requests()
        self.assertEqual([operation for operation, _status in operations(entries)],
                         ['list', 'list', 'inspect', 'update', 'close', 'next_click'],
                         f'the intruder reads and then tries to change the card: '
                         f'{operations(entries)}')
        self.assertEqual([status for _operation, status in operations(entries)],
                         ['panels', 'panels', 'inspected', 'failed', 'failed', 'failed'])
        self.assertEqual([call['answer'].get('code') for call in calls(entries)],
                         [None, None, None, 'not_owner', 'not_owner', 'no_cards'])
        self.assertIn('intruder: list answered 0 card(s) and none is foreign', stdout)
        self.assertIn('intruder: card p9 inspect answered inspected', stdout)
        self.assertIn('intruder: card p9 update answered not_owner', stdout)
        self.assertIn('intruder: card p9 close answered not_owner', stdout)
        self.assertIn('intruder: the wait on every card answered no_cards', stdout)
        self.assertIn('card-agent: intruder check done', stdout)

        # No card of its own, and the change it tried carries one heading only.
        self.assertNotIn(('create', 'created'), operations(entries))
        update = [call for call in calls(entries)
                  if call['arguments'].get('operation') == 'update'][0]
        validate_document(update['arguments']['document'])
        self.assertEqual([component['type'] for component
                          in update['arguments']['document']['components']], ['heading'])
        wait = [call for call in calls(entries)
                if call['arguments'].get('operation') == 'next_click'][0]
        self.assertNotIn('panel_id', wait['arguments'], 'the intruder waits on every card')
        self.assertEqual(wait['arguments']['timeout_millis'], 1_000)

        run.proc.stdin.close()
        code, rest, stderr = run.finish()
        self.assertEqual(code, 0, f'exit {code}\nstdout:\n{stdout}{rest}\nstderr:\n{stderr}')
        self.assertIn('stdin closed', rest)
        self.assert_status_scrape_safe(stdout + rest)

    def test_the_intruder_waits_for_a_card_and_gives_up_cleanly(self):
        module = load_agent()
        module.INTRUDER_LIST_WAIT_SECONDS = 0.01
        buffer = io.StringIO()
        client = StubClient([{'status': 'panels', 'panels': []},
                             {'status': 'panels', 'panels': [dict(INTRUDER_PANEL)]}])
        agent = module.CardAgent(client, 'continuous', 60.0)
        with contextlib.redirect_stdout(buffer):
            self.assertEqual(agent.foreign_cards(), [INTRUDER_PANEL])
        self.assertEqual([arguments['operation'] for _name, arguments in client.calls],
                         ['list', 'list'], 'an empty list is read again')
        self.assertIn('look again in', buffer.getvalue())

        module.INTRUDER_LIST_ATTEMPTS = 3
        buffer = io.StringIO()
        client = StubClient([{'status': 'panels', 'panels': []}] * 3)
        agent = module.CardAgent(client, 'continuous', 60.0)
        with contextlib.redirect_stdout(buffer):
            self.assertEqual(agent.foreign_cards(), [], 'no card leaves nothing to read')
        self.assertEqual(len(client.calls), 3, 'the look ends after the last attempt')
        self.assertIn('no card of another agent appeared in 3 looks', buffer.getvalue())

        client = StubClient([{'status': 'failed', 'code': 'caller_unavailable', 'message': 'x'}])
        agent = module.CardAgent(client, 'continuous', 60.0)
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(module.Fatal):
            agent.foreign_cards()

        self.assertEqual(module.scenario_keys('intruder'), (),
                         'the intruder scenario creates no card')
        self.assertNotIn('intruder', module.SCENARIOS)
        args, extra = module.parse_args(['--scenario', 'intruder', '--mcp-command', 'true'])
        self.assertEqual((args.scenario, extra), ('intruder', []))

    def test_the_any_card_wait_reads_its_outcomes(self):
        module = load_agent()
        agent = module.CardAgent(None, 'continuous', 60.0)
        card = module.MachineCard('machine', module.MACHINE_TITLE)
        card.panel_id = 'p1'
        agent.cards.append(card)
        unknown = {'status': 'clicked',
                   'click': {'panel_id': 'p9', 'component_id': 'stop'}}
        self.assertEqual(agent.handle_any(unknown, 'unused'), 'keep',
                         'a click of another card never acts on this one')
        for code, wanted in (('request_expired', 'keep'), ('host_timeout', 'keep'),
                             ('no_cards', 'stop'),
                             ('panel_unavailable', 'fallback'), ('not_block_panel', 'fallback'),
                             ('invalid_arguments', 'fallback'), ('unsupported', 'fallback')):
            self.assertEqual(agent.handle_any({'status': 'failed', 'code': code, 'message': 'x'}, 'unused'),
                             wanted, f'{code} must answer {wanted}')
        for code in ('caller_unavailable', 'wrong_host'):
            with self.assertRaises(module.Fatal):
                agent.handle_any({'status': 'failed', 'code': code, 'message': 'x'}, 'unused')
        self.assertEqual(agent.handle_any({'status': 'timed_out', 'timed_out': True}, 'unused'), 'keep')

    def test_documents_follow_the_model_rules(self):
        module = load_agent()
        for rich in (True, False):
            documents = (
                module.machine_document(rich=rich),
                module.machine_document(True, rich=rich),
                module.machine_document(False, '18:42:07', rich=rich),
                module.choose_document(rich=rich),
                module.choose_document('Hetzner', 1, rich),
                module.choose_document('Azure', 2, rich),
                module.browser_document(rich=rich),
                module.browser_document('Chrome on this Mac', 1, rich),
                module.browser_document('Chromium in Docker', 2, rich),
                module.tall_document(None, rich),
                module.tall_document('18:42:07', rich),
                module.machine_document(False, None, rich,
                                        'A terminal opened beside this panel at 18:42:07.'),
                module.machine_document(True, None, rich,
                                        'The host refused the terminal: not_owner.'),
            )
            for document in documents:
                validate_document(document)
                if not rich:
                    self.assertEqual(rich_fields(document), [],
                                     'the plain document may use no newer field')
        # The default row of each single-selection list, and none in the fallback.
        self.assertEqual(default_rows(module.choose_document()), ['hetzner'])
        self.assertEqual(default_rows(module.browser_document()), ['chrome'])
        for document in (module.choose_document(rich=False), module.browser_document(rich=False),
                         module.tall_document(rich=False)):
            self.assertEqual(default_rows(document), [], 'the plain model has no default row')
        # A second selected row of one list is what Horizon refuses.
        broken = module.choose_document()
        rows = next(component for component in broken['components']
                    if component['type'] == 'list')['rows']
        rows[1]['selected'] = True
        with self.assertRaises(DocumentError):
            validate_document(broken)
        # `--scenario all` leaves the tall card out; `all+tall` adds it.
        self.assertEqual(module.scenario_keys('all'), ('machine', 'choose', 'browser'))
        self.assertEqual(module.scenario_keys('all+tall'),
                         ('machine', 'choose', 'browser', 'tall'))
        self.assertEqual(module.scenario_keys('tall'), ('tall',))
        self.assertNotIn('tall', module.scenario_keys('all'))
        self.assertEqual(module.SCENARIOS['tall']().title, module.TALL_TITLE)
        # The two names that the Connect button reads, and the answer code reader.
        self.assertEqual((module.SSH_HOST_ENV, module.SSH_COMMAND_ENV),
                         (SSH_HOST_ENV, SSH_COMMAND_ENV))
        self.assertEqual(module.answer_code({'status': 'opened', 'panel_id': 't1'}), 'opened')
        self.assertEqual(module.answer_code(
            {'status': 'failed', 'code': 'not_owner', 'message': 'x'}), 'not_owner')
        args, extra = module.parse_args(['--scenario', 'all+tall', '--mcp-command', 'true'])
        self.assertEqual(args.scenario, 'all+tall')
        self.assertEqual(extra, [])
        self.assertFalse(module.looks_like_working_line(WAKE_LINE))
        self.assertFalse(module.looks_like_working_line('card-agent: machine: stop clicked'))
        # The mirror must still catch what Horizon catches.
        self.assertTrue(module.looks_like_working_line('\u280b Working... (esc to interrupt)'))


if __name__ == '__main__':
    if '--serve-fake' in sys.argv[1:]:
        log = os.environ.get('FAKE_MCP_LOG')
        if not log:
            print('FAKE_MCP_LOG is required', file=sys.stderr)
            sys.exit(2)
        sys.exit(FakeServer(log, os.environ.get('FAKE_MCP_MODE', 'full')).serve())
    unittest.main(verbosity=2)
