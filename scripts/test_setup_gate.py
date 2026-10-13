#!/usr/bin/env python3
"""Tests for the parts of scripts/setup-gate.py that need no Docker."""

import importlib.util
import json
import re
import shutil
import tempfile
import unittest
from unittest import mock
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location("setup_gate", ROOT / "scripts" / "setup-gate.py")
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)

# A name Docker accepts for a container, a volume and an image repository.
DOCKER_NAME = re.compile(r"[a-z0-9][a-z0-9_.-]*")


def entry(label, heavy=False):
    return {"label": label, "repo": label, "heavy": heavy, "leave_out": []}


class ListTests(unittest.TestCase):
    def setUp(self):
        self.folder = Path(tempfile.mkdtemp(prefix="setup-gate-list-"))
        self.addCleanup(shutil.rmtree, self.folder, ignore_errors=True)
        for name in ("example-api", "example-web"):
            (self.folder / name).mkdir()

    def load(self, items, only=()):
        path = self.folder / "list.json"
        path.write_text(json.dumps(items))
        return gate.load_list(path, only)

    def test_paths_resolve_against_the_list_and_jobs_are_left_out(self):
        [loaded] = self.load([{
            "repo": "example-api",
            "path": "example-api",
            "spec": {"workflow": "ci.yml", "jobs": ["test", "audit"], "event": "push"},
            "leave_out": ["audit"],
        }])
        self.assertEqual(loaded["path"], str((self.folder / "example-api").resolve()))
        self.assertEqual(loaded["spec"]["jobs"], ["test"])
        self.assertEqual(loaded["leave_out"], ["audit"])
        self.assertEqual(loaded["label"], "example-api")
        self.assertFalse(loaded["heavy"])

    def test_mistakes_in_the_list_are_refused(self):
        api = {"repo": "example-api", "path": "example-api", "spec": {"commands": ["make test"]}}
        cases = {
            "unknown keys: heavey": [dict(api, heavey=True)],
            "two entries are called example-api": [api, dict(api)],
            "has no job audit": [dict(api, spec={"workflow": "ci.yml", "jobs": ["test"]}, leave_out=["audit"])],
            "leaves out every job": [dict(api, spec={"workflow": "ci.yml", "jobs": ["test"]}, leave_out=["test"])],
            "leave_out needs a spec with jobs": [dict(api, leave_out=["test"])],
            "is not a folder": [dict(api, path="example-missing")],
            "cannot name a log file": [dict(api, label="../example")],
            "heavy must be true or false": [dict(api, heavy="yes")],
        }
        for message, items in cases.items():
            with self.subTest(message), self.assertRaisesRegex(gate.ListError, message):
                self.load(items)

    def test_only_selects_by_label_or_repo(self):
        items = [
            {"repo": "example-api", "path": "example-api", "spec": {"commands": ["make test"]}},
            {"repo": "example-web", "label": "web-ui", "path": "example-web", "spec": {"commands": ["make test"]}},
        ]
        self.assertEqual([e["label"] for e in self.load(items, ["web-*"])], ["web-ui"])
        self.assertEqual([e["label"] for e in self.load(items, ["example-w*"])], ["web-ui"])
        with self.assertRaisesRegex(gate.ListError, "matches --only"):
            self.load(items, ["nothing"])


class LaneTests(unittest.TestCase):
    def test_heavy_entries_share_the_first_lane_in_list_order(self):
        entries = [entry("a"), entry("big-1", True), entry("b"), entry("big-2", True), entry("c"), entry("d")]
        lanes = gate.split_lanes(entries, 2)
        self.assertEqual([e["label"] for e in lanes[0] if e["heavy"]], ["big-1", "big-2"])
        self.assertTrue(all(not e["heavy"] for lane in lanes[1:] for e in lane))
        # Two heavy entries count as six, so the other lane takes the light ones.
        self.assertEqual([e["label"] for e in lanes[1]], ["a", "b", "c", "d"])

    def test_light_entries_balance_and_lanes_never_outnumber_entries(self):
        entries = [entry(name) for name in "abcde"]
        self.assertEqual([[e["label"] for e in lane] for lane in gate.split_lanes(entries, 2)],
                         [["a", "c", "e"], ["b", "d"]])
        self.assertEqual(len(gate.split_lanes(entries[:2], 8)), 2)
        self.assertEqual(len(gate.split_lanes([entry("big-1", True), entry("big-2", True)], 4)), 1)

    def test_run_names_are_docker_names_and_distinct_per_lane(self):
        first, second = gate.lane_prefix("3fa9c1", 1), gate.lane_prefix("3fa9c1", 2)
        runs = [gate.run_name(prefix, index) for prefix in (first, second) for index in range(12)]
        self.assertEqual(len(set(runs)), len(runs))
        self.assertTrue(all(DOCKER_NAME.fullmatch(run) for run in runs), runs)
        self.assertEqual(gate.run_name(first, 7), "gate-3fa9c1-1-0007")
        # One run's name is never the start of another's.
        self.assertFalse(gate.run_name(first, 10).startswith(gate.run_name(first, 1) + "-"))


