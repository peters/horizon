"""Bytecode contract for the read-only Linux host preflight."""
from harness import *  # noqa: F401,F403


class BytecodeContract(unittest.TestCase):
    def test_import_disables_bytecode(self):
        self.assertTrue(sys.dont_write_bytecode)
