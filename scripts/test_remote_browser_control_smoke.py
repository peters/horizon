#!/usr/bin/env python3
"""Test the remote-browser smoke against a stand-in lane, without a host or an SSH target.

The real run needs a container that serves a page. This test drives the same step
functions with a fake lane that answers like the host, so the step list, the three
refusals and the report shape are checked in continuous integration.
"""

from __future__ import annotations

import importlib.util
import io
import json
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SMOKE = ROOT / "scripts" / "remote-browser-control" / "smoke.py"


def load_smoke():
    spec = importlib.util.spec_from_file_location("remote_browser_smoke", SMOKE)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


smoke = load_smoke()


class FakeLane:
    """The answers of one host, keyed by tool name."""

    def __init__(self, capabilities=None, close_works=True):
        self.calls: list[tuple[str, dict]] = []
        self.capabilities = capabilities if capabilities is not None else ["navigate", "query", "fill", "click"]
        self.close_works = close_works
        self.attached = None
        self.panel_open = False

    def call(self, name, arguments, *, expect_error=False):
        self.calls.append((name, arguments))
        answer = self.answer(name, arguments)
        if isinstance(answer, tuple):
            return answer
        if expect_error:
            raise AssertionError(f"{name} was expected to fail: {answer}")
        return answer, None

    def answer(self, name, arguments):
        if name == "browser_create":
            if self.panel_open:
                return (
                    None,
                    "browser panel profile-probe-1 (chromium) is already owned by this agent; reuse it, "
                    "or name another engine in backend (one panel per engine: chromium, firefox, safari), "
                    "or set allow_additional=true only when the user explicitly requested an independent "
                    "browser session",
                )
            self.panel_open = True
            if arguments.get("url") == smoke.DEFAULT_PAGE:
                return (
                    {
                        "action_id": "create-1",
                        "navigation": "pending",
                        "panel": {
                            "panel_id": "profile-probe-1",
                            "backend": "chromium",
                            "owned_by_caller": True,
                            "capabilities": self.capabilities,
                        },
                    },
                    None,
                )
            return None, "browser panel creation and duplication are available only to an agent panel launched inside Horizon"
        if name == "browser_wait":
            return ({"nodes": [{"ref": "e1"}]}, None)
        if name == "browser_query":
            if arguments.get("selector") == "#f":
                return ({"nodes": [{"ref": "e2", "file_input": {"accept": ""}}]}, None)
            return ({"nodes": [{"ref": "e1"}]}, None)
        if name == "browser_snapshot":
            return ({"nodes": [{"ref": "e1"}]}, None)
        if name == "browser_act":
            if arguments.get("ref") == smoke.MISSING_REF:
                return (None, "browser action act-3 failed (ref_not_found): the page has no reference ref-that-no-page-has")
            if arguments.get("action") == "set_files":
                self.attached = arguments["files"][0]
                return ({"action_id": "act-2"}, None)
            return ({"action_id": "act-1"}, None)
        if name == "browser_evaluate":
            if "out" in arguments.get("expression", ""):
                return ({"value": Path(self.attached or "none").name}, None)
            return ({"value": smoke.EXPECTED_HEADING}, None)
        if name == "browser_screenshot":
            return ({"path": "/state/browser-root/profile-host-captures/x/captures/shot.png"}, None)
        if name == "browser_video":
            if arguments.get("operation") == "start":
                return ({"recording": {"active": True}}, None)
            return ({"path": "/state/browser-root/captures/page.webm"}, None)
        if name == "browser_network":
            if arguments.get("operation") == "start":
                return ({"capture_id": "capture-1"}, None)
            return ({"path": "/state/browser-root/captures/page.ndjson"}, None)
        if name == "browser_handoff":
            return (
                None,
                "browser handoff failed (unsupported_backend): this page runs on another machine, "
                "so no person here can steer it; open the page on that machine, or use browser_act "
                "for supported actions and verify their outcomes",
            )
        if name == "browser_close":
            if self.close_works:
                self.panel_open = False
            return ({"closed": self.close_works, "panel_id": arguments.get("panel_id")}, None)
        if name == "browser_list":
            panels = [{"panel_id": "profile-probe-1"}] if self.panel_open else []
            return ({"panels": panels}, None)
        raise AssertionError(f"unexpected tool {name}")


def run_steps(lane, page="file:///state/test/upload.html", upload=None):
    module = smoke
    run = module.Run()
    with redirect_stdout(io.StringIO()):
        module.create_page(lane, run, page)
        if run.panel_id:
            module.wait_ready(lane, run)
            module.read_page(lane, run)
            module.fill_and_click(lane, run)
            module.evaluate_page(lane, run)
            module.attach_file(lane, run, upload)
            module.screenshot_page(lane, run)
            module.record_page(lane, run)
            module.capture_network(lane, run)
            module.take_refusals(lane, run, "horizon:remote-smoke")
            module.close_page(lane, run)
    return run