class CleanupTests(unittest.TestCase):
    def record(self, run):
        volume = f"horizon-setup-example-api-1a2b3c-{run}"
        return {
            "run": run,
            "image": f"{volume}:candidate",
            "volume": volume,
            "checks": gate.check_prefix(volume),
            "network": f"{volume}-preview",
        }

    def test_only_the_names_of_the_run_are_left_overs(self):
        mine, other = self.record("gate-3fa9c1-1-0001"), self.record("gate-3fa9c1-1-0011")
        listings = {
            "container": [mine["volume"] + "-proof-1", mine["volume"] + "-proof-1-env", other["volume"] + "-proof-1",
                          mine["checks"] + "0", "horizon-contract-horizon-setup-check-12345678-0", "postgres"],
            "image": [mine["image"], mine["volume"] + "-build1:candidate", other["image"], "ubuntu:24.04"],
            "volume": [mine["volume"], other["volume"], mine["volume"] + "0"],
            "network": [mine["network"], other["network"], "bridge"],
        }
        self.assertEqual(gate.leftover_names(mine, listings), {
            "container": [mine["volume"] + "-proof-1", mine["volume"] + "-proof-1-env", mine["checks"] + "0"],
            "image": [mine["image"], mine["volume"] + "-build1:candidate"],
            "volume": [mine["volume"]],
            "network": [mine["network"]],
        })

    def test_a_record_must_name_a_run_of_this_gate_run(self):
        prefixes = [gate.lane_prefix("3fa9c1", 1)]
        record = self.record("gate-3fa9c1-1-0001")
        self.assertTrue(gate.valid_record(record, prefixes))
        self.assertFalse(gate.valid_record(self.record("gate-ffffff-1-0001"), prefixes))
        self.assertFalse(gate.valid_record(dict(record, volume="postgres-data"), prefixes))
        self.assertFalse(gate.valid_record(dict(record, image="ubuntu:24.04"), prefixes))
        self.assertFalse(gate.valid_record(dict(record, checks="horizon-"), prefixes))
        other = self.record("gate-3fa9c1-1-0002")["checks"]
        self.assertFalse(gate.valid_record(dict(record, checks=other), prefixes))

    def test_the_check_prefix_is_the_one_horizon_makes(self):
        # gate.rs checks the same pair against check_operations in proof/resources.rs.
        self.assertEqual(gate.check_prefix("horizon-setup-example-cli-c357f1-gate-312b8e-1-0000"),
                         "horizon-contract-horizon-setup-check-8c80ce35-")

    def test_only_a_socket_of_this_computer_is_pinned(self):
        with mock.patch.dict(gate.os.environ, {"DOCKER_HOST": "unix:///run/user/1000/docker.sock",
                                                "DOCKER_CONTEXT": "remote"}):
            self.assertIsNone(gate.pin_docker())
            self.assertEqual(gate.os.environ["DOCKER_HOST"], "unix:///run/user/1000/docker.sock")
            self.assertNotIn("DOCKER_CONTEXT", gate.os.environ)
        for refused in ("tcp://10.0.0.5:2376", "ssh://builder@build.example"):
            with mock.patch.dict(gate.os.environ, {"DOCKER_HOST": refused}):
                self.assertIn("not a socket of this computer", gate.pin_docker())

    def test_docker_output_is_bounded_in_size_and_time(self):
        folder = Path(tempfile.mkdtemp(prefix="setup-gate-docker-"))
        self.addCleanup(shutil.rmtree, folder, ignore_errors=True)
        fake = folder / "docker"
        fake.write_text("#!/bin/sh\ncase $1 in\n"
                        "  ok) printf 'one\\ntwo\\n' ;;\n"
                        "  fail) echo partial; exit 1 ;;\n"
                        "  loud) yes long-line | head -c 4096 ;;\n"
                        "  slow) sleep 5 ;;\n"
                        "esac\n")
        fake.chmod(0o755)
        with mock.patch.dict(gate.os.environ, {"PATH": f"{folder}:{gate.os.environ['PATH']}"}), \
                mock.patch.object(gate, "OUTPUT_LIMIT", 1024):
            self.assertEqual(gate.docker(["ok"]), "one\ntwo\n")
            self.assertIsNone(gate.docker(["fail"]))
            self.assertIsNone(gate.docker(["loud"]), "a cut listing never passes for a whole one")
            begun = gate.time.monotonic()
            self.assertIsNone(gate.docker(["slow"], timeout=0.5))
            self.assertLess(gate.time.monotonic() - begun, 4)

    def test_stat_lines_with_odd_command_names_parse(self):
        self.assertEqual(gate.parse_stat("4242 (a (b) c) S 1 4242 4100 0 -1 4194560 120"), ("S", 4100))
        self.assertEqual(gate.parse_stat("17 (docker) Z 4242 17 4242 0"), ("Z", 4242))


