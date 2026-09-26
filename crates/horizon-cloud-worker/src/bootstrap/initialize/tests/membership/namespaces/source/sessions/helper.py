"""Exercise pre-write resource bounds and safe path handling in the actual helper."""
import io
import os
from pathlib import Path
import sys
import unittest

code, destination, revision, session, project = sys.argv[1:]
sys.argv = ["checkout", destination, "build", revision, session, project]
namespace = {}
exec(code.split('\nif MODE == "build":')[0], namespace)
sys.argv = ["helper-tests"]


class Limits(unittest.TestCase):
    def setUp(self):
        namespace["BUDGET"] = namespace["Budget"]()
        namespace["MAX_TOTAL"] = 8 * 1024**3
        namespace["MAX_WORKING"] = 4 * 1024**3
        namespace["MAX_METADATA"] = 64 * 1024**2

    def test_working_and_aggregate_limits_precede_chunk_write(self):
        for kind in ("working", "data"):
            path = Path("limited-" + kind)
            namespace["MAX_WORKING"] = 2
            namespace["MAX_TOTAL"] = namespace["MAX_METADATA"] + 2
            with self.assertRaises(ValueError):
                namespace["write"](path, io.BytesIO(b"large"), 5, kind)
            self.assertEqual(path.stat().st_size, 0)

    def test_metadata_is_aggregate_across_files(self):
        namespace["MAX_METADATA"] = 1024**2 + 5
        namespace["content"](Path("first-index"), b"four")
        with self.assertRaises(ValueError):
            namespace["content"](Path("second-index"), b"two")
        self.assertEqual(Path("second-index").stat().st_size, 0)

    def test_expansion_preflight_rejects_before_checkout_creation(self):
        namespace["MAX_WORKING"] = 1
        with self.assertRaises(ValueError):
            namespace["plan"]({"modules": [], "assets": []})
        self.assertFalse(Path("checkout").exists())

    def test_ready_inventory_propagates_file_sync_failure(self):
        from unittest.mock import patch
        Path("unsynced").write_bytes(b"prepared bytes")
        with patch("os.fsync", side_effect=OSError("injected sync failure")):
            with self.assertRaises(OSError):
                namespace["inventory"](sync=True)

    def test_symlink_parent_and_git_traversal_are_rejected(self):
        Path("real").mkdir()
        os.symlink("real", "link")
        with self.assertRaises(ValueError):
            namespace["parents"](Path("link/child"))
        self.assertFalse(Path("real/child").exists())
        for value in ("../escape", "/absolute", "a/.git/config", "a/../../escape"):
            with self.assertRaises(ValueError):
                namespace["safe"](value)


unittest.main()
