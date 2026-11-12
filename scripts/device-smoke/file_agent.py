#!/usr/bin/env python3
"""Run the tools of a Horizon candidate from the lines of a command file.

A computer can refuse the input that an automated test needs: a Mac whose owner
did not give the Accessibility permission to the program that runs the test
sends no keys and no clicks. This program keeps that computer testable: the
person writes one MCP tool call per line of a file, and the program runs each new
line and shows the answer in its panel and in a second file.

One line of the command file is one JSON object:

    {"tool": "machine_set", "arguments": {"name": "build-1", "ssh": {"host": "127.0.0.1"}}, "timeout": 30}
    {"quit": true}

The program starts `<candidate> --browser-mcp` as its child and speaks the MCP
protocol of that child, through the client of `card_agent.py`, which lies beside
this file. It reads the command file again every half second and runs each line
that it has not run yet, in the order of the file. It prints one line for each
answer and appends `{"request": ..., "answer": ...}` to the answer file.

A refusal of a tool is an answer, not a crash: the program records the refusal
and reads the next line. A line that is not a JSON object is reported and
skipped, and a line counts only when its newline is in the file, so a program
that writes one line in two writes does not lose it.

Standard library only. This program writes the answer file and no other file.
"""
from __future__ import annotations

import sys

# The MCP client of this program is the one of card_agent.py, so the two files lie
# together in a panel of the fixture, and this import leaves no __pycache__.
sys.dont_write_bytecode = True

from pathlib import Path
import argparse
import json
import os
import time

import card_agent
from card_agent import Fatal, McpFailure, StdioMcpClient, one_line

PROGRAM = 'file-agent'
MCP_COMMAND_ENV = 'FILE_AGENT_MCP_COMMAND'
DEFAULT_TIMEOUT_SECONDS = 30.0
POLL_SECONDS = 0.5
# The longest answer that one line of the panel holds; the answer file holds all.
PANEL_ANSWER_CHARS = 300


def log(message):
    """Print one line of this program, with its own name and not the name of card_agent."""
    print(f'{PROGRAM}: {message}', flush=True)


def parse_request(text):
    """One line of the command file as a request, or `(None, problem)`.

    A request names a tool of the server, its arguments and the seconds that one
    answer may take. `{"quit": true}` ends the run.
    """
    try:
        message = json.loads(text)
    except ValueError as problem:
        return None, f'not JSON: {problem}'
    if not isinstance(message, dict):
        return None, f'{type(message).__name__} is not a JSON object'
    if message.get('quit') is True:
        return {'quit': True}, None
    tool = message.get('tool')
    if not isinstance(tool, str) or not tool.strip():
        return None, 'the object needs a "tool" name'
    arguments = message.get('arguments', {})
    if not isinstance(arguments, dict):
        return None, f'"arguments" is {type(arguments).__name__}, not an object'
    timeout = message.get('timeout', DEFAULT_TIMEOUT_SECONDS)
    if isinstance(timeout, bool) or not isinstance(timeout, (int, float)) or timeout <= 0:
        return None, f'"timeout" must be a positive number of seconds, not {timeout!r}'
    return {'tool': tool, 'arguments': arguments, 'timeout': float(timeout)}, None


def complete_lines(text):
    """The lines of `text` that a writer has finished, without their newlines.

    A line counts when its newline is in the file. A person's editor, or a command
    that appends, can leave the last line half written, and a request that this
    program reads too early would be reported and lost.
    """
    if not text:
        return []
    lines = text.split('\n')
    return lines[:-1]


def unread_lines(path, handled):
    """The lines that this run has not read yet, and the text that it has read.

    A person appends to the file, so the text already read is the cursor. A file
    that does not start with that text was written again, and this run reads it
    again from its first line.
    """
    try:
        text = path.read_text(encoding='utf-8', errors='replace')
    except FileNotFoundError:
        return [], handled
    if not text.startswith(handled):
        if handled:
            log(f'{PROGRAM}: the command file was written again; read it from its first line')
        handled = ''
    lines = complete_lines(text[len(handled):])
    if lines:
        end = text.rindex('\n', len(handled)) + 1
        return lines, text[:end]
    return [], handled


def answer_of(client, request):
    """The answer of one tool call, or the refusal of that tool as an answer."""
    try:
        answer = client.call_tool(request['tool'], request['arguments'], request['timeout'])
    except McpFailure as refusal:
        return {'status': 'refused', 'error': str(refusal)}
    return answer if isinstance(answer, dict) else {'status': 'answered', 'answer': answer}


