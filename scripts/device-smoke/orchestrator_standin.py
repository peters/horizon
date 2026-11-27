#!/usr/bin/env python3
"""The stand-in of Claude Code for a conversation of the orchestrator dock.

Horizon starts this file as the program of one dock conversation when
`HORIZON_ORCHESTRATOR_PROGRAM` names it, with the arguments of Claude Code, and
the file ignores those arguments. It reads one JSON line of the person for each
turn on the standard input, and it answers in the line format that the codec of
`crates/horizon-core/src/agent_bridge/codec.rs` reads: the init line of the
session, the text of the agent, an optional tool call with its result, an
optional long answer, the state of the limit of the plan, and the result of the
turn with its cost. A turn whose text holds `slow` waits two seconds before its
answer, so a run shows the working state of the dock.

The words `tool`, `long`, `slow` and `card` choose what a turn answers. A turn
whose text holds `card` draws one card in the workspace of the conversation
through a real `horizon --browser-mcp` child, with the identity that Horizon gave
this program, and then keeps one `next_click` open for that card in a thread of
its own. A press of the person answers that wait, and the thread prints the text
of the agent and the result of that turn. The program ends when the standard
input closes, as an agent program of a conversation does.

Usage, inside the isolated fixture:

    HORIZON_ORCHESTRATOR_DOCK=1 \
    HORIZON_ORCHESTRATOR_PROGRAM=<repo>/scripts/device-smoke/orchestrator_standin.py \
        python3 scripts/device-smoke/serve.py --native-view --state <evidence>/dock
"""
import json
import os
import shlex
import sys
import threading
import time
import uuid

# The MCP client of this program is the one of card_agent.py, which the fixture
# copies beside this file, so that import leaves no __pycache__.
sys.dont_write_bytecode = True

from card_agent import Fatal, StdioMcpClient, one_line

MODEL = 'claude-stand-in'
COST_USD = 0.01
DURATION_MS = 12
SLOW_SECONDS = 2
LONG_LINES = 40
TURN_WORDS = {'tool': 'tool', 'long': 'long', 'slow': 'slow', 'card': 'card'}
TOOL_NAME = 'Bash'
TOOL_COMMAND = 'echo stand-in'
TOOL_RESULT = 'stand-in'
# The card of a turn that holds `card`: one heading with a subtitle, one text and
# two buttons, so the person has a primary action and a way out.
CARD_HEADING = 'Plan'
CARD_SUBTITLE = 'drawn by the stand-in orchestrator'
CARD_TEXT = 'This card comes from the orchestrator of this workspace, not from an agent panel.'
CARD_BUTTONS = ({'id': 'go', 'label': 'Go', 'tone': 'primary'},
                {'id': 'cancel', 'label': 'Cancel', 'tone': 'danger'})
# How long the card waits for one press: five minutes, the default of the tool.
CLICK_TIMEOUT_MILLIS = 300_000
CARD_TIMEOUT_SECONDS = 30.0
CARD_WAIT_TIMEOUT_SECONDS = CLICK_TIMEOUT_MILLIS / 1000.0 + 60.0
# The variable that Horizon sets in every program it starts: the program that runs
# this Horizon, which serves the MCP tools of the card.
EXECUTABLE_ENV = 'HORIZON_EXECUTABLE'
MCP_ARGUMENTS = ('--browser-mcp',)
# One line of JSON leaves this program at a time, from the turn and from the
# thread of a card.
WRITE_LOCK = threading.Lock()


def line(record):
    """Writes one JSON line of the agent, flushed for the reader of Horizon."""
    with WRITE_LOCK:
        sys.stdout.write(json.dumps(record) + '\n')
        sys.stdout.flush()


def init(session):
    """The line that names the session at the start of a turn."""
    line({'type': 'system', 'subtype': 'init', 'session_id': session, 'cwd': os.getcwd(),
          'model': MODEL, 'tools': [TOOL_NAME], 'permissionMode': 'auto'})


def text(session, message_id, words):
    """One text of the agent, as one message of the line format."""
    line({'type': 'assistant', 'session_id': session, 'message': {
        'id': message_id, 'type': 'message', 'role': 'assistant', 'model': MODEL,
        'content': [{'type': 'text', 'text': words}], 'stop_reason': None, 'usage': {}}})


def tool(session, message_id):
    """One call of `Bash` and its result, as two lines of the line format."""
    tool_id = f'toolu-{message_id}'
    line({'type': 'assistant', 'session_id': session, 'message': {
        'id': f'tool-{message_id}', 'type': 'message', 'role': 'assistant', 'model': MODEL,
        'content': [{'type': 'tool_use', 'id': tool_id, 'name': TOOL_NAME,
                     'input': {'command': TOOL_COMMAND, 'description': 'Print the stand-in'}}],
        'stop_reason': None, 'usage': {}}})
    line({'type': 'user', 'session_id': session, 'message': {'role': 'user', 'content': [
        {'type': 'tool_result', 'tool_use_id': tool_id, 'content': TOOL_RESULT, 'is_error': False}]}})


def long_answer(session, message_id):
    """One text of forty numbered lines, for the scroll of the dock."""
    text(session, f'{message_id}-long',
         '\n'.join(f'{number}. a line of a long answer' for number in range(1, LONG_LINES + 1)))


