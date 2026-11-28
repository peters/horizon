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

A program whose actor is the front desk (`HORIZON_BROWSER_ACTOR` is
`horizon:front-desk`) answers as the front desk instead: `two tasks` proposes two
work items through the three operations of the front desk, a turn that holds
`started the work items` answers the line that Horizon sends after a press, and
`what waits` reads the work items and counts the ones that need the person.

The words `tool`, `fail`, `long`, `slow`, `card`, `sleepy`, `question` and
`terminal` choose what a turn answers: a tool call that succeeds, a tool call that
fails with a result of two lines, a long answer, a wait before the answer, a card,
a card that asks the person a question, or the terminal of a machine of the list,
which an orchestrator opens with `machine`. A turn whose text
holds `card` draws one card in the workspace of the conversation through a real
`horizon --browser-mcp` child, with the identity that Horizon gave this program,
and then keeps one `next_click` open for that card in a thread of its own. A
press of the person answers that wait, and the thread prints the text of the
agent and the result of that turn.

A turn whose text holds `sleepy` draws the same card and opens no `next_click`, so
the press of the person waits in the queue of the card until a later turn holds
the wake line that Horizon sends to the orchestrator of the workspace. That turn
reads the one press with a short wait, and prints `Woken, you pressed <id>`.

A turn whose text holds a worker asks for one: `start a worker` starts the first
agent preset that `presets` answers with the worker operation of `block_panel`,
`started the worker` answers the line that Horizon sends after a press, and a
line that holds `Report of your worker` answers with the text after its first
line. The same file has a second mode for a WORKER panel: `--worker` prints that
it reports and calls `worker_report` with one text, so a procedure reads the
report of a worker without a real agent program. The fixture opens that panel
with `--panels`, whose entry names this file and `--worker`.

A turn whose text holds `question` draws a card titled `Cold start` that holds one
callout with two answers and keeps one `next_click` open for it. A press of the
person updates that card to a finished document without a callout, and the turn
prints `You picked <label>`, so the group `Needs you` of the sidebar shows the
question and ends with the answer. The program ends when the standard input
closes, as an agent program of a conversation does.

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
TURN_WORDS = {'tool': 'tool', 'fail': 'fail', 'long': 'long', 'slow': 'slow', 'card': 'card',
              'sleepy': 'sleepy', 'question': 'question', 'terminal': 'terminal'}
# The word of a turn that opens the terminal of a machine of the list, and the text
# of a turn that found no machine to open.
TERMINAL_WORD = 'terminal'
TERMINAL_NO_MACHINE_TEXT = ('I found no machine in the list of this Horizon, so I opened no '
                            'terminal. Register one with machine_set first.')
# The word and the address of the route of a turn that asks for a route of its own.
ROUTE_WORD = 'ssh'
ROUTE_HOST = '203.0.113.10'
TOOL_NAME = 'Bash'
TOOL_COMMAND = 'echo stand-in'
TOOL_RESULT = 'stand-in'
TOOL_FAIL_COMMAND = 'exit 3'
TOOL_FAIL_RESULT = 'the command failed\nsecond line'
# The card of a turn that holds `card`: one heading with a subtitle, one text and
# two buttons, so the person has a primary action and a way out.
CARD_HEADING = 'Plan'
CARD_SUBTITLE = 'drawn by the stand-in orchestrator'
CARD_TEXT = 'This card comes from the orchestrator of this workspace, not from an agent panel.'
CARD_BUTTONS = ({'id': 'go', 'label': 'Go', 'tone': 'primary'},
                {'id': 'cancel', 'label': 'Cancel', 'tone': 'danger'})
# The card of a turn that holds `question`: one heading and one callout with the
# two ways that it offers, so the group `Needs you` of the sidebar has a question
# to show. The ids stay short and stable, and the labels are the words that the
# answer of the stand-in prints back.
QUESTION_HEADING = 'Cold start'
QUESTION_TEXT = 'Two ways cut the start time. Pick one.'
QUESTION_BUTTONS = ({'id': 'lazy-fonts', 'label': 'Lazy fonts', 'tone': 'primary'},
                    {'id': 'new-format', 'label': 'New format', 'tone': 'neutral'})
