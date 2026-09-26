"""A dedicated worker stops itself only after its whole idle period without activity."""
import json
from pathlib import Path
import runpy
import subprocess
import tempfile
import unittest
from unittest import mock


MODULE = runpy.run_path(str(Path(__file__).with_name('horizon-worker-idle')))
Activity = MODULE['Activity']


class IdleTests(unittest.TestCase):
    def test_agent_output_and_busy_cpu_postpone_the_stop(self):
        activity = Activity(600, now=1000)
        self.assertFalse(activity.observe(1300, output=None, cpu=10.0))
        self.assertFalse(activity.observe(1500, output=1450, cpu=10.1))
        self.assertFalse(activity.observe(2000, output=1450, cpu=10.2))
        self.assertTrue(activity.observe(2060, output=1450, cpu=10.3))
        # A quiet build keeps the worker busy although no window prints.
        self.assertFalse(activity.observe(2120, output=1450, cpu=10.3 + 60))
        self.assertFalse(activity.observe(2700, output=1450, cpu=70.3))
        self.assertTrue(activity.observe(2720, output=1450, cpu=70.3))

    def test_missing_signals_count_as_idle_from_start(self):
        activity = Activity(600, now=1000)
        self.assertFalse(activity.observe(1599, output=None, cpu=None))
        self.assertTrue(activity.observe(1600, output=None, cpu=None))

    def test_cgroup_usage_is_read_in_seconds(self):
        with tempfile.TemporaryDirectory() as root:
            stat = Path(root, 'cpu.stat')
            stat.write_text('usage_usec 2500000\nuser_usec 2000000\n')
            self.assertEqual(MODULE['cpu_seconds'](stat), 2.5)
            self.assertIsNone(MODULE['cpu_seconds'](Path(root, 'missing')))

    def test_newest_window_output_wins_and_no_server_means_no_sessions(self):
        listing = subprocess.CompletedProcess([], 0, stdout='100\n300\n200\n')
        with mock.patch('subprocess.run', return_value=listing):
            self.assertEqual(MODULE['last_output'](), 300)
        with mock.patch('subprocess.run', side_effect=subprocess.CalledProcessError(1, 'tmux')):
            self.assertIsNone(MODULE['last_output']())

    def test_invalid_settings_stay_passive_instead_of_ending_the_worker(self):
        for environment in [{'HORIZON_IDLE_STOP_MINUTES': '5'},
                            {'HORIZON_IDLE_STOP_MINUTES': '30'}]:
            with mock.patch.dict('os.environ', environment, clear=True), \
                    mock.patch.dict(MODULE['main'].__globals__, {'passive': mock.Mock(side_effect=SystemExit)}):
                with self.assertRaises(SystemExit):
                    MODULE['main']()
                MODULE['main'].__globals__['passive'].assert_called_once()

    def test_stop_targets_only_this_worker_with_its_own_credential(self):
        def reply(payload):
            response = mock.MagicMock(status=200)
            response.__enter__.return_value = response
            response.read.return_value = json.dumps(payload).encode()
            return response
        stopped = {'data': {'podStop': {'id': 'pod123', 'desiredStatus': 'EXITED'}}}
        with mock.patch('urllib.request.urlopen', return_value=reply(stopped)) as opened:
            self.assertTrue(MODULE['stop_worker']('pod123', 'pod-scoped'))
        request = opened.call_args[0][0]
        self.assertEqual(request.full_url, 'https://api.runpod.io/graphql')
        self.assertEqual(json.loads(request.data)['variables'], {'pod': 'pod123'})
        self.assertEqual(request.get_header('Authorization'), 'Bearer pod-scoped')
        self.assertEqual(request.get_header('User-agent'), 'horizon-worker-idle/1')
        for refused in [{'errors': [{'message': 'denied'}]}, {'data': {'podStop': None}},
                        {'data': {'podStop': {'id': 'other'}}}, {'data': ['unexpected']}, {'data': None}]:
            with mock.patch('urllib.request.urlopen', return_value=reply(refused)):
                self.assertFalse(MODULE['stop_worker']('pod123', 'pod-scoped'))

if __name__ == '__main__':
    unittest.main()
