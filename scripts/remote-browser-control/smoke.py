#!/usr/bin/env python3
"""Prove the remote browser lane end to end, from one run of the public MCP contract.

An agent on this computer creates a page on another machine, reads it, fills it,
clicks it, evaluates in it, attaches a file to it, screenshots it, records it,
closes it, and receives the refusals that this lane defines. Every answer is kept
in a transcript and the run writes one `report.json`.

The lane is the browser host over SSH: `horizon --browser-mcp --remote <target>`.
The other machine is a browser profile host, which the image of
`packaging/browser-vm` runs in a local container (`horizon --browser-vm-local up`).

Usage:
    python3 scripts/remote-browser-control/smoke.py --remote horizon-browser-vm \
        --profile probe --report-dir /tmp/remote-smoke

Exit status: 0 when every step passed, 1 when one failed, 2 when the run could not
start (no binary, no host, no transcript directory).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Sequence

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser-smoke"))
from mcp_gate import HANDSHAKE_PROTOCOL_VERSION, McpClient  # noqa: E402  (the client of the local gate)

# The page that the container serves to this lane. The host runs on that machine,
# so the page has to exist there; the procedure writes it.
DEFAULT_PAGE = "file:///state/test/upload.html"
DEFAULT_ACTOR = "horizon:remote-smoke"
# A `browser_create` of the host answers once the page is requested, not once it
# committed, so the run waits for a selector before it reads the page.
READY_SELECTOR = "h1"
READY_TIMEOUT_MILLIS = 30_000
# A reference that no page has, so the third refusal of the run is a reference one.
MISSING_REF = "ref-that-no-page-has"
# The heading of the page that this run serves, so an evaluate proves the page it
# reads is the page it asked for.
EXPECTED_HEADING = "Remote smoke"


@dataclass
class Step:
    """One step of the run: what it asked, what came back, and whether it held."""

    name: str
    held: bool
    detail: str = ""
    answer: Any = None


@dataclass
class Run:
    """The steps of one run and the values that later steps need."""

    steps: list[Step] = field(default_factory=list)
    panel_id: str = ""

    def record(self, name: str, held: bool, detail: str = "", answer: Any = None) -> bool:
        self.steps.append(Step(name=name, held=held, detail=detail, answer=answer))
        print(f"{'pass' if held else 'FAIL'} {name}{f': {detail}' if detail else ''}", flush=True)
        return held

    @property
    def failed(self) -> list[Step]:
        return [step for step in self.steps if not step.held]


def text_of(answer: dict[str, Any]) -> str:
    """The text blocks of a tool answer, joined."""
    return "".join(block.get("text", "") for block in answer.get("content", []) if isinstance(block, dict))


def refusal_of(error: str | None) -> str:
    """The typed code of a refusal: the first word group of its message.

    The client hands back the whole tool result as JSON, so the message comes out
    of its first text block, and the code is what the tools put in the parentheses
    before the reason, such as `stale_reference` or `unsupported_backend`.
    """
    if not error:
        return ""
    text = error.strip()
    if text.startswith("{"):
        try:
            result = json.loads(text)
        except json.JSONDecodeError:
            return text[:120]
        blocks = result.get("content") or []
        text = "".join(block.get("text", "") for block in blocks if isinstance(block, dict)).strip()
    codes = []
    for part in text.split(","):
        opening = part.find("(")
        if opening < 0:
            continue
        code = part[opening + 1 :].split(")", 1)[0].strip()
        if code and " " not in code:
            codes.append(code)
    # A typed code of Horizon carries an underscore; a word in parentheses such as
    # `(chromium)` names an engine and is not the code of the refusal.
    for code in codes:
        if "_" in code:
            return code
    return "refused"


def handshake(client: McpClient) -> None:
    """The opening exchange of the bridge.

    The gate of the local server also checks its own tool list, which the bridge
    replaces with the surface of the host, so this run checks the handshake only.
    """
    response = client.request(
        "initialize",
        {
            "protocolVersion": HANDSHAKE_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "remote-browser-control-smoke", "version": "1"},
        },
    )
    result = response["result"]
    if result["protocolVersion"] != HANDSHAKE_PROTOCOL_VERSION:
        raise AssertionError(f"the bridge answered protocol {result['protocolVersion']}")
    if result["serverInfo"]["name"] != "horizon-browser":
        raise AssertionError(f"the bridge answered as {result['serverInfo']['name']}")
    client.notify("notifications/initialized")


class Lane:
    """The calls that this run makes, so a fake lane can answer them instead.

    Every call is written to the transcript, which is the evidence of the run
    beside `report.json`.
    """

    def __init__(self, client: McpClient, transcript: Path | None = None) -> None:
        self.client = client
        self.transcript = transcript

    def call(self, name: str, arguments: dict[str, Any], *, expect_error: bool = False):
        structured, error = self.client.call(name, arguments, expect_error=expect_error)
        self.write(name, arguments, structured, error)
        return structured, error

    def write(self, name: str, arguments: dict[str, Any], structured: Any, error: str | None) -> None:
        if self.transcript is None:
            return
        record = {
            "tool": name,
            "arguments": arguments,
            "held": structured is not None,
            "refusal": refusal_of(error) if error else None,
            "structured": structured,
        }
        with self.transcript.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(record, sort_keys=True) + "\n")


def clear_lane(lane: Lane, run: Run) -> None:
    """Close the pages that an earlier run of this actor left open.

    A run that stops in the middle keeps its page, and one agent may hold one page
    per engine, so the next run would be refused before its first step. The lane is
    of the actor of this run, so the page it left is its own to close.
    """
    listed, error = lane.call("browser_list", {})
    panels = (listed or {}).get("panels") or []
    closed = 0
    for panel in panels:
        answer, _ = lane.call("browser_close", {"panel_id": panel.get("panel_id", "")})
        if (answer or {}).get("closed") is True:
            closed += 1
    detail = f"closed {closed} of {len(panels)} earlier pages" if panels else "no earlier page"
    run.record("start from an empty lane", closed == len(panels), detail if listed else str(error))


def create_page(lane: Lane, run: Run, page: str) -> None:
    structured, error = lane.call("browser_create", {"url": page, "visible": False, "timeout_millis": 45_000})
    if structured is None:
        run.record("create", False, f"browser_create was refused: {error}")
        return
    panel = structured.get("panel") or {}
    run.panel_id = panel.get("panel_id", "")
    held = bool(run.panel_id) and panel.get("owned_by_caller") is True and panel.get("remote_target") is None
    run.record(
        "create",
        held,
        f"panel={run.panel_id} navigation={structured.get('navigation')} backend={panel.get('backend')}",
        answer=None,
    )
    # The page is not local: a bridged page loses the handoff capability, and a
    # local Chromium page keeps it.
    capabilities = panel.get("capabilities") or []
    run.record(
        "the page runs on the other machine",
        "handoff" not in capabilities,
        f"capabilities={len(capabilities)} handoff={'handoff' in capabilities}",
    )


def wait_ready(lane: Lane, run: Run) -> None:
    structured, error = lane.call(
        "browser_wait",
        {"panel_id": run.panel_id, "selector": READY_SELECTOR, "state": "visible", "timeout_millis": READY_TIMEOUT_MILLIS},
    )
    run.record("wait for the page", structured is not None, "" if structured else str(error))


def read_page(lane: Lane, run: Run) -> None:
    structured, error = lane.call("browser_query", {"panel_id": run.panel_id, "selector": "#f"})
    nodes = (structured or {}).get("nodes") or []
    run.record("query the page", len(nodes) == 1, f"nodes={len(nodes)}" if structured else str(error))


def fill_and_click(lane: Lane, run: Run) -> None:
    snapshot, error = lane.call("browser_snapshot", {"panel_id": run.panel_id, "max_nodes": 60})
    if snapshot is None:
        run.record("snapshot", False, str(error))
        return
    query, _ = lane.call("browser_query", {"panel_id": run.panel_id, "selector": "#name"})
    nodes = (query or {}).get("nodes") or []
    run.record("snapshot the page", bool(snapshot.get("nodes")), f"nodes={len(snapshot.get('nodes') or [])}")
    if not nodes:
        run.record("fill the page", False, "the page has no #name field")
        return
    reference = nodes[0].get("ref") or ""
    filled, error = lane.call(
        "browser_act",
        {"panel_id": run.panel_id, "action": "fill", "ref": reference, "value": "remote smoke"},
    )
    run.record("fill the page", filled is not None, "" if filled else str(error))
    clicked, error = lane.call(
        "browser_act",
        {"panel_id": run.panel_id, "action": "click", "ref": reference},
    )
    run.record("click the page", clicked is not None, "" if clicked else str(error))


def evaluate_page(lane: Lane, run: Run) -> None:
    structured, error = lane.call(
        "browser_evaluate",
        {"panel_id": run.panel_id, "expression": "document.querySelector('h1').textContent"},
    )
    value = (structured or {}).get("value")
    run.record(
        "evaluate the page",
        value == EXPECTED_HEADING,
        f"heading={value!r}" if structured else str(error),
    )


def attach_file(lane: Lane, run: Run, upload: Path) -> None:
    query, _ = lane.call("browser_query", {"panel_id": run.panel_id, "selector": "#f"})
    nodes = (query or {}).get("nodes") or []
    if not nodes:
        run.record("attach a file", False, "the page has no #f file input")
        return
    reference = nodes[0].get("ref") or ""
    structured, error = lane.call(
        "browser_act",
        {"panel_id": run.panel_id, "action": "set_files", "ref": reference, "files": [str(upload)]},
    )
    run.record("attach a file", structured is not None, "" if structured else str(error))
    # The page names the file that it received, which proves the copy on the other
    # machine reached the engine and not only the bridge.
    named, _ = lane.call(
        "browser_evaluate",
        {"panel_id": run.panel_id, "expression": "document.getElementById('out').textContent"},
    )
    value = (named or {}).get("value")
    run.record("the page received the file", value == upload.name, f"page says {value!r}")


def screenshot_page(lane: Lane, run: Run) -> None:
    structured, error = lane.call("browser_screenshot", {"panel_id": run.panel_id})
    path = (structured or {}).get("path")
    held = isinstance(path, str) and path.endswith(".png")
    run.record("screenshot the page", held, f"a private file on the other machine: {path}" if structured else str(error))


def record_page(lane: Lane, run: Run) -> None:
    started, error = lane.call("browser_video", {"panel_id": run.panel_id, "operation": "start"})
    run.record("start a recording", started is not None, "" if started else str(error))
    if started is None:
        return
    stopped, error = lane.call("browser_video", {"panel_id": run.panel_id, "operation": "stop"})
    path = (stopped or {}).get("path")
    run.record("stop the recording", isinstance(path, str) and path.endswith(".webm"), f"{path}" if stopped else str(error))


def capture_network(lane: Lane, run: Run) -> None:
    started, error = lane.call("browser_network", {"panel_id": run.panel_id, "operation": "start"})
    run.record("start a network capture", started is not None, "" if started else str(error))
    if started is None:
        return
    stopped, error = lane.call("browser_network", {"panel_id": run.panel_id, "operation": "stop"})
    run.record("stop the network capture", stopped is not None, "" if stopped else str(error))


def take_refusals(lane: Lane, run: Run, actor: str) -> None:
    # 1. A person cannot steer a page that lives on the other machine.
    _, error = lane.call("browser_handoff", {"panel_id": run.panel_id, "reason": "remote smoke"}, expect_error=True)
    run.record(
        "refuse a handoff of a page elsewhere",
        "another machine" in (error or ""),
        refusal_of(error),
    )
    # 2. A second page of the same engine needs the explicit opt-in.
    _, error = lane.call(
        "browser_create",
        {"url": DEFAULT_PAGE, "visible": False, "timeout_millis": 30_000},
        expect_error=True,
    )
    run.record(
        "refuse a second page without the opt-in",
        "allow_additional=true" in (error or ""),
        refusal_of(error),
    )
    # 3. A reference that the page does not have is refused instead of guessed.
    _, error = lane.call(
        "browser_act",
        {"panel_id": run.panel_id, "action": "fill", "ref": MISSING_REF, "value": "remote smoke"},
        expect_error=True,
    )
    run.record("refuse a reference that the page does not have", bool(error), refusal_of(error))


def close_page(lane: Lane, run: Run) -> None:
    closed, error = lane.call("browser_close", {"panel_id": run.panel_id})
    run.record("close the page", (closed or {}).get("closed") is True, "" if closed else str(error))
    listed, error = lane.call("browser_list", {})
    panels = (listed or {}).get("panels") or []
    run.record("the closed page leaves the list", not panels, f"panels={len(panels)}" if listed else str(error))


STEPS: list[Callable[..., None]] = [
    create_page,
    wait_ready,
    read_page,
    fill_and_click,
    evaluate_page,
    attach_file,
    screenshot_page,
    record_page,
    capture_network,
    take_refusals,
    close_page,
]


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def git_head(repository: Path) -> str:
    try:
        return subprocess.run(
            ["git", "-C", str(repository), "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def page_source() -> str:
    """The page of this lane: a heading, a text field and one file input."""
    return (
        f"<html><body><h1>{EXPECTED_HEADING}</h1>"
        "<input id=\"name\" type=\"text\">"
        "<input id=\"f\" type=\"file\">"
        "<p id=\"out\">none</p>"
        "<script>"
        "document.getElementById('f').addEventListener('change', (event) => {"
        "  const file = event.target.files[0];"
        "  document.getElementById('out').textContent = file ? file.name : 'none';"
        "});"
        "</script></body></html>"
    )


def write_page(path: Path) -> None:
    path.write_text(page_source(), encoding="utf-8")


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--horizon", default="target/debug/horizon", help="the Horizon binary that serves MCP")
    parser.add_argument("--remote", required=True, help="the SSH target of the browser host")
    parser.add_argument("--profile", default=None, help="the host profile that a create names no target for")
    parser.add_argument("--actor", default=DEFAULT_ACTOR, help="the identity that the bridge forwards")
    parser.add_argument("--page", default=DEFAULT_PAGE, help="the page that the host serves to this run")
    parser.add_argument("--report-dir", required=True, help="where report.json and the transcripts go")
    parser.add_argument("--timeout", type=float, default=120.0, help="the bound of one MCP call, in seconds")
    return parser.parse_args(argv)


def bridge_args(args: argparse.Namespace) -> list[str]:
    """The flags that make the child a bridge to one host, and not a local server."""
    flags = ["--remote", args.remote]
    if args.profile:
        flags += ["--profile", args.profile]
    return flags


def write_report(
    path: Path,
    args: argparse.Namespace,
    binary: Path,
    run: Run,
    started: float,
    transcript: Path,
    attachment_root: Path,
) -> None:
    report = {
        "candidate_commit": git_head(Path(__file__).resolve().parent.parent.parent),
        "candidate_binary_sha256": sha256_of(binary),
        "remote": args.remote,
        "profile": args.profile,
        "actor": args.actor,
        "page": args.page,
        "attachment_root": str(attachment_root),
        "started_unix": int(started),
        "seconds": round(time.time() - started, 1),
        "transcript": transcript.name,
        "held": not run.failed,
        "steps": [
            {"step": step.name, "held": step.held, "detail": step.detail}
            for step in run.steps
        ],
    }
    path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    binary = Path(args.horizon).resolve()
    if not binary.is_file():
        print(f"no Horizon binary at {binary}", file=sys.stderr)
        return 2
    report_dir = Path(args.report_dir).resolve()
    report_dir.mkdir(parents=True, exist_ok=True)
    upload = report_dir / "upload.txt"
    upload.write_text("remote smoke attachment\n", encoding="utf-8")
    # The file of the run lies in its report folder, which is outside the work root
    # of this computer, so the run names that folder as an attachment root of the
    # bridge. A person does the same with this variable for a folder of their own.
    previous_roots = os.environ.get("HORIZON_BROWSER_ATTACHMENT_ROOTS")
    os.environ["HORIZON_BROWSER_ATTACHMENT_ROOTS"] = os.pathsep.join(
        [str(report_dir)] + ([previous_roots] if previous_roots else [])
    )
    transcript = report_dir / f"rpc-{args.remote}.jsonl"
    transcript.write_text("", encoding="utf-8")
    log = report_dir / f"stderr-{args.remote}.log"
    started = time.time()
    run = Run()
    client = McpClient(
        binary,
        log,
        args.timeout,
        args.actor,
        extra_args=bridge_args(args),
    )
    try:
        handshake(client)
        lane = Lane(client, transcript)
        clear_lane(lane, run)
        create_page(lane, run, args.page)
        if run.panel_id:
            wait_ready(lane, run)
            remaining = [step for step in STEPS if step not in (create_page, wait_ready, close_page)]
            for step in remaining:
                if step is attach_file:
                    step(lane, run, upload)
                elif step is take_refusals:
                    step(lane, run, args.actor)
                else:
                    step(lane, run)
            close_page(lane, run)
    except (AssertionError, RuntimeError, TimeoutError, OSError) as error:
        run.record("run", False, f"{type(error).__name__}: {error}")
    finally:
        if previous_roots is None:
            os.environ.pop("HORIZON_BROWSER_ATTACHMENT_ROOTS", None)
        else:
            os.environ["HORIZON_BROWSER_ATTACHMENT_ROOTS"] = previous_roots
        try:
            client.close()
        except (AssertionError, OSError):
            pass
    write_report(report_dir / "report.json", args, binary, run, started, transcript, report_dir)
    failed = run.failed
    print(f"{len(run.steps) - len(failed)} of {len(run.steps)} steps held; report in {report_dir / 'report.json'}")
    for step in failed:
        print(f"failed: {step.name}: {step.detail}", file=sys.stderr)
    return 0 if not failed else 1


if __name__ == "__main__":
    sys.exit(main())
