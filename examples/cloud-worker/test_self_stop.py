"""An agent whose task is done stops its worker through the idle watcher, never while
another agent is working, and the reason survives the stop."""
import io
import json
from pathlib import Path
import runpy
import tempfile
import threading
import time
import unittest


ROOT = Path(__file__).parent
IDLE = runpy.run_path(str(ROOT / 'horizon-worker-idle'))
STOP = runpy.run_path(str(ROOT / 'horizon-worker-stop'))
Refused = IDLE['Refused']


class WatcherTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.log = Path(temp.name, 'workspace', '.horizon', 'self-stops.jsonl')
        self.requests = []

    def stop(self, reason='PR 12 merged', window='@1', output=None, cores=None, accepted=True):
        def request(pod, key):
            self.requests.append((pod, key))
            return accepted
        return IDLE['self_stop'](reason, window, 'pod1', 'pod-key', log=self.log, now=lambda: 10_000.0,
                                 output=lambda own: output, cores=lambda: cores, request=request)

    def test_a_quiet_worker_records_the_reason_then_stops(self):
        self.assertIn('Stop requested', self.stop(output=10_000 - 600, cores=0.1))
        self.assertEqual(self.requests, [('pod1', 'pod-key')])
        record = json.loads(self.log.read_text())
        self.assertEqual(record, {'at': 10_000_000, 'reason': 'PR 12 merged'})
        self.assertEqual(STOP['last_stop'](self.log), record)

    def test_another_working_agent_or_a_busy_worker_refuses_the_stop(self):
        with self.assertRaisesRegex(Refused, 'Another agent session'):
            self.stop(output=10_000 - 30)
        with self.assertRaisesRegex(Refused, 'busy'):
            self.stop(cores=IDLE['BUSY_CORES'])
        with self.assertRaisesRegex(Refused, 'short reason'):
            self.stop(reason='  ')
        self.assertEqual(self.requests, [])
        self.assertFalse(self.log.exists())

    def test_the_callers_own_window_is_not_other_activity(self):
        listing = '@1 9990\n@2 9000\n'
        other = IDLE['other_output']
        original = IDLE['subprocess'].run
        IDLE['subprocess'].run = lambda *args, **kwargs: IDLE['subprocess'].CompletedProcess(args, 0, stdout=listing)
        try:
            self.assertEqual(other('@1'), 9000)
            self.assertEqual(other('@2'), 9990)
        finally:
            IDLE['subprocess'].run = original

    def test_a_refused_provider_request_leaves_no_record(self):
        self.stop(reason='first stop')
        with self.assertRaisesRegex(Refused, 'did not accept'):
            self.stop(reason='second stop', accepted=False)
        self.assertEqual(STOP['last_stop'](self.log)['reason'], 'first stop')
        self.assertEqual(len(self.log.read_text().splitlines()), 1)

    def test_requests_arrive_over_the_socket_and_malformed_ones_are_answered(self):
        self.assertEqual(IDLE['answer']('not json', 'pod1', 'key'), {'ok': False, 'message': 'Malformed stop request.'})
        with tempfile.TemporaryDirectory() as root:
            path = Path(root, 'stop.sock')
            seen = []

            def stop(reason, window, pod, key):
                seen.append((reason, window, pod, key))
                if reason == 'busy':
                    raise Refused('This worker is busy')
                return 'Stop requested.'
            threading.Thread(target=IDLE['serve_stop_requests'], args=('pod1', 'key', path, stop),
                             daemon=True).start()
            deadline = time.time() + 5
            while not path.exists() and time.time() < deadline:
                time.sleep(0.01)
            request = STOP['request_stop']
            self.assertEqual(request('done', path=path, window=lambda: '@4'), (True, 'Stop requested.'))
            self.assertEqual(request('busy', path=path, window=lambda: None), (False, 'This worker is busy'))
            self.assertEqual(seen[0], ('done', '@4', 'pod1', 'key'))


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

    def test_last_stop_skips_unreadable_lines(self):
        with tempfile.TemporaryDirectory() as root:
            log = Path(root, 'self-stops.jsonl')
            self.assertIsNone(STOP['last_stop'](log))
            log.write_text('{"at": 1, "reason": "first"}\n{"at": 2, "reason": "   "}\nbroken\n')
            self.assertEqual(STOP['last_stop'](log), {'at': 1, 'reason': 'first'})


if __name__ == '__main__':
    unittest.main()
