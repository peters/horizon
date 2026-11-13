#!/usr/bin/env python3
"""A fixture run keeps no Horizon identity of the shell that started it.

A run that a person starts inside a panel of their own Horizon inherits the
browser actor and the work identity of that panel. A panel of the fixture would
then carry the identity of a panel of another board, which the board of the
fixture cannot answer, so every request of that panel is refused as an unknown
caller. The fixture removes those variables and sets the values of its own run.
These tests need no display and no bwrap.
"""
from __future__ import annotations

from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import sandbox  # noqa: E402

# The variables of a Horizon panel, as a shell inside one panel holds them.
INHERITED = {
    'PATH': '/usr/bin:/bin',
    'HORIZON': '1',
    'HORIZON_HOME': '/home/someone/.horizon',
    'HORIZON_BROWSER_ACTOR': 'horizon:30406ea523c53def',
    'HORIZON_BROWSER_HOST_INSTANCE': '493bd8b4-0d97-4a24-9f4b-2ad1a0a0c0d1',
    'HORIZON_BROWSER_LATENCY': '1',
    'HORIZON_TERMINAL_LATENCY': '1',
    'HORIZON_WORK_OWNER': 'someone',
    'HORIZON_WORK_ROOT': '/home/someone/.horizon/work',
    'HORIZON_WORK_KIND': 'codex',
    'HORIZON_WORK_PANEL': 'panel-3',
    'HORIZON_WORK_EXECUTABLE': '/home/someone/.local/bin/horizon',
    'DISPLAY': ':0',
}


class StrippedEnvironment(unittest.TestCase):
    def test_the_horizon_variables_of_the_calling_shell_are_removed(self):
        stripped = sandbox.without_inherited_horizon(INHERITED)

        self.assertEqual(stripped, {
            'PATH': '/usr/bin:/bin',
            'HORIZON_BROWSER_LATENCY': '1',
            'HORIZON_TERMINAL_LATENCY': '1',
        })

    def test_the_trace_switches_of_the_document_survive_the_strip(self):
        # `docs/latency-trace.md` names the switches of the two actions. A
        # fixture that removed them would leave a person without the numbers
        # that the document promises, and no message would say why.
        document = (Path(__file__).resolve().parents[3] / 'docs/latency-trace.md').read_text()
        switches = {name.strip('`') for name in __import__('re').findall(r'`(HORIZON_[A-Z_]+)=1`', document)}

        self.assertEqual(set(sandbox.TRACE_SWITCHES), switches)
        for switch in switches:
            self.assertIn(switch, sandbox.without_inherited_horizon({switch: '1'}))

    def test_the_shell_of_the_test_keeps_its_own_dictionary(self):
        sandbox.without_inherited_horizon(INHERITED)

        self.assertEqual(INHERITED['HORIZON_BROWSER_ACTOR'], 'horizon:30406ea523c53def')

    def test_a_fixture_run_inherits_none_of_them(self):
        with tempfile.TemporaryDirectory() as parent:
            state = sandbox.prepare_state(Path(parent) / 'run', reuse=False)
            box = sandbox.prepare(state, environ=INHERITED)

            for child_env in (box.host_env, box.sandbox_env):
                for key in INHERITED:
                    # The path is not an identity, and the trace switches are the
                    # diagnostic of the run that starts the fixture.
                    if key in ('PATH', 'DISPLAY') or key in sandbox.TRACE_SWITCHES:
                        continue
                    self.assertNotIn(key, child_env)
            self.assertEqual(box.host_env['PATH'], '/usr/bin:/bin')

    def test_a_fixture_run_keeps_the_values_that_it_sets_itself(self):
        with tempfile.TemporaryDirectory() as parent:
            state = sandbox.prepare_state(Path(parent) / 'run', reuse=False)
            box = sandbox.prepare(state, environ=INHERITED)

            self.assertEqual(box.host_env['XDG_DATA_HOME'], str(box.data))
            self.assertEqual(box.host_env['HISTFILE'], '/dev/null')
            self.assertEqual(box.sandbox_env['XDG_RUNTIME_DIR'], sandbox.runtime_dest())


if __name__ == '__main__':
    unittest.main()
