#!/usr/bin/env python3
"""A panel of the fixture writes its temporary files inside the fixture state.

The namespace of the fixture binds one writable `/tmp` and keeps every other
folder of the host read-only. A panel that inherits the `TMPDIR` of the person
therefore names a folder that no program inside the fixture can write, and the
first program that needs a temporary file stops with a message about that
folder, before it does its work. These tests need no display and no bwrap.
"""
from __future__ import annotations

from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import sandbox  # noqa: E402

# The environment of a shell of this computer, with a temporary folder that the
# namespace of the fixture keeps read-only.
HOST_ENV = {
    'PATH': '/usr/bin:/bin',
    'HOME': '/home/someone',
    'TMPDIR': '/var/tmp',
}


class PanelTemporaryFolder(unittest.TestCase):
    def box(self, parent):
        state = sandbox.prepare_state(Path(parent) / 'run', reuse=False)
        return state, sandbox.prepare(state, environ=HOST_ENV)

    def test_a_panel_writes_its_temporary_files_inside_the_state(self):
        with tempfile.TemporaryDirectory() as parent:
            state, box = self.box(parent)

            self.assertEqual(box.sandbox_env['TMPDIR'], str(box.private_tmp))
            self.assertEqual(
                Path(box.sandbox_env['TMPDIR']),
                state / 'data' / 'tmp',
                'the folder is the one that the namespace binds over /tmp',
            )

    def test_the_folder_of_a_panel_accepts_a_write(self):
        with tempfile.TemporaryDirectory() as parent:
            _state, box = self.box(parent)

            probe = Path(box.sandbox_env['TMPDIR']) / 'probe'
            probe.write_text('a temporary file of a panel\n')

            self.assertEqual(probe.read_text(), 'a temporary file of a panel\n')

    def test_the_host_processes_keep_the_temporary_folder_of_the_run(self):
        # The X server runs on the host and not in the namespace, so its own
        # `/tmp` stays the folder that the preflight of the fixture wrote to.
        with tempfile.TemporaryDirectory() as parent:
            _state, box = self.box(parent)

            self.assertEqual(box.host_env['TMPDIR'], '/var/tmp')


if __name__ == '__main__':
    unittest.main()