# The text of the turn that answers a question, in front of the label of the answer.
PICKED_PREFIX = 'You picked '
# How long the card waits for one press: five minutes, the default of the tool.
CLICK_TIMEOUT_MILLIS = 300_000
CARD_TIMEOUT_SECONDS = 30.0
CARD_WAIT_TIMEOUT_SECONDS = CLICK_TIMEOUT_MILLIS / 1000.0 + 60.0
# The actor of the front desk of the dock: a program of that actor answers as the
# front desk and not as the orchestrator of a workspace. It is the value that
# `HORIZON_BROWSER_ACTOR` holds in a program of the front desk.
FRONT_DESK_ACTOR = 'horizon:front-desk'
ACTOR_ENV = 'HORIZON_BROWSER_ACTOR'
# The two work items that a `two tasks` turn of the front desk proposes: the title
# becomes the name of a workspace, and the task becomes the first message of its
# orchestrator. The second task holds the word `question`, so the orchestrator that
# reads it draws a card that asks the person.
PROPOSAL_ITEMS = (
    {'title': 'Fix login redirect', 'task': 'Fix issue 1987. Done means the test passes.'},
    {'title': 'Faster cold start', 'task': 'question: make the cold start faster.'},
)
# The answer of the turn that proposes those work items.
PROPOSED_TEXT = 'Two work items are ready to start.'
# The answer of the turn that Horizon sends after the person starts them.
STARTED_TEXT = 'Both are started. I tell you when one needs you.'
# The first words of the turn that the orchestrator sends to a worker, and of the
# turn that Horizon sends back to the orchestrator when that worker reports. The
# second one is the line of `block_requests/workers.rs` of the window.
REPORT_LINE_PREFIX = 'Report of your worker '
# The task of the worker that a `start a worker` turn asks for, and the words that
# the answer of the request and the words of the press hold.
WORKER_TASK = 'Say hello and report.'
WORKER_ASKED_TEXT = 'I asked for a worker.'
WORKER_STARTED_TEXT = 'The worker runs.'
WORKER_REPORT_PREFIX = 'I read the report: '
# The text that the worker mode of this file reports, and the line that it prints
# before it.
WORKER_MODE_ARGUMENT = '--worker'
WORKER_REPORT_TEXT = 'hello from the worker'
WORKER_MODE_LINE = 'The worker reports.'
# The kinds of preset that start an agent program, as the settings store them. A
# preset of one of these kinds is a worker.
AGENT_KINDS = ('claude', 'codex', 'pi', 'gemini', 'open_code', 'kilo_code', 'grok')
# How long the worker mode of this file waits for the report to be answered.
WORKER_REPORT_TIMEOUT_SECONDS = 60.0
# The first words of the one line that Horizon sends to an orchestrator when the
# person presses a button of its card and no `next_click` waits. It is a part of
# `WAKE_LINE` of `crates/horizon-core/src/block_panel/wake.rs`, and a test of this
# file holds the two together.
WAKE_LINE_TEXT = 'A button was pressed on your Horizon card'
# How long the wake turn of a `sleepy` card waits for the press that woke it.
WAKE_TIMEOUT_MILLIS = 5_000
WAKE_WAIT_TIMEOUT_SECONDS = WAKE_TIMEOUT_MILLIS / 1000.0 + 30.0
# The card of a `sleepy` turn whose `next_click` a later wake line opens: the
# client of its MCP server, its panel id, and the number of the turn that drew it.
QUIET_CARD = {}
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


