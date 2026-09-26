"""An agent whose task is done stops its worker through the idle watcher, never while
another agent is working, and the reason and requester survive the stop."""
import io
import json
import os
from pathlib import Path
import runpy
import subprocess
import tempfile
import threading
import time
import unittest


ROOT = Path(__file__).parent
IDLE = runpy.run_path(str(ROOT / 'horizon-worker-idle'))
STOP = runpy.run_path(str(ROOT / 'horizon-worker-stop'))
Refused = IDLE['Refused']
REQUESTER = ('@1', 'agent-a', 'claude')


class WatcherTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.log = self.root / 'workspace' / '.horizon' / 'self-stops.jsonl'
        self.requests = []
        self.steps = []

    def stop(self, reason='PR 12 merged', output=None, cores=None, accepted=True):
        def request(pod, key):
            self.requests.append((pod, key))
            return accepted

        def sample():
            self.steps.append('cores')
            return cores

        def newest(own):
            self.steps.append(('output', own))
            return output
        return IDLE['self_stop'](reason, REQUESTER, 'pod1', 'pod-key', log=self.log, now=lambda: 10_000.0,
                                 output=newest, cores=sample, request=request)

    def test_a_quiet_worker_records_the_reason_and_requester_then_stops(self):
        self.assertIn('Stop requested', self.stop(output=10_000 - 600, cores=0.1))
        self.assertEqual(self.requests, [('pod1', 'pod-key')])
        record = json.loads(self.log.read_text())
        self.assertEqual(record, {'at': 10_000_000, 'reason': 'PR 12 merged', 'session': 'agent-a',
                                  'agent': 'claude'})
        self.assertEqual(STOP['last_stop'](self.log), record)
        # Other sessions are checked after the CPU sample, right before the stop.
        self.assertEqual(self.steps, ['cores', ('output', '@1')])

    def test_another_working_agent_or_a_busy_worker_refuses_the_stop(self):
        with self.assertRaisesRegex(Refused, 'Another agent session'):
            self.stop(output=10_000 - 30)
        with self.assertRaisesRegex(Refused, 'busy'):
            self.stop(cores=IDLE['BUSY_CORES'])
        with self.assertRaisesRegex(Refused, 'short reason'):
            self.stop(reason='  ')
        self.assertEqual(self.requests, [])
        self.assertFalse(self.log.exists())

    def test_a_refused_provider_request_leaves_no_record(self):
        self.stop(reason='first stop')
        with self.assertRaisesRegex(Refused, 'did not accept'):
            self.stop(reason='second stop', accepted=False)
        self.assertEqual(STOP['last_stop'](self.log)['reason'], 'first stop')
        self.assertEqual(len(self.log.read_text().splitlines()), 1)

    def test_the_caller_is_found_from_its_process_tree_not_its_own_claim(self):
        panes = '100 @1 agent-a\n200 @2 agent-b\n'
        parents = {510: 505, 505: 100, 700: 1}
        sessions = self.root / 'sessions'
        (sessions / 'agent-a').mkdir(parents=True)
        (sessions / 'agent-a' / 'agent').write_text('codex\n')
        caller = IDLE['caller']
        query = lambda *args: panes
        self.assertEqual(caller(510, query=query, parent_of=parents.get, sessions=sessions),
                         ('@1', 'agent-a', 'codex'))
        # A process outside every agent session cannot stop the worker.
        with self.assertRaisesRegex(Refused, 'Only an agent session'):
            caller(700, query=query, parent_of=lambda pid: parents.get(pid, 0), sessions=sessions)
        # A session without a readable agent binding still identifies its window.
        self.assertEqual(caller(200, query=query, parent_of=parents.get, sessions=sessions), ('@2', 'agent-b', ''))

    def test_windows_other_than_the_callers_count_and_tmux_failures_refuse(self):
        listing = lambda *args: '@1 9990\n@2 9000\n'
        self.assertEqual(IDLE['other_output']('@1', query=listing), 9000)
        self.assertEqual(IDLE['other_output']('@2', query=listing), 9990)
        self.assertIsNone(IDLE['other_output']('@1', query=lambda *args: '@1 9990\n'))

        def broken(*args, **kwargs):
            raise subprocess.TimeoutExpired('tmux', 10)
        with self.assertRaisesRegex(Refused, 'cannot be checked'):
            IDLE['tmux']('list-windows', run=broken)

        def failed(stderr):
            def run(*args, **kwargs):
                raise subprocess.CalledProcessError(1, 'tmux', stderr=stderr)
            return run
        with self.assertRaisesRegex(Refused, 'cannot be checked'):
            IDLE['tmux']('list-windows', run=failed('lost server'))
        # No agent session was ever started: an empty server, so a caller outside one is
        # told it is not an agent session.
        empty = failed('no server running on /tmp/tmux-0/horizon-cloud\n')
        self.assertEqual(IDLE['tmux']('list-panes', run=empty), '')
        with self.assertRaisesRegex(Refused, 'Only an agent session'):
            IDLE['caller'](42, query=lambda *args: IDLE['tmux'](*args, run=empty), parent_of=lambda pid: 1)

    def test_the_parent_of_a_process_comes_from_proc(self):
        proc = self.root / 'proc'
        (proc / '42').mkdir(parents=True)
        (proc / '42' / 'stat').write_text('42 (a (strange) name) S 17 42 42 0 -1\n')
        self.assertEqual(IDLE['parent'](42, proc=proc), 17)
        self.assertEqual(IDLE['parent'](43, proc=proc), 0)

    def test_requests_arrive_over_the_socket_with_the_kernels_caller(self):
        self.assertEqual(IDLE['answer']('not json', 1, 'pod1', 'key'),
                         {'ok': False, 'message': 'Malformed stop request.'})
        path = self.root / 'stop.sock'
        seen = []

        def identify(pid):
            seen.append(pid)
            return REQUESTER

        def stop(reason, requester, pod, key):
            if reason == 'busy':
                raise Refused('This worker is busy')
            return f'Stop requested by {requester[1]}.'
        threading.Thread(target=IDLE['serve_stop_requests'], args=('pod1', 'key', path, stop, identify),
                         daemon=True).start()
        deadline = time.time() + 5
        while not path.exists() and time.time() < deadline:
            time.sleep(0.01)
        request = STOP['request_stop']
        self.assertEqual(request('done', path=path), (True, 'Stop requested by agent-a.'))
        self.assertEqual(request('busy', path=path), (False, 'This worker is busy'))
        self.assertEqual(seen, [os.getpid(), os.getpid()])