def record(answers, request, answer):
    """Append one request and its answer to the answer file, as one JSON line."""
    answers.parent.mkdir(parents=True, exist_ok=True)
    with answers.open('a', encoding='utf-8') as handle:
        handle.write(json.dumps({'request': request, 'answer': answer}, sort_keys=True) + '\n')
        handle.flush()
        os.fsync(handle.fileno())


def show_answer(tool, answer):
    """One line of the panel for one answer, short enough to read there.

    An answer holds the text of a page or of a card, and Horizon reads the last
    lines of an agent panel to report its state: an answer whose text would read
    as a working line is shown as its status alone, and the answer file keeps the
    whole text.
    """
    detail = one_line(json.dumps(answer, sort_keys=True))
    if len(detail) > PANEL_ANSWER_CHARS:
        detail = detail[:PANEL_ANSWER_CHARS] + '...'
    line = f'{tool} -> {detail}'
    if card_agent.looks_like_working_line(f'{PROGRAM}: {line}'):
        status = answer.get('status') if isinstance(answer.get('status'), str) else 'answered'
        line = f'{tool} -> {status}; the answer file holds the text of this answer'
    log(line)


def run(client, commands, answers, poll_seconds=POLL_SECONDS):
    """Read the command file until it says quit, a signal arrives or the panel goes."""
    log(f'{PROGRAM}: commands {commands}')
    log(f'{PROGRAM}: answers {answers}')
    log(f'{PROGRAM}: a new line runs within {poll_seconds:.1f} s; write {{"quit": true}} to end this program')
    handled = ''
    while True:
        lines, handled = unread_lines(commands, handled)
        for text in lines:
            if not text.strip():
                continue
            request, problem = parse_request(text)
            if request is None:
                # A skipped line is evidence too: keep it in the answer file.
                skipped = {'status': 'skipped', 'error': problem}
                record(answers, {'line': text}, skipped)
                log(f'{PROGRAM}: skipped one line: {problem}')
                continue
            if request.get('quit'):
                log(f'{PROGRAM}: quit')
                return 0
            answer = answer_of(client, request)
            record(answers, request, answer)
            show_answer(request['tool'], answer)
        time.sleep(poll_seconds)


def parse_args(argv):
    parser = argparse.ArgumentParser(
        description='Run the tools of a Horizon candidate from the lines of a command file.')
    parser.add_argument(
        '--mcp-command', default=os.environ.get(MCP_COMMAND_ENV, ''),
        help='Command line of the Horizon MCP server, such as "<candidate> --browser-mcp". '
             f'Defaults to the {MCP_COMMAND_ENV} environment variable.')
    parser.add_argument(
        '--commands', type=Path, required=True,
        help='Command file that this program reads again every half second, one JSON object '
             'per line: {"tool": NAME, "arguments": {...}, "timeout": SECONDS} or {"quit": true}')
    parser.add_argument(
        '--answers', type=Path, required=True,
        help='Answer file that this program appends to, one JSON object per request: '
             '{"request": ..., "answer": ...}')
    args, extra = parser.parse_known_args(argv)
    if not args.mcp_command.strip():
        parser.error(f'--mcp-command is required, or set {MCP_COMMAND_ENV}')
    return args, extra


def main(argv=None):
    args, extra = parse_args(sys.argv[1:] if argv is None else argv)
    card_agent.install_signal_handlers()
    client = None
    try:
        log(f'{PROGRAM}: MCP command: {args.mcp_command}')
        if extra:
            log(f'{PROGRAM}: ignored launch arguments: {" ".join(extra)}')
        client = StdioMcpClient(args.mcp_command)
        handshake = client.initialize()
        server = (handshake.get('serverInfo') or {}).get('name') or 'unknown'
        log(f'{PROGRAM}: initialized with {server}, {len(client.tool_names())} tools')
        return run(client, args.commands, args.answers)
    except card_agent.Stop:
        log(f'{PROGRAM}: signal received; exit')
        return 0
    except Fatal as error:
        print(f'{PROGRAM}: {error}', file=sys.stderr, flush=True)
        return 1
    finally:
        if client is not None:
            client.close()


if __name__ == '__main__':
    sys.exit(main())
