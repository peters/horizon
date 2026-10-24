"""The fixture reports an unwritable host /tmp before it starts a display (#1793)."""
from __future__ import annotations

import os
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import serve  # noqa: E402


class WritableTmp(unittest.TestCase):
    def test_a_writable_directory_leaves_no_probe_behind(self):
        with tempfile.TemporaryDirectory(prefix='horizon-smoke-tmp-') as directory:
            serve.require_writable_tmp(directory)

            self.assertEqual(sorted(os.listdir(directory)), [])

    @unittest.skipIf(os.geteuid() == 0, 'root writes into a directory with mode 0500')
    def test_a_directory_that_refuses_a_write_names_the_cause(self):
        with tempfile.TemporaryDirectory(prefix='horizon-smoke-tmp-') as parent:
            read_only = Path(parent) / 'read-only'
            read_only.mkdir(mode=0o500)

            with self.assertRaises(RuntimeError) as raised:
                serve.require_writable_tmp(read_only)

            message = str(raised.exception)
            self.assertIn(str(read_only), message)
            self.assertIn('refused a write', message)
            self.assertIn('keymap files', message)
            self.assertIn('quota', message)

    def test_a_leftover_file_in_the_directory_does_not_stop_the_check(self):
        # /tmp is world-writable: another user or an earlier run of this harness can
        # leave a file there. The check must not open it or fail on it.
        with tempfile.TemporaryDirectory(prefix='horizon-smoke-tmp-') as directory:
            leftover = Path(directory) / 'horizon-smoke-write-1.tmp'
            leftover.write_text('leftover')

            serve.require_writable_tmp(directory)

            self.assertEqual(leftover.read_text(), 'leftover')

    def test_the_run_checks_the_host_tmp_its_x_server_uses(self):
        self.assertEqual(serve.TMP_DIR, Path('/tmp'))
        serve.require_writable_tmp(serve.TMP_DIR)


if __name__ == '__main__':
    unittest.main()
