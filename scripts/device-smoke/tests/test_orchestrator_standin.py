"""The stand-in program of a dock conversation answers in the Claude Code format.

The dock of the fixture starts `orchestrator_standin.py` as the program of one
conversation. These tests run the file itself, with the arguments that Horizon
gives Claude Code, and read the lines that it answers.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import unittest.mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import orchestrator_standin  # noqa: E402
import serve  # noqa: E402

SCRIPT = Path(__file__).resolve().parents[1] / serve.ORCHESTRATOR_STANDIN_PROGRAM
REPO = Path(__file__).resolve().parents[3]
HORIZON_ARGUMENTS = ['-p', '--input-format', 'stream-json', '--output-format', 'stream-json',
                     '--verbose', '--session-id', '00000000-0000-4000-8000-000000000001']


def request(text):
    """One line of the person, in the shape of the standard input of the program."""
    return json.dumps({'type': 'user', 'message': {'role': 'user', 'content': [
        {'type': 'text', 'text': text}]}}) + '\n'


def run(turns, horizon_program=None):
    """Every line of the program after it read one line for each text of `turns`.

    The environment names no Horizon program, so a turn that holds `card` draws
    no card and starts no MCP server: these tests start no Horizon host. A test
    that needs the card path names a program that does not exist and reads the
    refusal, so no MCP server starts either.
    """
    environment = {name: value for name, value in os.environ.items()
                   if name != orchestrator_standin.EXECUTABLE_ENV}
    if horizon_program:
        environment[orchestrator_standin.EXECUTABLE_ENV] = horizon_program
    environment['PYTHONDONTWRITEBYTECODE'] = '1'
    answer = subprocess.run([str(SCRIPT), *HORIZON_ARGUMENTS], input=''.join(request(text) for text in turns),
                            text=True, capture_output=True, timeout=120, env=environment)
    if answer.returncode != 0:
        raise AssertionError(f'the stand-in ended with {answer.returncode}: {answer.stderr}')
    return [json.loads(line) for line in answer.stdout.splitlines()]


def texts_of(records):
    """The texts of the agent of one run, in order."""
    found = []
    for record in records:
        if record.get('type') != 'assistant':
            continue
        for block in record['message']['content']:
            if block.get('type') == 'text':
                found.append(block['text'])
    return found


class OrchestratorStandinTests(unittest.TestCase):
    def test_the_program_is_executable_and_ignores_the_arguments_of_claude_code(self):
        self.assertTrue(SCRIPT.stat().st_mode & 0o100, 'the program is executable')
        self.assertEqual(SCRIPT.stat().st_mode & 0o444, 0o444, 'every reader runs the program')

    def test_one_turn_names_the_session_and_answers_with_the_text_of_the_person(self):
        records = run(['hello there'])
        kinds = [(record.get('type'), record.get('subtype')) for record in records]
        self.assertEqual(kinds[0], ('system', 'init'))
        self.assertEqual(kinds[-1], ('result', 'success'))
        self.assertEqual(texts_of(records), ['You said: hello there'])
        self.assertIn('rate_limit_event', [kind for kind, _ in kinds])
        result = records[-1]
        self.assertFalse(result['is_error'])
        self.assertGreater(result['duration_ms'], 0)
        self.assertAlmostEqual(result['total_cost_usd'], 0.01)
        self.assertAlmostEqual(sum(model['costUSD'] for model in result['model_usage_since_last_result'].values()), 0.01)

    def test_the_word_tool_adds_a_call_of_bash_and_its_result(self):
        records = run(['use a tool please'])
        calls = [block for record in records if record.get('type') == 'assistant'
                 for block in record['message']['content'] if block.get('type') == 'tool_use']
        self.assertEqual([call['name'] for call in calls], ['Bash'])
        self.assertEqual(calls[0]['input']['command'], 'echo stand-in')
        results = [block for record in records if record.get('type') == 'user'
                   for block in record['message']['content'] if block.get('type') == 'tool_result']
        self.assertEqual([result['tool_use_id'] for result in results], [calls[0]['id']])
        self.assertFalse(results[0]['is_error'])

    def test_the_word_long_adds_forty_numbered_lines(self):
        records = run(['long answer please'])
        texts = texts_of(records)
        self.assertEqual(len(texts), 2, 'the answer and the long text')
        lines = texts[1].splitlines()
        self.assertEqual(len(lines), 40)
        self.assertTrue(lines[0].startswith('1. '))
        self.assertTrue(lines[-1].startswith('40. '))

    def test_the_card_names_the_plan_the_subtitle_the_text_and_two_buttons(self):
        document = orchestrator_standin.card_document()
        components = document['components']
        self.assertEqual([component['type'] for component in components],
                         ['heading', 'text', 'buttons'])
        self.assertEqual(components[0]['text'], orchestrator_standin.CARD_HEADING)
        self.assertEqual(components[0]['subtitle'], orchestrator_standin.CARD_SUBTITLE)
        self.assertEqual(components[1]['text'], orchestrator_standin.CARD_TEXT)
        buttons = components[2]['items']
        self.assertEqual([button['id'] for button in buttons], ['go', 'cancel'])
        self.assertEqual([button['label'] for button in buttons], ['Go', 'Cancel'])
        self.assertEqual(buttons[0]['tone'], 'primary')

    def test_a_turn_of_a_card_draws_nothing_without_a_program_of_horizon(self):
        records = run(['draw a card'])
        texts = texts_of(records)
        self.assertEqual(texts[0], 'You said: draw a card')
        self.assertIn(orchestrator_standin.EXECUTABLE_ENV, texts[1])
        self.assertEqual(len(texts), 2, 'the answer and the notice, and no card text')
        self.assertEqual(records[-1]['type'], 'result', 'the turn ends with its result')
        self.assertEqual(texts_of(run(['hello there'])), ['You said: hello there'])

    def test_the_word_sleepy_draws_a_card_without_a_wait(self):
        records = run(['sleepy card'])
        texts = texts_of(records)
        self.assertEqual(texts[0], 'You said: sleepy card')
        self.assertIn(
            orchestrator_standin.EXECUTABLE_ENV,
            texts[1],
            'the turn names that no program of Horizon draws its card and no wait opens',
        )
        self.assertEqual(len(texts), 2, 'the answer and the notice, and no card text')
        self.assertEqual(records[-1]['type'], 'result', 'the turn ends with its result')
        self.assertEqual(orchestrator_standin.QUIET_CARD, {}, 'a turn without a card holds none')

    def test_a_wake_turn_without_a_card_answers_that_it_holds_none(self):
        records = run([orchestrator_standin.WAKE_LINE_TEXT])
        texts = texts_of(records)
        self.assertEqual(texts[0], f'You said: {orchestrator_standin.WAKE_LINE_TEXT}')
        self.assertIn('The stand-in holds no card to read.', texts)
        self.assertEqual(records[-1]['type'], 'result', 'the turn ends with its result')

    def test_a_wake_turn_draws_no_card_and_calls_no_tool(self):
        # The wake line holds the words `card` and `tool`, and it asks for neither:
        # the turn reads the press of the card that its sleepy turn drew.
        records = run([orchestrator_standin.WAKE_LINE_TEXT], horizon_program='/nonexistent/horizon')
        texts = texts_of(records)
        self.assertIn('The stand-in holds no card to read.', texts)
        self.assertFalse([text for text in texts if text.startswith('The stand-in drew no card')],
                         'the wake turn draws no card')
        calls = [block for record in records if record.get('type') == 'assistant'
                 for block in record['message']['content'] if block.get('type') == 'tool_use']
        self.assertEqual(calls, [], 'the wake turn calls no tool')

    def test_the_question_names_the_card_the_question_and_two_answers(self):
        document = orchestrator_standin.question_document()
        components = document['components']
        self.assertEqual([component['type'] for component in components], ['heading', 'callout'])
        self.assertEqual(components[0]['text'], 'Cold start')
        self.assertEqual(orchestrator_standin.QUESTION_HEADING, 'Cold start')
        self.assertEqual(components[1]['text'], 'Two ways cut the start time. Pick one.')
        self.assertEqual(orchestrator_standin.QUESTION_TEXT, 'Two ways cut the start time. Pick one.')
        answers = components[1]['buttons']
        self.assertEqual([answer['id'] for answer in answers], ['lazy-fonts', 'new-format'])
        self.assertEqual([answer['label'] for answer in answers], ['Lazy fonts', 'New format'])
        self.assertEqual(answers[0]['tone'], 'primary')

    def test_the_answer_of_a_question_finishes_the_card_and_names_the_label(self):
        document = orchestrator_standin.answered_document()
        self.assertTrue(document['finished'], 'the answered card is finished')
        self.assertEqual([component['type'] for component in document['components']], ['heading'],
                         'the callout of the question is gone')
        self.assertEqual(document['components'][0]['text'], 'Cold start')
        self.assertEqual(orchestrator_standin.picked_label('lazy-fonts'), 'Lazy fonts')
        self.assertEqual(orchestrator_standin.picked_label('new-format'), 'New format')
        self.assertEqual(orchestrator_standin.picked_label('unknown'), 'unknown',
                         'a press that names no answer prints its id')

    def test_a_turn_of_a_question_draws_nothing_without_a_program_of_horizon(self):
        records = run(['ask a question'])
        texts = texts_of(records)
        self.assertEqual(texts[0], 'You said: ask a question')
        self.assertIn(orchestrator_standin.EXECUTABLE_ENV, texts[1])
        self.assertEqual(len(texts), 2, 'the answer and the notice, and no card text')
        self.assertEqual(records[-1]['type'], 'result', 'the turn ends with its result')

    def test_the_wake_text_of_the_stand_in_is_part_of_the_line_of_horizon(self):
        source = REPO / 'crates/horizon-core/src/block_panel/wake.rs'
        self.assertIn(
            orchestrator_standin.WAKE_LINE_TEXT,
            source.read_text(encoding='utf-8'),
            'the stand-in answers the line that Horizon sends to an orchestrator',
        )

    def test_the_command_of_the_mcp_server_is_the_program_of_horizon(self):
        with unittest.mock.patch.dict('os.environ', {orchestrator_standin.EXECUTABLE_ENV: ''}):
            self.assertEqual(orchestrator_standin.mcp_command(), '')
        with unittest.mock.patch.dict('os.environ',
                                      {orchestrator_standin.EXECUTABLE_ENV: '/opt/horizon'}):
            self.assertEqual(orchestrator_standin.mcp_command(), '/opt/horizon --browser-mcp')

    def test_two_turns_of_one_program_keep_their_order(self):
        records = run(['one', 'two'])
        self.assertEqual([record.get('type') for record in records].count('result'), 2)
        self.assertEqual(texts_of(records), ['You said: one', 'You said: two'])

    def test_the_fixture_names_the_copy_of_the_program_in_its_state_folder(self):
        with tempfile.TemporaryDirectory() as folder:
            state = Path(folder)
            (state / 'data').mkdir()
            environment = {}
            with unittest.mock.patch.dict('os.environ', {serve.ORCHESTRATOR_PROGRAM_VARIABLE: str(SCRIPT)}):
                copy = serve.orchestrator_program_in_state(state, (environment,))
            self.assertIsNotNone(copy, 'the fixture copies the stand-in of the dock')
            self.assertEqual(Path(copy).parent, state.resolve() / 'data' / serve.PANEL_PROGRAM_FOLDER)
            self.assertTrue(Path(copy).stat().st_mode & 0o100, 'the copy is executable')
            self.assertEqual(environment[serve.ORCHESTRATOR_PROGRAM_VARIABLE], str(copy))
            beside = Path(copy).parent / serve.WORKTREE_CARD_PROGRAM
            self.assertTrue(
                beside.is_file(),
                'the stand-in imports the MCP client of card_agent.py, which lies beside it')


if __name__ == '__main__':
    unittest.main()