def tool(session, message_id, failed=False):
    """One call of `Bash` and its result, as two lines of the line format. A call
    that failed names its command and its error, and the error has two lines."""
    tool_id = f'toolu-{message_id}'
    command = TOOL_FAIL_COMMAND if failed else TOOL_COMMAND
    result_text = TOOL_FAIL_RESULT if failed else TOOL_RESULT
    line({'type': 'assistant', 'session_id': session, 'message': {
        'id': f'tool-{message_id}', 'type': 'message', 'role': 'assistant', 'model': MODEL,
        'content': [{'type': 'tool_use', 'id': tool_id, 'name': TOOL_NAME,
                     'input': {'command': command, 'description': 'Print the stand-in'}}],
        'stop_reason': None, 'usage': {}}})
    line({'type': 'user', 'session_id': session, 'message': {'role': 'user', 'content': [
        {'type': 'tool_result', 'tool_use_id': tool_id, 'content': result_text,
         'is_error': failed}]}})


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


def question_document():
    """The document of the one card of a `question` turn: the question that waits
    for the person, with the two answers of the stand-in."""
    return {'components': [
        {'type': 'heading', 'text': QUESTION_HEADING},
        {'type': 'callout', 'text': QUESTION_TEXT,
         'buttons': [dict(button) for button in QUESTION_BUTTONS]},
    ]}


def answered_document():
    """The document of the card of a `question` turn after the press: the answer is
    in, the callout is gone and the card is finished."""
    return {'finished': True, 'components': [
        {'type': 'heading', 'text': QUESTION_HEADING},
    ]}


def picked_label(component):
    """The label of the answer whose id is `component`, or the id itself for a press
    that names no answer of the card."""
    for button in QUESTION_BUTTONS:
        if button['id'] == component:
            return button['label']
    return component


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


def card_turn(session, number, listen=True):
    """Draws one card in the workspace of this conversation, and listens for it
    unless `listen` is false: then only the wake line of a later turn reads the
    press, and the client of that card stays in `QUIET_CARD`."""
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
    if not listen:
        QUIET_CARD.update(client=client, panel_id=panel_id, number=number)
        text(session, f'msg-{number}-card',
             f'I drew the card {panel_id} and I hold no wait for it. Press Go or Cancel and one notice '
             'of the dock wakes me.')
        return
    text(session, f'msg-{number}-card',
         f'I drew the card {panel_id} in this workspace. Press Go or Cancel and I read it.')
    watch_card(session, number, client, panel_id)


def wake_turn(session, number):
    """Reads the one press that woke this program, with a short wait.

    The card of a `sleepy` turn opened no `next_click`, so the press waited in
    the queue of the card until Horizon sent the wake line. This turn reads that
    press once: the card is taken out of `QUIET_CARD` first, so a second wake line
    opens no second wait.
    """
    card = dict(QUIET_CARD)
    QUIET_CARD.clear()
    client, panel_id = card.get('client'), card.get('panel_id')
    if client is None or not panel_id:
        text(session, f'msg-{number}-wake', 'The stand-in holds no card to read.')
        return
    try:
        answer = client.call_tool(
            'block_panel',
            {'operation': 'next_click', 'panel_id': panel_id,
             'timeout_millis': WAKE_TIMEOUT_MILLIS},
            WAKE_WAIT_TIMEOUT_SECONDS)
    except Fatal as refusal:
        client.close()
        text(session, f'msg-{number}-wake', f'The wait of the card ended: {one_line(str(refusal))}')
        return
    client.close()
    click = answer.get('click') if isinstance(answer.get('click'), dict) else {}
    component = click.get('component_id')
    if isinstance(component, str) and component:
        text(session, f'msg-{number}-wake', f'Woken, you pressed {component}')