class SummaryTests(unittest.TestCase):
    def phases(self, *seconds, failure=None):
        names = ("Build image", "Run CI in image", "Fast rerun")
        result = [{"phase": name, "seconds": value, "failure": None} for name, value in zip(names, seconds)]
        if failure:
            result[len(seconds) - 1]["failure"] = failure
        return result + [{"phase": name, "seconds": None, "failure": None} for name in names[len(seconds):]]

    def test_rows(self):
        passed = {"passed": True, "phases": self.phases(42, 31, 5),
                  "preview": {"outcome": "live", "seconds": 12, "window": "1280x800"}}
        failed = {"passed": False, "phases": self.phases(40, 90, failure="the CI jobs failed: exit status 1\nmore detail"),
                  "preview": None}
        preview = {"passed": False, "phases": self.phases(40, 30, 4),
                   "preview": {"outcome": "failed", "detail": "no window within 10 minutes"}}
        error = {"passed": False, "error": "the spec does not parse"}
        self.assertEqual(gate.summary_row(entry("example-api"), passed),
                         ["example-api", "PASS", "42 s", "31 s", "5 s", "live 12 s", ""])
        self.assertEqual(gate.summary_row(entry("example-cli"), failed),
                         ["example-cli", "FAIL", "40 s", "90 s", "-", "-", "Run CI in image: the CI jobs failed: exit status 1"])
        self.assertEqual(gate.summary_row(entry("example-ui"), preview)[1:],
                         ["FAIL", "40 s", "30 s", "4 s", "failed", "preview: no window within 10 minutes"])
        self.assertEqual(gate.summary_row(entry("example-x"), error)[-1], "the spec does not parse")
        self.assertEqual(gate.summary_row(entry("example-y"), None, "no result: stopped by SIGINT"),
                         ["example-y", "FAIL", "-", "-", "-", "-", "no result: stopped by SIGINT"])

    def test_table_aligns_every_column_but_the_last(self):
        table = gate.format_table([["example-api", "PASS", "42 s", "31 s", "5 s", "-", ""],
                                   ["b", "FAIL", "4 s", "-", "-", "-", "the build failed"]]).splitlines()
        self.assertEqual(len(table), 3)
        self.assertEqual(table[0].index("Result"), table[1].index("PASS"))
        self.assertEqual(table[2].index("the build failed"), table[0].index("First failure"))

    def test_the_test_binary_is_the_bin_target_built_for_tests(self):
        lines = [
            "not json",
            json.dumps({"reason": "compiler-artifact", "target": {"name": "horizon", "kind": ["bin"]},
                        "profile": {"test": False}, "executable": "/t/debug/horizon"}),
            json.dumps({"reason": "compiler-artifact", "target": {"name": "font_assets", "kind": ["test"]},
                        "profile": {"test": True}, "executable": "/t/debug/deps/font_assets-1"}),
            json.dumps({"reason": "compiler-artifact", "target": {"name": "horizon", "kind": ["bin"]},
                        "profile": {"test": True}, "executable": "/t/debug/deps/horizon-2"}),
        ]
        self.assertEqual(gate.find_test_binary(lines), "/t/debug/deps/horizon-2")
        self.assertIsNone(gate.find_test_binary(lines[:3]))


if __name__ == "__main__":
    unittest.main()
