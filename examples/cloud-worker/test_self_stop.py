"""An agent whose task is done stops its worker through the stop service, never while
another agent is working, and the reason and requester survive the stop. Where the worker
holds no stop credential, the service records the request and Horizon takes it."""
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
from unittest import mock


ROOT = Path(__file__).parent
STOPD = runpy.run_path(str(ROOT / 'horizon-worker-stopd'))
STOP = runpy.run_path(str(ROOT / 'horizon-worker-stop'))
Refused = STOPD['Refused']
REQUESTER = ('@1', 'agent-a', 'claude')


class ServiceTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.log = self.root / 'workspace' / '.horizon' / 'self-stops.jsonl'
        self.requests = []
        self.steps = []

    def stop(self, reason='PR 12 merged', output=None, cores=0.1, accepted=True):
        def request(pod, key):
            self.requests.append((pod, key))
            return accepted

        def sample():
            self.steps.append('cores')
            return cores

        def newest(own):
            self.steps.append(('output', own))
            return output
        return STOPD['self_stop'](reason, REQUESTER, 'pod1', 'pod-key', log=self.log, now=lambda: 10_000.0,
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
            self.stop(cores=STOPD['BUSY_CORES'])
        # Unknown CPU use is not idle: a quiet build might be running.
        with self.assertRaisesRegex(Refused, 'cannot measure its CPU use'):
            self.stop(cores=None)
        with self.assertRaisesRegex(Refused, 'short reason'):
            self.stop(reason='  ')
        # Only text is a reason; other JSON values fail closed.
        for invalid in [123, {'text': 'done'}, ['done'], None, True]:
            with self.assertRaisesRegex(Refused, 'short reason'):
                self.stop(reason=invalid)
        self.assertEqual(self.requests, [])
        self.assertFalse(self.log.exists())

    def test_a_stop_log_that_cannot_be_written_refuses_the_stop(self):
        def full(*args, **kwargs):
            raise OSError('No space left on device')
        answered = STOPD['answer']('{"reason": "done"}', 1, stop=full, identify=lambda pid: REQUESTER)
        self.assertFalse(answered['ok'])
        self.assertIn('could not be recorded', answered['message'])

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
        caller = STOPD['caller']
        query = lambda *args: panes
        self.assertEqual(caller(510, query=query, parent_of=parents.get, sessions=sessions),
                         ('@1', 'agent-a', 'codex'))
        # A process outside every agent session cannot stop the worker.
        with self.assertRaisesRegex(Refused, 'Only an agent session'):
            caller(700, query=query, parent_of=lambda pid: parents.get(pid, 0), sessions=sessions)
        # A pane without a readable agent binding, or bound to a plain shell, is not an
        # agent session.
        with self.assertRaisesRegex(Refused, 'Only an agent session'):
            caller(200, query=query, parent_of=lambda pid: parents.get(pid, 0), sessions=sessions)
        (sessions / 'agent-b').mkdir()
        (sessions / 'agent-b' / 'agent').write_text('shell\n')
        with self.assertRaisesRegex(Refused, 'Only an agent session'):
            caller(200, query=query, parent_of=lambda pid: parents.get(pid, 0), sessions=sessions)

    def test_a_binding_that_is_a_pipe_or_a_link_is_no_agent_and_holds_nothing(self):
        panes = '100 @1 agent-a\n'
        sessions = self.root / 'sessions'
        (sessions / 'agent-a').mkdir(parents=True)
        target = self.root / 'claude-binding'
        target.write_text('claude\n')
        binding = sessions / 'agent-a' / 'agent'
        for place in (lambda: os.mkfifo(binding), lambda: binding.symlink_to(target)):
            place()
            # A pipe nobody writes would hold a plain read for good.
            with self.assertRaisesRegex(Refused, 'Only an agent session'):
                STOPD['caller'](100, query=lambda *args: panes, parent_of=lambda pid: 0, sessions=sessions)
            binding.unlink()

    def test_a_client_that_trickles_its_request_is_cut_off(self):
        class Trickle:
            def __init__(self, data):
                self.data = data
            def settimeout(self, seconds):
                self.timeout = seconds
            def recv(self, size):
                clock[0] += 10
                chunk, self.data = self.data[:1], self.data[1:]
                return chunk
        clock = [0.0]
        tick = lambda: clock[0]
        self.assertEqual(STOPD['request_line'](Trickle(b'{"a"}\nrest'), clock=tick, seconds=1000), '{"a"}')
        clock[0] = 0.0
        with self.assertRaises(TimeoutError):
            STOPD['request_line'](Trickle(b'{"reason": "done"}\n'), clock=tick)
        # A client that closes early gives what it sent.
        self.assertEqual(STOPD['request_line'](Trickle(b'{}'), clock=tick, seconds=1000), '{}')

    def test_windows_other_than_the_callers_count_and_tmux_failures_refuse(self):
        listing = lambda *args: '@1 9990\n@2 9000\n'
        self.assertEqual(STOPD['other_output']('@1', query=listing), 9000)
        self.assertEqual(STOPD['other_output']('@2', query=listing), 9990)
        self.assertIsNone(STOPD['other_output']('@1', query=lambda *args: '@1 9990\n'))

        def broken(*args, **kwargs):
            raise subprocess.TimeoutExpired('tmux', 10)
        with self.assertRaisesRegex(Refused, 'cannot be checked'):
            STOPD['tmux']('list-windows', run=broken)

        def failed(stderr):
            def run(*args, **kwargs):
                raise subprocess.CalledProcessError(1, 'tmux', stderr=stderr)
            return run
        with self.assertRaisesRegex(Refused, 'cannot be checked'):
            STOPD['tmux']('list-windows', run=failed('lost server'))
        # No agent session was ever started: an empty server, so a caller outside one is
        # told it is not an agent session.
        empty = failed('no server running on /tmp/tmux-0/horizon-cloud\n')
        self.assertEqual(STOPD['tmux']('list-panes', run=empty), '')
        with self.assertRaisesRegex(Refused, 'Only an agent session'):
            STOPD['caller'](42, query=lambda *args: STOPD['tmux'](*args, run=empty), parent_of=lambda pid: 1)

    def test_the_parent_of_a_process_comes_from_proc(self):
        proc = self.root / 'proc'
        (proc / '42').mkdir(parents=True)
        (proc / '42' / 'stat').write_text('42 (a (strange) name) S 17 42 42 0 -1\n')
        self.assertEqual(STOPD['parent'](42, proc=proc), 17)
        self.assertEqual(STOPD['parent'](43, proc=proc), 0)

    def test_requests_arrive_over_the_socket_with_the_kernels_caller(self):
        self.assertEqual(STOPD['answer']('not json', 1, stop=None),
                         {'ok': False, 'message': 'Malformed stop request.'})
        path = self.root / 'stop.sock'
        seen = []

        def identify(pid):
            seen.append(pid)
            return REQUESTER

        def stop(reason, requester):
            if reason == 'busy':
                raise Refused('This worker is busy')
            return f'Stop requested by {requester[1]}.'
        threading.Thread(target=STOPD['serve_stop_requests'], args=(stop, path, identify), daemon=True).start()
        deadline = time.time() + 5
        while not path.exists() and time.time() < deadline:
            time.sleep(0.01)
        request = STOP['request_stop']
        self.assertEqual(request('done', path=path), (True, 'Stop requested by agent-a.'))
        self.assertEqual(request('busy', path=path), (False, 'This worker is busy'))
        self.assertEqual(seen, [os.getpid(), os.getpid()])


class ModeTests(unittest.TestCase):
    def test_every_dedicated_worker_offers_the_stop_but_a_shared_or_device_one_does_not(self):
        mode = lambda environment: STOPD['mode']({'HORIZON_WORKER_CAPABILITIES': '{"agents":["claude"]}',
                                                  **environment})
        credential = {'RUNPOD_POD_ID': 'pod1', 'RUNPOD_API_KEY': 'key'}
        self.assertEqual(mode(credential), 'self')
        # Agent sessions see the availability flag in place of the credential.
        self.assertEqual(mode({'HORIZON_WORKER_SELF_STOP_AVAILABLE': '1'}), 'self')
        self.assertEqual(mode({'HORIZON_WORKER_STOP': 'horizon'}), 'horizon')
        self.assertIsNone(mode({}), 'neither the provider nor Horizon can stop it')
        self.assertIsNone(mode({'HORIZON_WORKER_STOP': 'other'}))
        # Capabilities the environment does not name might hold hosted devices.
        self.assertIsNone(STOPD['mode'](credential))
        self.assertIsNone(STOPD['mode']({**credential, 'HORIZON_WORKER_CAPABILITIES': ''}))
        # An idle period no longer matters either way.
        self.assertEqual(mode({**credential, 'HORIZON_IDLE_STOP_MINUTES': '30'}), 'self')
        self.assertIsNone(mode({**credential, 'HORIZON_WORKER_STARTUP': 'shared'}), 'a shared worker')
        devices = json.dumps({'agents': ['claude'], 'browserstack': {'account': 'synthetic'}})
        self.assertIsNone(mode({**credential, 'HORIZON_WORKER_CAPABILITIES': devices}), 'hosted devices')
        self.assertIsNone(mode({**credential, 'HORIZON_WORKER_CAPABILITIES': 'not json'}))
        plain = json.dumps({'agents': ['claude'], 'browserstack': None})
        self.assertEqual(mode({'HORIZON_WORKER_STOP': 'horizon', 'HORIZON_WORKER_CAPABILITIES': plain}), 'horizon')

    def test_the_supervisor_declares_that_it_starts_the_service(self):
        declared = subprocess.run(['python3', str(ROOT / 'horizon-worker-supervise'), '--agent-stop-contract'],
                                  capture_output=True, timeout=10)
        self.assertEqual((declared.returncode, declared.stdout), (0, b'horizon-agent-stop-contract=1\n'))

    def test_only_the_service_options_are_accepted(self):
        with mock.patch('builtins.print') as printed:
            self.assertEqual(STOPD['entry'](['--contract']), 0)
        printed.assert_called_once_with('horizon-agent-stop-contract=1')
        with mock.patch('sys.stderr'):
            self.assertEqual(STOPD['entry'](['--report']), 2)

    def test_without_an_offered_stop_the_service_stays_passive_instead_of_ending_the_worker(self):
        with tempfile.TemporaryDirectory() as root:
            stale = Path(root, 'stop-request.json')
            stale.write_text('{}')
            with mock.patch.dict('os.environ', {}, clear=True), \
                    mock.patch.dict(STOPD['main'].__globals__, {'serve_stop_requests': mock.Mock(), 'REQUEST': stale,
                                                                'REQUEST_LOCK': Path(root, 'lock')}), \
                    mock.patch('time.sleep', side_effect=SystemExit):
                with self.assertRaises(SystemExit):
                    STOPD['main']()
                STOPD['main'].__globals__['serve_stop_requests'].assert_not_called()
            self.assertFalse(stale.exists(), 'a request from before the start is dropped')


class HorizonStopTests(unittest.TestCase):
    """Where the worker holds no stop credential, Horizon carries the request out."""

    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        self.request = root / 'run' / 'stop-request.json'
        self.lock = root / 'run' / 'stop-request.lock'
        self.log = root / 'workspace' / '.horizon' / 'self-stops.jsonl'
        self.clock = 10_000.0

    def record(self, reason='PR 12 merged', output=None, cores=0.1):
        return STOPD['record_request'](reason, REQUESTER, path=self.request, lock=self.lock, now=lambda: self.clock,
                                       output=lambda own: output, cores=lambda: cores, log=self.log)

    def take(self, output=None, cores=0.1, windows='@1 agent-a\n@2 agent-b\n'):
        def newest(own):
            self.taken_window = own
            return output
        return STOPD['take'](path=self.request, lock=self.lock, log=self.log, now=lambda: self.clock,
                             output=newest, cores=lambda: cores, query=lambda *args: windows)

    def test_a_checked_request_is_recorded_and_taken_once_onto_the_stop_log(self):
        message = self.record()
        self.assertIn('Horizon', message)
        self.assertIn('while Horizon runs', message)
        self.assertEqual(self.request.stat().st_mode & 0o777, 0o600)
        self.assertFalse(self.log.exists(), 'nothing is on record before Horizon stops the worker')
        self.clock += 30
        taken = self.take(output=self.clock - 600)
        self.assertEqual(taken, {'at': 10_000_000, 'reason': 'PR 12 merged', 'session': 'agent-a',
                                 'agent': 'claude'})
        self.assertEqual(STOP['last_stop'](self.log), taken)
        # Until the worker stops, a later take gives the same stop again, so an answer lost
        # on the way to Horizon loses no stop; the log keeps it once.
        self.assertEqual(self.take(output=self.clock - 5), taken, 'no new check')
        self.assertEqual(len(self.log.read_text().splitlines()), 1)
        # A taken stop that never happened lapses off the log, ten minutes after its take.
        self.clock += STOPD['REQUEST_SECONDS'] + 1
        self.assertIn('lapsed', self.take()['refused'])
        self.assertIsNone(STOP['last_stop'](self.log))
        self.assertIsNone(self.take())

    def test_a_request_is_refused_when_made_and_again_when_taken(self):
        with self.assertRaisesRegex(Refused, 'Another agent session'):
            self.record(output=self.clock - 30)
        with self.assertRaisesRegex(Refused, 'busy'):
            self.record(cores=STOPD['BUSY_CORES'])
        self.assertFalse(self.request.exists())
        # Another session started to work after the request: Horizon leaves the worker running
        # and takes the request again later, until it lapses.
        self.record()
        self.clock += 60
        self.assertIn('An agent session', self.take(output=self.clock - 10)['waiting'])
        self.assertTrue(self.request.exists())
        self.assertFalse(self.log.exists())
        self.assertIn('busy', self.take(cores=STOPD['BUSY_CORES'])['waiting'])
        # The requester's own output counts at the take too: it may have taken up new work.
        self.assertIsNone(self.taken_window, 'every window counts')
        self.clock += 300
        self.assertEqual(self.take(output=self.clock - 600)['reason'], 'PR 12 merged')

    def test_a_stop_horizon_could_not_start_is_taken_back(self):
        self.record()
        taken = self.take()
        untake = lambda at, log=self.log: STOPD['untake'](at, path=self.request, log=log, lock=self.lock)
        self.assertFalse(untake(taken['at'] + 1), 'only the stop it took')
        self.assertTrue(self.request.exists())
        self.assertTrue(untake(taken['at']))
        self.assertIsNone(STOP['last_stop'](self.log))
        self.assertFalse(self.request.exists(), 'nor is it taken again')
        self.assertIsNone(self.take())
        self.assertFalse(untake(taken['at']), 'once')
        self.assertFalse(untake(1, log=self.log.with_name('missing.jsonl')))

    def test_a_request_recorded_while_one_is_taken_is_not_lost(self):
        self.record(reason='first')
        held = STOPD['locked'](self.lock)
        recorded = threading.Thread(target=self.record, kwargs={'reason': 'second'})
        recorded.start()
        recorded.join(0.2)
        self.assertTrue(recorded.is_alive(), 'the request waits for the take')
        self.assertEqual(json.loads(self.request.read_text())['reason'], 'first')
        held.close()
        recorded.join(5)
        self.assertEqual(json.loads(self.request.read_text())['reason'], 'second')

    def test_a_take_whose_taken_state_cannot_be_saved_leaves_no_stop_on_the_log(self):
        self.record()
        self.clock += 130
        failing = mock.patch.dict(STOPD['take'].__globals__, write_request=mock.Mock(side_effect=OSError('full')))
        with failing:
            self.assertIn('could not be recorded', self.take()['waiting'])
        self.assertEqual(self.log.read_text(), '')
        # The next take logs the stop once, and taking it back leaves nothing behind.
        taken = self.take()
        self.assertEqual(taken['reason'], 'PR 12 merged')
        self.assertEqual(len(self.log.read_text().splitlines()), 1)
        self.assertTrue(STOPD['untake'](taken['at'], path=self.request, log=self.log, lock=self.lock))
        self.assertIsNone(STOP['last_stop'](self.log))

    def test_a_take_or_untake_cut_short_never_logs_a_stop_twice_or_keeps_one_that_did_not_happen(self):
        self.record()
        self.clock += 130
        record = json.loads(self.request.read_text())
        stop = {key: record[key] for key in ('at', 'reason', 'session', 'agent')}
        # A take that ended after it logged the stop, before it saved the taken state.
        STOPD['append'](self.log, stop)
        self.assertEqual(self.take(), stop)
        self.assertEqual(len(self.log.read_text().splitlines()), 1)
        # An untake that cannot change the log keeps the request, so the stop is given again.
        with mock.patch.dict(STOPD['untake'].__globals__, remove_newest=mock.Mock(return_value=False)):
            self.assertFalse(STOPD['untake'](stop['at'], path=self.request, log=self.log, lock=self.lock))
        self.assertEqual(self.take(), stop)
        # An untake that ended after it emptied the log: the next take logs the stop again.
        STOPD['remove_newest'](self.log, stop['at'])
        self.assertEqual(self.take(), stop)
        self.assertEqual(len(self.log.read_text().splitlines()), 1)
        self.assertTrue(STOPD['untake'](stop['at'], path=self.request, log=self.log, lock=self.lock))
        self.assertFalse(self.request.exists())
        self.assertEqual(self.log.read_text(), '')

    def test_a_line_that_cannot_be_written_whole_leaves_the_log_as_it_was(self):
        STOPD['append'](self.log, {'at': 1})
        before = self.log.read_bytes()
        real = os.fdopen

        def partial(descriptor, *args, **kwargs):
            file = real(descriptor, *args, **kwargs)
            def write(text):
                os.write(descriptor, text[:5].encode())
                raise OSError('full')
            file.write = write
            return file
        with mock.patch.object(STOPD['append'].__globals__['os'], 'fdopen', partial):
            with self.assertRaises(OSError):
                STOPD['append'](self.log, {'at': 2, 'reason': 'x' * 50})
        self.assertEqual(self.log.read_bytes(), before)

    def test_a_taken_stop_lapses_from_its_take_not_from_its_request(self):
        self.record()
        self.clock += STOPD['REQUEST_SECONDS'] - 5
        self.assertEqual(self.take()['reason'], 'PR 12 merged')
        # Past ten minutes from the request, but not from the take: the stop still counts.
        self.clock += 10
        self.assertEqual(self.take()['reason'], 'PR 12 merged')
        with self.assertRaisesRegex(Refused, 'already stopping'):
            self.record(reason='second')
        self.assertEqual(len(self.log.read_text().splitlines()), 1)
        self.clock += STOPD['REQUEST_SECONDS']
        self.assertIn('lapsed', self.take()['refused'])
        self.assertEqual(self.log.read_text(), '')

    def test_a_request_while_horizon_carries_out_a_taken_one_is_refused(self):
        self.record(reason='first')
        self.assertEqual(self.take()['reason'], 'first')
        with self.assertRaisesRegex(Refused, 'already stopping this worker at the request of session agent-a'):
            self.record(reason='second')
        self.assertEqual(json.loads(self.request.read_text())['reason'], 'first')
        # A taken stop that lapsed leaves the log and no longer holds new requests.
        self.clock += STOPD['REQUEST_SECONDS'] + 1
        self.record(reason='second')
        self.assertEqual(json.loads(self.request.read_text())['reason'], 'second')
        self.assertEqual(self.log.read_text(), '')

    def test_the_stop_log_is_never_written_through_a_link_on_the_volume(self):
        self.log.parent.mkdir(parents=True)
        target = self.log.parent.parent / 'outside.txt'
        target.write_text('untouched\n')
        self.log.symlink_to(target)
        self.record()
        self.assertIn('could not be recorded', self.take()['waiting'])
        self.assertEqual(target.read_text(), 'untouched\n')
        self.assertFalse(STOPD['untake'](1, path=self.request, log=self.log, lock=self.lock))
        self.assertEqual(target.read_text(), 'untouched\n')
        # Nor through a linked folder.
        self.log.unlink()
        self.log.parent.rmdir()
        elsewhere = self.log.parent.parent / 'elsewhere'
        elsewhere.mkdir()
        self.log.parent.symlink_to(elsewhere)
        self.record()
        self.assertIn('could not be recorded', self.take()['waiting'])
        self.assertEqual(list(elsewhere.iterdir()), [])

    def test_a_window_another_session_now_holds_is_not_the_requesters(self):
        self.record()
        # The tmux server restarted and window @1 belongs to another session now.
        self.assertIn('has ended', self.take(windows='@1 agent-c\n')['refused'])
        # The requester's session ended and nobody reuses its window: the stop still counts.
        self.record()
        self.assertEqual(self.take(windows='@2 agent-b\n')['session'], 'agent-a')

    def test_a_pipe_or_a_hard_link_as_the_stop_log_is_refused_without_waiting(self):
        self.log.parent.mkdir(parents=True)
        os.mkfifo(self.log)
        self.record()
        self.assertIn('could not be recorded', self.take()['waiting'])
        self.assertFalse(STOPD['untake'](1, path=self.request, log=self.log, lock=self.lock))
        self.log.unlink()
        other = self.log.with_name('other.jsonl')
        other.write_text('')
        os.link(other, self.log)
        self.record()
        self.assertIn('could not be recorded', self.take()['waiting'])
        self.assertEqual(other.read_text(), '')

    def test_a_request_that_cannot_be_written_is_refused_to_the_agent(self):
        blocked = self.request.parent
        blocked.parent.mkdir(parents=True, exist_ok=True)
        blocked.write_text('not a folder')
        with self.assertRaisesRegex(Refused, 'could not be recorded'):
            STOPD['record_request']('done', REQUESTER, path=blocked / 'stop-request.json',
                                    lock=blocked / 'stop-request.lock', now=lambda: self.clock,
                                    output=lambda own: None, cores=lambda: 0.1)

    def test_the_request_lock_is_private_and_never_follows_a_link(self):
        old = os.umask(0)
        try:
            with STOPD['locked'](self.lock):
                pass
        finally:
            os.umask(old)
        self.assertEqual(self.lock.stat().st_mode & 0o777, 0o600)
        # A link in its place, as a planted one would be, is refused, not followed.
        target = self.lock.parent / 'elsewhere'
        self.lock.unlink()
        self.lock.symlink_to(target)
        with self.assertRaises(OSError):
            STOPD['locked'](self.lock)
        self.assertFalse(target.exists())
        with self.assertRaisesRegex(Refused, 'could not be recorded'):
            self.record()

    def test_a_lapsed_or_malformed_request_is_dropped(self):
        self.record()
        self.clock += STOPD['REQUEST_SECONDS'] + 1
        self.assertIn('lapsed', self.take()['refused'])
        self.assertFalse(self.request.exists())
        self.request.write_text('{"at": "soon"}')
        self.assertIn('malformed', self.take()['refused'])
        self.request.write_text('not json')
        self.assertIn('malformed', self.take()['refused'])
        self.assertFalse(self.log.exists())


class ClientTests(unittest.TestCase):
    def test_without_the_service_the_worker_cannot_stop(self):
        accepted, message = STOP['request_stop']('done', path=Path('/nonexistent/stop.sock'))
        self.assertFalse(accepted)
        self.assertIn('shared with other clouds', message)

    def test_the_mcp_tool_carries_the_guidance_and_reports_refusals(self):
        respond = STOP['respond']
        started = respond({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize',
                           'params': {'protocolVersion': '2025-06-18'}})
        self.assertEqual(started['result']['protocolVersion'], '2025-06-18')
        # A version this server does not implement is answered with its newest one.
        newer = respond({'jsonrpc': '2.0', 'id': 5, 'method': 'initialize',
                         'params': {'protocolVersion': '2026-07-28'}})
        self.assertEqual(newer['result']['protocolVersion'], '2025-11-25')
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
            log.write_text('{"at": 1, "reason": "first"}\n{"at": 2, "reason": "   "}\nbroken\n'
                           '{"at": -3, "reason": "negative"}\n{"at": true, "reason": "flag"}\n'
                           '{"at": 4, "reason": "line\\nbreak"}\n{"at": 5, "reason": "' + 'x' * 201 + '"}\n')
            self.assertEqual(STOP['last_stop'](log), {'at': 1, 'reason': 'first', 'agent': '', 'session': ''})
            # A log with lines but none readable is not "no stop recorded" either.
            corrupt = Path(root, 'corrupt.jsonl')
            corrupt.write_text('broken\n{"at": -1, "reason": "x"}\n')
            with self.assertRaises(OSError):
                STOP['last_stop'](corrupt)
            empty = Path(root, 'empty.jsonl')
            empty.write_text('\n')
            self.assertIsNone(STOP['last_stop'](empty))
            # A log that exists but cannot be read is not "no stop recorded".
            unreadable = Path(root, 'directory.jsonl')
            unreadable.mkdir()
            with self.assertRaises(OSError):
                STOP['last_stop'](unreadable)


class ProviderTests(unittest.TestCase):
    def test_cgroup_v2_and_v1_usage_is_read_in_seconds(self):
        with tempfile.TemporaryDirectory() as root:
            stat = Path(root, 'cpu.stat')
            stat.write_text('usage_usec 2500000\nuser_usec 2000000\n')
            self.assertEqual(STOPD['cpu_seconds'](stat), 2.5)
            missing = Path(root, 'missing')
            self.assertIsNone(STOPD['cpu_seconds'](missing, accounting=(missing,)))
            # cgroup v1 hosts report nanoseconds in cpuacct.usage.
            usage = Path(root, 'cpuacct.usage')
            usage.write_text('1216209670\n')
            self.assertAlmostEqual(STOPD['cpu_seconds'](missing, accounting=(missing, usage)), 1.21620967)

    def test_isolated_tmux_clients_clear_root_environment_before_dropping_uid(self):
        with mock.patch('os.geteuid', return_value=0), mock.patch.object(Path, 'exists', return_value=True), \
                mock.patch.dict(os.environ, {'RUNPOD_API_KEY': 'synthetic-provider-secret',
                                             'TS_AUTHKEY': 'synthetic-tailnet-secret'}, clear=True), \
                mock.patch('subprocess.run', return_value=subprocess.CompletedProcess([], 0, stdout='42\n')) as run:
            self.assertEqual(STOPD['tmux']('list-windows', run=run), '42\n')
        for call in run.call_args_list:
            command = call.args[0]
            self.assertEqual(command[:4], ['env', '-i', 'PATH=/usr/local/bin:/usr/bin:/bin', 'HOME=/workspace/home'])
            self.assertEqual(command[4], 'setpriv')
            self.assertIn('--no-new-privs', command)
            self.assertIn('--bounding-set=-all', command)
            self.assertNotIn('synthetic-provider-secret', ' '.join(command))
            self.assertNotIn('synthetic-tailnet-secret', ' '.join(command))

    def test_stop_targets_only_this_worker_with_its_own_credential(self):
        def reply(payload):
            response = mock.MagicMock(status=200)
            response.__enter__.return_value = response
            response.read.return_value = json.dumps(payload).encode()
            return response
        stopped = {'data': {'podStop': {'id': 'pod123', 'desiredStatus': 'EXITED'}}}
        with mock.patch('urllib.request.urlopen', return_value=reply(stopped)) as opened:
            self.assertTrue(STOPD['stop_worker']('pod123', 'pod-scoped'))
        request = opened.call_args[0][0]
        self.assertEqual(request.full_url, 'https://api.runpod.io/graphql')
        self.assertEqual(json.loads(request.data)['variables'], {'pod': 'pod123'})
        self.assertEqual(request.get_header('Authorization'), 'Bearer pod-scoped')
        self.assertEqual(request.get_header('User-agent'), 'horizon-worker-stopd/1')
        for refused in [{'errors': [{'message': 'denied'}]}, {'data': {'podStop': None}},
                        {'data': {'podStop': {'id': 'other'}}}, {'data': ['unexpected']}, {'data': None},
                        {'data': {'podStop': {'id': 'pod123', 'desiredStatus': 'RUNNING'}}},
                        {'data': {'podStop': {'id': 'pod123'}}}]:
            with mock.patch('urllib.request.urlopen', return_value=reply(refused)):
                self.assertFalse(STOPD['stop_worker']('pod123', 'pod-scoped'))


if __name__ == '__main__':
    unittest.main()