def question_turn(session, number):
    """Draws the card of a `question` turn, waits for the press of the person in a
    thread of its own, and answers with the label that the person chose.

    A press updates the card to the finished document without a callout, so the
    question leaves the group `Needs you` of the sidebar and the card folds.
    """
    command = mcp_command()
    if not command:
        text(session, f'msg-{number}-question',
             f'The stand-in draws no card: {EXECUTABLE_ENV} names no program of Horizon.')
        return
    client = StdioMcpClient(command)
    try:
        client.initialize()
        answer_of_create = client.call_tool(
            'block_panel',
            {'operation': 'create', 'title': QUESTION_HEADING, 'document': question_document()},
            CARD_TIMEOUT_SECONDS)
    except Fatal as refusal:
        client.close()
        text(session, f'msg-{number}-question', f'The stand-in drew no card: {one_line(str(refusal))}')
        return
    panel_id = answer_of_create.get('panel_id')
    if not isinstance(panel_id, str) or not panel_id:
        client.close()
        text(session, f'msg-{number}-question',
             f'The host answered the card without a panel id: {one_line(json.dumps(answer_of_create))}')
        return
    text(session, f'msg-{number}-question',
         f'I drew the card {panel_id} with one question. Pick Lazy fonts or New format in the sidebar.')
    watch_question(session, number, client, panel_id)


def watch_question(session, number, client, panel_id):
    """Reads the one press on the card of a `question` turn in a thread of its own.

    The press arrives on `next_click`, and the card then takes the finished
    document without a callout: the question leaves the sidebar and the card folds.
    """
    def wait():
        try:
            answer_of_wait = client.call_tool(
                'block_panel',
                {'operation': 'next_click', 'panel_id': panel_id,
                 'timeout_millis': CLICK_TIMEOUT_MILLIS},
                CARD_WAIT_TIMEOUT_SECONDS)
        except Fatal as refusal:
            client.close()
            text(session, f'msg-{number}-picked', f'The wait of the card ended: {one_line(str(refusal))}')
            return
        click = answer_of_wait.get('click') if isinstance(answer_of_wait.get('click'), dict) else {}
        component = click.get('component_id')
        if not isinstance(component, str) or not component:
            client.close()
            return
        try:
            client.call_tool(
                'block_panel',
                {'operation': 'update', 'panel_id': panel_id, 'document': answered_document()},
                CARD_TIMEOUT_SECONDS)
        except Fatal as refusal:
            text(session, f'msg-{number}-picked', f'The card kept its question: {one_line(str(refusal))}')
        client.close()
        picked = PICKED_PREFIX + picked_label(component)
        text(session, f'msg-{number}-picked', picked)
        result(session, picked, DURATION_MS)

    thread = threading.Thread(target=wait, name='stand-in-question', daemon=True)
    thread.start()


def is_front_desk():
    """Whether this program is the front desk of the dock and not the orchestrator
    of a workspace."""
    return os.environ.get(ACTOR_ENV, '').strip() == FRONT_DESK_ACTOR


def work_item_tool(command, operation, timeout):
    """One call of the block_panel tool of this Horizon, and its answer."""
    client = StdioMcpClient(command)
    try:
        client.initialize()
        return client.call_tool('block_panel', operation, timeout)
    finally:
        client.close()


def worker_preset(command):
    """The first agent preset that `presets` answers, with the kind of a program.

    A preset of an agent kind starts an agent program, which is a worker. The
    stand-in reads the list, so the name is the name of the person's settings and
    not a name that this file keeps.
    """
    answer = work_item_tool(command, {'operation': 'presets'}, CARD_TIMEOUT_SECONDS)
    for preset in answer.get('presets') or []:
        if isinstance(preset, dict) and isinstance(preset.get('name'), str) and preset.get('kind') in AGENT_KINDS:
            return preset['name']
    return ''


def machine_of_list(command):
    """The name of the first machine of the list of this Horizon, or an empty text."""
    answer = work_item_tool(command, {'operation': 'machines'}, CARD_TIMEOUT_SECONDS)
    for machine in answer.get('machines') or []:
        if isinstance(machine, dict) and isinstance(machine.get('name'), str):
            return machine['name']
    return ''