class RemoteSmokeTest(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.upload = Path(self.folder.name) / "upload.txt"
        self.upload.write_text("remote smoke attachment\n", encoding="utf-8")

    def test_every_step_holds_on_a_lane_that_answers(self):
        run = run_steps(FakeLane(), upload=self.upload)
        self.assertEqual(run.failed, [], [f"{step.name}: {step.detail}" for step in run.failed])
        self.assertEqual(run.panel_id, "profile-probe-1")
        names = [step.name for step in run.steps]
        for expected in ["create", "query the page", "fill the page", "click the page", "attach a file"]:
            self.assertIn(expected, names)

    def test_a_page_without_handoff_is_the_one_proof_that_it_runs_elsewhere(self):
        local = run_steps(FakeLane(capabilities=["navigate", "handoff"]), upload=self.upload)
        held = {step.name: step.held for step in local.steps}
        self.assertFalse(held["the page runs on the other machine"])
        remote = run_steps(FakeLane(), upload=self.upload)
        self.assertTrue({step.name: step.held for step in remote.steps}["the page runs on the other machine"])

    def test_a_leftover_page_of_an_earlier_run_is_closed_first(self):
        lane = FakeLane()
        lane.panel_open = True
        run = smoke.Run()
        with redirect_stdout(io.StringIO()):
            smoke.clear_lane(lane, run)
        self.assertTrue(run.steps[0].held, run.steps[0].detail)
        self.assertFalse(lane.panel_open, "the page of the earlier run is gone")

    def test_a_create_that_is_refused_ends_the_run_before_the_other_steps(self):
        lane = FakeLane()
        run = smoke.Run()
        with redirect_stdout(io.StringIO()):
            smoke.create_page(lane, run, "https://example.org/")
        self.assertEqual(run.panel_id, "")
        self.assertEqual([step.held for step in run.steps], [False])
        self.assertEqual([name for name, _ in lane.calls], ["browser_create"])

    def test_the_three_refusals_are_asked_and_taken(self):
        lane = FakeLane()
        lane.panel_open = True
        run = smoke.Run()
        run.panel_id = "profile-probe-1"
        with redirect_stdout(io.StringIO()):
            smoke.take_refusals(lane, run, "horizon:remote-smoke")
        self.assertEqual(len(run.steps), 3)
        self.assertTrue(all(step.held for step in run.steps), [step.detail for step in run.steps])
        asked = [name for name, _ in lane.calls]
        self.assertEqual(asked, ["browser_handoff", "browser_create", "browser_act"],
                         "a handoff, a second page and a reference that does not exist")

    def test_a_closed_page_that_stays_in_the_list_fails_the_last_step(self):
        # `close_works=False` is a close that is refused, which leaves the page.
        run = run_steps(FakeLane(close_works=False), upload=self.upload)
        names = {step.name: step.held for step in run.steps}
        self.assertFalse(names["the closed page leaves the list"])

    def test_the_page_of_this_lane_holds_one_field_one_input_and_one_readout(self):
        page = smoke.page_source()
        for marker in ['id="name"', 'id="f" type="file"', 'id="out"']:
            self.assertIn(marker, page)

    def test_a_report_names_the_candidate_the_lane_and_every_step(self):
        lane = FakeLane()
        run = run_steps(lane, upload=self.upload)
        args = smoke.parse_args(["--remote", "horizon-browser-vm", "--profile", "probe",
                                 "--report-dir", self.folder.name])
        report_path = Path(self.folder.name) / "report.json"
        smoke.write_report(report_path, args, Path(sys.executable), run, 0, Path("rpc-horizon-browser-vm.jsonl"))
        report = json.loads(report_path.read_text(encoding="utf-8"))
        self.assertEqual(report["remote"], "horizon-browser-vm")
        self.assertEqual(report["profile"], "probe")
        self.assertEqual(len(report["candidate_binary_sha256"]), 64)
        self.assertEqual(len(report["steps"]), len(run.steps))
        self.assertTrue(report["held"])

    def test_a_refusal_is_reported_by_its_typed_code(self):
        answer = json.dumps(
            {
                "content": [
                    {
                        "type": "text",
                        "text": "browser action action-1 failed (stale_reference): element reference is stale; take a new snapshot",
                    }
                ],
                "isError": True,
            }
        )
        self.assertEqual(smoke.refusal_of(answer), "stale_reference")
        self.assertEqual(
            smoke.refusal_of(
                "browser handoff failed (unsupported_backend): this page runs on another machine"
            ),
            "unsupported_backend",
        )
        self.assertEqual(smoke.refusal_of(""), "")
        self.assertEqual(
            smoke.refusal_of("browser panel p (chromium) is already owned by this agent"),
            "refused",
            "a word in parentheses is not a typed code",
        )
        self.assertEqual(smoke.refusal_of("not json at all"), "refused")

    def test_the_bridge_flags_name_the_host_and_the_profile(self):
        args = smoke.parse_args(["--remote", "horizon-browser-vm", "--profile", "probe",
                                 "--report-dir", self.folder.name])
        self.assertEqual(smoke.bridge_args(args), ["--remote", "horizon-browser-vm", "--profile", "probe"])
        without = smoke.parse_args(["--remote", "host", "--report-dir", self.folder.name])
        self.assertEqual(smoke.bridge_args(without), ["--remote", "host"])


if __name__ == "__main__":
    unittest.main()