def limit(session):
    """The state of the window of the account, which every turn of Claude Code reports."""
    line({'type': 'rate_limit_event', 'session_id': session, 'rate_limit_info': {
        'status': 'allowed', 'rateLimitType': 'five_hour', 'resetsAt': int(time.time()) + 3600,
        'overageStatus': 'rejected', 'isUsingOverage': False}})


def result(session, words, duration_ms):
    """The result of one turn, with the cost of the turn."""
    line({'type': 'result', 'subtype': 'success', 'session_id': session, 'is_error': False,
          'duration_ms': duration_ms, 'duration_api_ms': duration_ms, 'num_turns': 1,
          'result': words, 'total_cost_usd': COST_USD,
          'model_usage_since_last_result': {MODEL: {'costUSD': COST_USD}},
          'permission_denials': [], 'terminal_reason': 'completed'})


def words_of(request):
    """The text of one line of the person, or an empty text."""
    try:
        record = json.loads(request)
    except ValueError:
        return ''
    for block in record.get('message', {}).get('content', []):
        if block.get('type') == 'text':
            return block.get('text', '')
    return ''


def card_document():
    """The document of the one card of a `card` turn."""
    return {'components': [
        {'type': 'heading', 'text': CARD_HEADING, 'subtitle': CARD_SUBTITLE},
        {'type': 'text', 'text': CARD_TEXT},
        {'type': 'buttons', 'items': [dict(button) for button in CARD_BUTTONS]},
    ]}


def mcp_command():
    """The command line of the MCP server of this Horizon, or an empty text.

    Horizon sets `HORIZON_EXECUTABLE` in every program it starts, and the server
    reads the actor and the host instance of this program from the environment
    that it inherits.
    """
    executable = os.environ.get(EXECUTABLE_ENV, '').strip()
    if not executable:
        return ''
    return shlex.join([executable, *MCP_ARGUMENTS])


def create_card(command):
    """Draws the one card of a `card` turn and answers its client and its id."""
    client = StdioMcpClient(command)
    try:
        client.initialize()
        answer = client.call_tool(
            'block_panel',
            {'operation': 'create', 'title': CARD_HEADING, 'document': card_document()},
            CARD_TIMEOUT_SECONDS)
    except Fatal:
        client.close()
        raise
    panel_id = answer.get('panel_id')
    if not isinstance(panel_id, str) or not panel_id:
        client.close()
        raise Fatal(f'the host answered the card without a panel id: {one_line(json.dumps(answer))}')
    return client, panel_id


def watch_card(session, number, client, panel_id):
    """Keeps one `next_click` of the card open in a thread of its own.

    A press of the person answers the wait, and the thread then prints the text
    of the agent and the result of that press. Nothing waits on this thread: the
    turn of the person goes on, and the program ends when its input closes.
    """
    def wait():
        try:
            answer = client.call_tool(
                'block_panel',
                {'operation': 'next_click', 'panel_id': panel_id,
                 'timeout_millis': CLICK_TIMEOUT_MILLIS},
                CARD_WAIT_TIMEOUT_SECONDS)
        except Fatal as refusal:
            client.close()
            text(session, f'msg-{number}-press', f'The wait of the card ended: {one_line(str(refusal))}')
            return
        client.close()
        click = answer.get('click') if isinstance(answer.get('click'), dict) else {}
        component = click.get('component_id')
        if not isinstance(component, str) or not component:
            return
        text(session, f'msg-{number}-press', f'You pressed {component}')
        result(session, f'You pressed {component}', DURATION_MS)

    thread = threading.Thread(target=wait, name='stand-in-card', daemon=True)
    thread.start()


def card_turn(session, number):
    """Draws one card in the workspace of this conversation and listens for it."""
    command = mcp_command()
    if not command:
        text(session, f'msg-{number}-card',
             f'The stand-in draws no card: {EXECUTABLE_ENV} names no program of Horizon.')
        return
    try:
        client, panel_id = create_card(command)
    except Fatal as refusal:
        text(session, f'msg-{number}-card', f'The stand-in drew no card: {one_line(str(refusal))}')
        return
    text(session, f'msg-{number}-card',
         f'I drew the card {panel_id} in this workspace. Press Go or Cancel and I read it.')
    watch_card(session, number, client, panel_id)


def answer(session, number, words):
    """One turn: the init line, the answer, what the words ask for, and the result."""
    init(session)
    started = time.monotonic()
    if TURN_WORDS['slow'] in words:
        time.sleep(SLOW_SECONDS)
    message_id = f'msg-{number}'
    text(session, message_id, f'You said: {words}')
    if TURN_WORDS['card'] in words:
        card_turn(session, number)
    if TURN_WORDS['tool'] in words:
        tool(session, message_id)
    if TURN_WORDS['long'] in words:
        long_answer(session, message_id)
    limit(session)
    result(session, f'You said: {words}', int((time.monotonic() - started) * 1000) + DURATION_MS)


def main():
    session = str(uuid.uuid4())
    number = 0
    for request in sys.stdin:
        if not request.strip():
            continue
        number += 1
        answer(session, number, words_of(request))
    return 0


if __name__ == '__main__':
    sys.exit(main())