def terminal_turn(session, number, words):
    """One turn that opens the terminal of a machine of the list, as the
    orchestrator of this workspace opens it: with `machine` and no route of its own.

    The last word of the turn names the machine, and a turn that names none takes
    the first machine of the list, which is the record that the fixture seeded. A
    turn that also holds `ssh` asks for a route of its own, which Horizon refuses
    to an orchestrator, so the run reads that refusal.
    """
    command = mcp_command()
    asked = words.partition(TERMINAL_WORD)[2].split()
    name = asked[-1] if asked else ''
    if ROUTE_WORD in words:
        operation = {'operation': 'open_terminal', 'ssh': {'host': ROUTE_HOST}}
    else:
        name = name or machine_of_list(command)
        if not name:
            text(session, f'msg-{number}-terminal', TERMINAL_NO_MACHINE_TEXT)
            return
        operation = {'operation': 'open_terminal', 'machine': name}
    try:
        answer = work_item_tool(command, operation, CARD_TIMEOUT_SECONDS)
    except Fatal as failure:
        text(session, f'msg-{number}-terminal', f'The host answered: {one_line(str(failure))}')
        return
    panel_id = answer.get('panel_id')
    text(session, f'msg-{number}-terminal', f'I opened the terminal of {name} ({panel_id}).')


def worker_turn(session, number, words):
    """One turn of an orchestrator about a worker: the start, the line of a press,
    or the report of a worker that came back.

    The start asks for the first agent preset of the settings with one task. A
    program that the chip `Workers` does not hold starts nothing, and Horizon
    answers that the person decides: the stand-in says which of the two answers
    it read.
    """
    if words.startswith(REPORT_LINE_PREFIX):
        # The first line names the program and the panel of the worker, and the
        # report is under it.
        _, _, report = words.partition('\n')
        text(session, f'msg-{number}-report', WORKER_REPORT_PREFIX + report.strip())
        return True
    if 'started the worker' in words:
        text(session, f'msg-{number}-started-worker', WORKER_STARTED_TEXT)
        return True
    if 'start a worker' not in words:
        return False
    command = mcp_command()
    if not command:
        text(session, f'msg-{number}-worker',
             f'The stand-in starts no worker: {EXECUTABLE_ENV} names no program of Horizon.')
        return True
    try:
        program = worker_preset(command)
        if not program:
            text(session, f'msg-{number}-worker', 'The stand-in starts no worker: the settings hold no agent preset.')
            return True
        answer = work_item_tool(command, {'operation': 'worker_start', 'program': program,
                                          'task': WORKER_TASK}, CARD_TIMEOUT_SECONDS)
    except Fatal as refusal:
        text(session, f'msg-{number}-worker', f'The stand-in started no worker: {one_line(str(refusal))}')
        return True
    worker = answer.get('worker')
    if isinstance(worker, str) and worker:
        text(session, f'msg-{number}-worker', f'{WORKER_ASKED_TEXT} It runs as {program} in the panel {worker}.')
    else:
        text(session, f'msg-{number}-worker', WORKER_ASKED_TEXT)
    return True


def front_desk_turn(session, number, words):
    """One turn of the front desk: the work items that the person may start, the
    line that names their press, or the state of the work items.

    The tool of the front desk is `block_panel`: `work_item_propose` offers work
    items that start only at a press of the person, `work_items` reads their state,
    and the line of a press answers without a tool.
    """
    if 'started the work items' in words:
        text(session, f'msg-{number}-started', STARTED_TEXT)
        return
    command = mcp_command()
    if 'two tasks' in words:
        if not command:
            text(session, f'msg-{number}-propose',
                 f'The stand-in proposes no work item: {EXECUTABLE_ENV} names no program of Horizon.')
            return
        try:
            answer_of_propose = work_item_tool(
                command,
                {'operation': 'work_item_propose', 'items': [dict(item) for item in PROPOSAL_ITEMS]},
                CARD_TIMEOUT_SECONDS)
        except Fatal as refusal:
            text(session, f'msg-{number}-propose', f'The stand-in proposed no work item: {one_line(str(refusal))}')
            return
        proposal = answer_of_propose.get('proposal')
        if not isinstance(proposal, str) or not proposal:
            text(session, f'msg-{number}-propose',
                 f'The host answered the proposal without an id: {one_line(json.dumps(answer_of_propose))}')
            return
        text(session, f'msg-{number}-propose', PROPOSED_TEXT)
        return
    if 'what waits' in words:
        if not command:
            text(session, f'msg-{number}-waits',
                 f'The stand-in reads no work item: {EXECUTABLE_ENV} names no program of Horizon.')
            return
        try:
            answer_of_read = work_item_tool(command, {'operation': 'work_items'}, CARD_TIMEOUT_SECONDS)
        except Fatal as refusal:
            text(session, f'msg-{number}-waits', f'The stand-in read no work item: {one_line(str(refusal))}')
            return
        items = answer_of_read.get('items') if isinstance(answer_of_read.get('items'), list) else []
        need_you = sum(1 for item in items if isinstance(item, dict) and item.get('state') == 'needs_you')
        text(session, f'msg-{number}-waits', f'{len(items)} work items, {need_you} need you.')
        return
    text(session, f'msg-{number}', f'You said: {words}')


