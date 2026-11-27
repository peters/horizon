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

The words `tool`, `long` and `slow` choose what a turn answers. The program ends
when the standard input closes, as an agent program of a conversation does.

Usage, inside the isolated fixture:

    HORIZON_ORCHESTRATOR_DOCK=1 \
    HORIZON_ORCHESTRATOR_PROGRAM=<repo>/scripts/device-smoke/orchestrator_standin.py \
        python3 scripts/device-smoke/serve.py --native-view --state <evidence>/dock
"""
import json
import os
import sys
import time
import uuid

MODEL = 'claude-stand-in'
COST_USD = 0.01
DURATION_MS = 12
SLOW_SECONDS = 2
LONG_LINES = 40
TURN_WORDS = {'tool': 'tool', 'long': 'long', 'slow': 'slow'}
TOOL_NAME = 'Bash'
TOOL_COMMAND = 'echo stand-in'
TOOL_RESULT = 'stand-in'


def line(record):
    """Writes one JSON line of the agent, flushed for the reader of Horizon."""
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


def answer(session, number, words):
    """One turn: the init line, the answer, what the words ask for, and the result."""
    init(session)
    started = time.monotonic()
    if TURN_WORDS['slow'] in words:
        time.sleep(SLOW_SECONDS)
    message_id = f'msg-{number}'
    text(session, message_id, f'You said: {words}')
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
