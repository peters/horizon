"""Failure-injection checks for the benchmark's independent decoder gates."""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import bench


class BackendGates(unittest.TestCase):
    def test_nvenc_with_cpu_scaling_still_qualifies_encoder_only_gpu(self):
        bench.validate_backend({"encoder": "h264_nvenc", "scaler": "cpu"}, "gpu", None)

    def test_software_encoding_never_qualifies_gpu(self):
        with self.assertRaisesRegex(RuntimeError, "software fallback"):
            bench.validate_backend({"encoder": "libx264", "scaler": "cpu"}, "gpu", None)

    def test_missing_or_changed_scaler_never_qualifies_requested_cuda(self):
        for scaler in [None, "cpu", "unknown"]:
            with self.subTest(scaler=scaler), self.assertRaisesRegex(RuntimeError, "scaler"):
                bench.validate_backend({"encoder": "h264_nvenc", "scaler": scaler}, "gpu", "cuda")


class DecoderGates(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.result = {"width": 1280, "height": 720, "total_frames": 2, "submitted": 2}
        self.state = {"teardown": True, "events": True, "frames": 2,
                      "configs": 1, "config_dimensions": [[1280, 720]] * 3}
        self.receiver = SimpleNamespace(errors=[], sessions=[self.state])
        self.info = {"streams": [{"width": 1280, "height": 720, "nb_read_frames": "2"}]}
        pixels = bytearray(bench.FRAME_BYTES)
        pixels[0:6] = bytes([255] * 6)  # Counter = 1, twice per bit.
        for row in range(32):
            y = (2 * row + 1) * 720 // 64
            for column in range(64):
                x = (2 * column + 1) * 1280 // 128
                value = 30 if (x // 16 + y // 16) % 2 == 0 else 180
                at = ((row + 1) * 64 + column) * 3
                pixels[at:at + 3] = bytes([value] * 3)
        second = bytearray(pixels)
        second[0:6] = bytes(6)
        second[6:12] = bytes([255] * 6)
        self.pixels = bytes(pixels + second)

    def decode(self):
        with patch.object(bench, "execute", side_effect=[SimpleNamespace(stdout=json.dumps(self.info)),
                                                       SimpleNamespace(stdout=self.pixels)]):
            return bench.decode(self.root, self.result, self.receiver)

    def test_valid_decoded_fixture_qualifies(self):
        self.assertEqual(self.decode()["decoded_frames"], 2)

    def test_single_frame_does_not_prove_advancement(self):
        self.state["frames"] = self.result["total_frames"] = self.result["submitted"] = 1
        self.info["streams"][0]["nb_read_frames"] = "1"
        self.pixels = self.pixels[:bench.FRAME_BYTES]
        with self.assertRaisesRegex(RuntimeError, "advancement"):
            self.decode()

    def test_receiver_authentication_failure_never_scores(self):
        self.receiver.errors.append("authentication failed")
        with self.assertRaisesRegex(RuntimeError, "receiver failed"):
            self.decode()

    def test_missing_teardown_never_scores(self):
        self.state["teardown"] = False
        with self.assertRaisesRegex(RuntimeError, "teardown"):
            self.decode()

    def test_wrong_output_resolution_never_scores(self):
        self.info["streams"][0]["width"] = 720
        with self.assertRaisesRegex(RuntimeError, "canvas"):
            self.decode()

    def test_missing_video_configuration_never_scores(self):
        self.state["configs"] = 0
        self.state["config_dimensions"] = []
        with self.assertRaisesRegex(RuntimeError, "configuration"):
            self.decode()

    def test_earlier_wrong_configuration_is_not_hidden_by_a_later_correct_one(self):
        self.state["configs"] = 2
        self.state["config_dimensions"] = [[1, 1]] * 3 + [[1280, 720]] * 3
        with self.assertRaisesRegex(RuntimeError, "configuration differs"):
            self.decode()

    def test_missing_decoder_frames_never_score(self):
        self.info["streams"][0]["nb_read_frames"] = "0"
        with self.assertRaisesRegex(RuntimeError, "coverage"):
            self.decode()

    def test_truncated_quality_pixels_never_score(self):
        self.pixels = self.pixels[:-1]
        with self.assertRaisesRegex(RuntimeError, "coverage"):
            self.decode()

    def test_frozen_frame_ids_never_score(self):
        self.state["frames"] = self.result["total_frames"] = 2
        self.info["streams"][0]["nb_read_frames"] = "2"
        self.pixels = self.pixels[:bench.FRAME_BYTES] * 2
        with self.assertRaisesRegex(RuntimeError, "duplicated"):
            self.decode()

    def test_changed_image_content_never_scores(self):
        self.pixels = b"".join(
            self.pixels[offset:offset + 192] + bytes(bench.FRAME_BYTES - 192)
            for offset in range(0, len(self.pixels), bench.FRAME_BYTES)
        )
        with self.assertRaisesRegex(RuntimeError, "^decoded image quality regressed$"):
            self.decode()

    def test_resource_sampler_errors_are_retained(self):
        readings = {}
        with patch.object(bench, "sample_resources", side_effect=OSError("sampler failed")):
            bench.sample(None, readings, False)
        self.assertEqual(readings["sampler_error"], "sampler failed")


@unittest.skipUnless(sys.platform == "linux", "process-tree benchmark is Linux-only")
class CleanupGates(unittest.TestCase):
    def test_receiver_close_interrupts_idle_accepted_control_socket(self):
        with tempfile.TemporaryDirectory() as directory:
            receiver = bench.Receiver(directory)
            connection = socket.create_connection(("127.0.0.1", receiver.port))
            deadline = time.monotonic() + 2
            while not receiver.sessions and time.monotonic() < deadline:
                time.sleep(0.01)
            self.assertTrue(receiver.sessions)
            started = time.monotonic()
            receiver.close()
            connection.close()
            self.assertLess(time.monotonic() - started, 5.1)
            self.assertFalse(any(worker.is_alive() for worker in receiver.threads))

    def test_exited_sender_with_live_child_is_detected_and_cleaned(self):
        process = subprocess.Popen([sys.executable, "-c",
                                    "import subprocess; print(subprocess.Popen(['sleep','30']).pid, flush=True)"],
                                   start_new_session=True, stdout=subprocess.PIPE, text=True)
        try:
            child = int(process.stdout.readline())
            process.wait(timeout=3)
            self.assertTrue(bench.stop_group(process))
            self.assertEqual(bench.group_members(process.pid), [])
            status = Path(f"/proc/{child}/stat")
            if status.exists():
                self.assertEqual(status.read_text().rsplit(")", 1)[1].split()[0], "Z")
        finally:
            bench.stop_group(process)
            process.stdout.close()


if __name__ == "__main__":
    unittest.main()