class ClientTests(unittest.TestCase):
    def test_without_the_watcher_the_worker_cannot_stop_itself(self):
        accepted, message = STOP['request_stop']('done', path=Path('/nonexistent/stop.sock'))
        self.assertFalse(accepted)
        self.assertIn('idle_stop_minutes', message)

    def test_the_mcp_tool_carries_the_guidance_and_reports_refusals(self):
        respond = STOP['respond']
        started = respond({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize',
                           'params': {'protocolVersion': '2025-06-18'}})
        self.assertEqual(started['result']['protocolVersion'], '2025-06-18')
        self.assertIsNone(respond({'jsonrpc': '2.0', 'method': 'notifications/initialized'}))
        tools = respond({'jsonrpc': '2.0', 'id': 2, 'method': 'tools/list'})['result']['tools']
        self.assertEqual([tool['name'] for tool in tools], ['stop_this_worker'])
        self.assertIn('Never stop the worker to escape a failing', tools[0]['description'])
        call = {'jsonrpc': '2.0', 'id': 3, 'method': 'tools/call',
                'params': {'name': 'stop_this_worker', 'arguments': {'reason': 'PR merged'}}}
        done = respond(call, stop=lambda reason: (True, 'Stop requested for ' + reason))['result']
        self.assertEqual((done['isError'], done['content'][0]['text']), (False, 'Stop requested for PR merged'))
        refused = respond(call, stop=lambda reason: (False, 'busy'))['result']
        self.assertTrue(refused['isError'])
        self.assertEqual(respond({'jsonrpc': '2.0', 'id': 4, 'method': 'resources/list'})['error']['code'], -32601)

    def test_the_server_answers_line_by_line(self):
        stdin = io.StringIO(json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': 'ping'}) + '\nnot json\n')
        stdout = io.StringIO()
        STOP['serve'](stdin, stdout)
        self.assertEqual([json.loads(line) for line in stdout.getvalue().splitlines()],
                         [{'jsonrpc': '2.0', 'id': 1, 'result': {}}])

    def test_last_stop_skips_unreadable_lines_and_fills_a_missing_requester(self):
        with tempfile.TemporaryDirectory() as root:
            log = Path(root, 'self-stops.jsonl')
            self.assertIsNone(STOP['last_stop'](log))
            log.write_text('{"at": 1, "reason": "first"}\n{"at": 2, "reason": "   "}\nbroken\n')
            self.assertEqual(STOP['last_stop'](log), {'at': 1, 'reason': 'first', 'agent': '', 'session': ''})


if __name__ == '__main__':
    unittest.main()