def answer(session, number, words):
    """One turn: the init line, the answer, what the words ask for, and the result."""
    init(session)
    started = time.monotonic()
    if is_front_desk():
        front_desk_turn(session, number, words)
        limit(session)
        result(session, f'You said: {words}', int((time.monotonic() - started) * 1000) + DURATION_MS)
        return
    if TURN_WORDS['slow'] in words:
        time.sleep(SLOW_SECONDS)
    message_id = f'msg-{number}'
    text(session, message_id, f'You said: {words}')
    if WAKE_LINE_TEXT in words:
        # The wake line carries the words `card` and `tool`, and it is no request
        # of the person: this turn reads the one press that woke the program and
        # draws nothing and calls nothing else.
        wake_turn(session, number)
    elif worker_turn(session, number, words):
        pass
    else:
        if TURN_WORDS['terminal'] in words:
            terminal_turn(session, number, words)
        if TURN_WORDS['question'] in words:
            question_turn(session, number)
        elif TURN_WORDS['card'] in words:
            card_turn(session, number, listen=TURN_WORDS['sleepy'] not in words)
        if TURN_WORDS['fail'] in words:
            tool(session, message_id, failed=True)
        elif TURN_WORDS['tool'] in words:
            tool(session, message_id)
        if TURN_WORDS['long'] in words:
            long_answer(session, message_id)
    limit(session)
    result(session, f'You said: {words}', int((time.monotonic() - started) * 1000) + DURATION_MS)


def worker_main():
    """The mode of a WORKER panel: it reads its task, reports one text and ends.

    The fixture opens this file as a panel of an agent kind with `--worker`, and
    Horizon types the task of the worker into it as its first prompt line. The
    panel then reports that one text through the `worker_report` operation of
    this Horizon, so a procedure reads the report of a worker without a real agent
    program. The line before the report is text of the terminal, not a line of the
    agent format.
    """
    sys.stdout.write(WORKER_MODE_LINE + '\n')
    sys.stdout.flush()
    task = sys.stdin.readline().strip()
    sys.stdout.write(f'The worker read: {task}\n')
    sys.stdout.flush()
    command = mcp_command()
    if not command:
        sys.stdout.write(f'The worker reports nothing: {EXECUTABLE_ENV} names no program of Horizon.\n')
        sys.stdout.flush()
        return 0
    try:
        answer_of_report = work_item_tool(
            command, {'operation': 'worker_report', 'text': WORKER_REPORT_TEXT}, WORKER_REPORT_TIMEOUT_SECONDS)
    except Fatal as refusal:
        sys.stdout.write(f'The worker reported nothing: {one_line(str(refusal))}\n')
        sys.stdout.flush()
        return 0
    sys.stdout.write(f'The worker reported: {answer_of_report.get("status")}\n')
    sys.stdout.flush()
    return 0


def main():
    if WORKER_MODE_ARGUMENT in sys.argv[1:]:
        return worker_main()
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
