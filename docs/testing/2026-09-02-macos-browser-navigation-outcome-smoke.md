# macOS browser navigation outcome smoke test

Temporary validation artifact for the browser navigation-outcome pull request
(issue #350) and the pull requests stacked on it (#348 create readiness, #349
event-driven wait). Run every lane on a logged-in Mac desktop against the
exact PR head named in the request. Do not merge until the report ends with
`SMOKE-TEST: DONE` and every required lane passes. This dated plan stays in
the repository as the execution record; append findings to the report, do not
rewrite the procedure.

Status: the stack has merged to `main` as #365 (`2ae3383`, issue #350), #366
(`9e2f719`, issue #348), and #367 (`82d070c`, issue #349), each after a
`SMOKE-TEST: DONE` report. Section 6 records the post-merge verification of
the merged tree.

## Safety contract

- Do not stop, signal, reuse, or automate a pre-existing Horizon process, and
  never touch `~/github/horizon`; use a task-owned worktree only.
- Launch only the task-owned Horizon binary with an isolated `HOME`, config,
  and `--ephemeral`. Record its PID and target only that PID and window.
- Drive browser panels only through the fourteen public `browser_*` MCP tools
  as `scripts/browser-smoke/run.py` does; never read Horizon's private runtime
  files, raw CDP/BiDi/WebDriver endpoints, or another session's profile.
- Never log page contents, credentials, cookies, transcripts, or unrelated
  machine identifiers. Fixture pages are the only oracles.
- Close the task-owned Horizon window normally and verify only its PID and its
  browser process tree exit.

## 1. Exact-head and environment preflight

```bash
git fetch origin
git worktree add /tmp/horizon-browser-nav-smoke <exact-pr-sha>
cd /tmp/horizon-browser-nav-smoke
PATH=/opt/homebrew/bin:$PATH git lfs pull
git rev-parse HEAD
git status --short
sw_vers
uname -m
rustc --version
safaridriver --version
geckodriver --version
"/Applications/Firefox.app/Contents/MacOS/firefox" --version
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --version
```

Confirm:

- `HEAD` equals the requested PR SHA and the worktree is clean (`Cargo.lock`
  may show the pre-existing surge tag drift; do not commit it).
- A logged-in console user owns an interactive desktop session (the gate opens
  a visible Horizon window and, for Safari, a visible Safari window).
- `safaridriver --enable` has been run once for this user; Safari's Develop
  menu allows remote automation.
- Existing Horizon PIDs are recorded and excluded from every later command.
- At least 15 GiB is free for the build.

## 2. Native build and automated Mac tests

```bash
cargo fmt --all -- --check
./scripts/check-maintainability.sh
RUSTFLAGS="-D warnings" cargo test --workspace
RUSTFLAGS="-D warnings" cargo test --workspace --features speech
cargo clippy --all-targets --features speech,trace-profiling -- -D warnings
cargo clippy --workspace --lib --bins --examples --features speech -- -D warnings -D clippy::unwrap_used -D clippy::expect_used
cargo clippy --workspace --all-targets --features speech -- -D warnings -W clippy::pedantic
cargo build
```

The engine tests must include `navigation::tests::*` (pending-navigation
state machine), `wait::tests::*` (pending-wait state machine), the MCP tests
`server::tests::navigation_outcomes_map_to_typed_tool_results` and
`server::tests::wait_inputs_map_to_one_bounded_engine_action`, and the host
test `create_readiness_waits_for_the_backend_and_the_committed_first_page`.

## 3. Official MCP gate per backend

The runner launches the exact debug binary with an agent panel, drives the
public MCP tools against the committed fixtures, prints `MCP gate passed`, and
then waits for a normal close of that window. Close it through the window's
close button; do not kill the process. Run each backend from the clean exact
head (no `--allow-dirty`):

```bash
python3 scripts/browser-smoke/run.py --backend safari --ephemeral --skip-handoff
python3 scripts/browser-smoke/run.py --backend firefox --ephemeral --skip-handoff \
  --firefox-command "/Applications/Firefox.app/Contents/MacOS/firefox" \
  --geckodriver-command "$(command -v geckodriver)"
python3 scripts/browser-smoke/run.py --backend chromium --ephemeral --skip-handoff \
  --chromium-command "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
```

Each run must end with `MCP gate passed`, `gate exit=0` semantics (the runner
exits 0), zero `audit_permission_findings`, zero `network_capture_findings`,
and an empty `surviving_browser_processes` list in the final JSON line. Keep
the printed smoke root's `logs/` for the report and delete the root afterwards.

What the create lane proves on every backend (`create_panel` in
`scripts/browser-smoke/mcp_gate.py`, stacked PR for #348):

- `browser_create` with the fixture URL returns `navigation: "committed"`,
  the panel already reporting the committed `index.html` URL, an integer
  `startup_millis`, and an immediate `browser_query` for `#smoke-input` finds
  exactly one node. Record the printed `create_startup_millis` per backend in
  the report (Firefox is expected to be the slowest, several seconds).

What the wait lane proves on every backend (`exercise_wait_outcomes`, stacked
PR for #349):

- On `delayed.html?delay=4000`, `browser_wait` for `#late-marker` (visible)
  settles from the engine's own observation with one matched node,
  `elapsed_millis` at most 8000 and at least the remainder of the 4000 ms
  fixture delay after the measured navigate wall time, minus one second of
  slack (never below 300), and at least three `polls`; `#early-marker` (hidden)
  settles with `polls: 1`; a selector that never appears fails with the typed
  `wait_timeout` code at a 1.5 s bound; and the final JSON's `wait_outcomes`
  shows `audit_entries_per_wait` of exactly 3 for all three wait action ids;
  removal (`#removed-marker` hidden), an attribute change (`#attr-marker`
  visible), and a style change (`#early-marker` hidden) are each observed as
  a delayed transition on a fresh `delayed.html?delay=4000` load and reported
  under `wait_outcomes.transitions` with three audit entries each.

What the navigation lane proves on every backend (`exercise_navigation_outcomes`
in `scripts/browser-smoke/mcp_gate.py`):

- `browser_navigate` to `next.html` returns `state: committed`,
  `completed: true`, the committed URL, `redirected: false`, and an integer
  `elapsed_millis`.
- `/redirect-to-next` (HTTP 302) returns `committed_url` of `next.html` with
  `redirected: true`.
- `wait: dom_content_loaded` returns `state: dom_content_loaded`.
- `wait: dispatched` returns `state: dispatched` without a committed URL.
- An unreachable loopback destination (`http://127.0.0.1:9/unreachable`) is a
  `navigation_failed` error audited as failed, and the previously committed
  page is retained.
- Chromium and Firefox only: a 3-second bound on the 11-second page returns
  `state: timed_out`, `completed: false`, no committed URL, and the page
  later commits (`#slow-marker` becomes visible). Safari's classic WebDriver
  applies the bound as its page-load timeout and reports `timed_out` too; its
  retained slow-page lane now starts with a 3-second bound that must return
  `state: timed_out`, then waits for the panel URL to reach the slow page (the
  driver polls the classic navigation to its commit) before `#slow-marker`.

Backend notes to confirm in the report:

- Safari: `committed` and `dom_content_loaded` are reported after the classic
  navigation returned, with `loading: false` and a non-empty `title`. The
  classic protocol has no dispatch-only navigation, so `wait: dispatched`
  also returns after the load (the gate's dispatch lane still passes because
  `#next-marker` is already present).
- Firefox: the commit is observed at BiDi `DOMContentLoaded`; `committed` and
  `dom_content_loaded` therefore settle at the same event.
- Chromium: `committed` arrives from `Page.frameNavigated` before the load
  event, so `loading: true` is expected on a committed outcome.

## 4. Visual and lifecycle review

While the Safari gate window is open, confirm the Horizon browser panel shows
the fixture pages as the lane navigates, that the panel's URL chip follows the
committed URL (including the redirect target), and that the timed-out
navigation on Firefox/Chromium eventually shows the slow page. After each
normal close, confirm no `safaridriver`, `geckodriver`, Firefox, or Chrome
processes started by the gate remain (`ps` filtered by the smoke root path).

Capture privacy-safe screenshots of the Horizon window during the Safari lane
at the redirect target and at the slow page. Record filenames and image
dimensions in the report; do not commit screenshots.

## 5. PR report

Before execution, post on the PR:

```text
SMOKE-TEST REQUEST Mac Studio/macOS — plan: docs/testing/2026-09-02-macos-browser-navigation-outcome-smoke.md — scope: native build and matrix, Safari/Firefox/Chrome MCP gates with typed navigation outcomes, create readiness, and engine-side waits, visual and lifecycle review
```

If a behavior change is pushed, rerun every affected lane on the new SHA.

Post the final report as:

```text
SMOKE-TEST REPORT (Mac Studio/macOS <version>, <arch>, <final-sha>)
- exact-head/native build and matrix: pass | fail — ...
- Safari MCP gate (typed navigation outcomes, create readiness, engine-side waits): pass | fail — ... (create_startup_millis, wait_outcomes)
- Firefox MCP gate (typed navigation outcomes, bounded timeout, create readiness, engine-side waits): pass | fail — ... (create_startup_millis, wait_outcomes)
- Chrome MCP gate (typed navigation outcomes, bounded timeout, create readiness, engine-side waits): pass | fail — ... (create_startup_millis, wait_outcomes)
- visual URL chip / redirect / slow page: pass | fail — ...
- lifecycle and process isolation: pass | fail — ...
Summary: <evidence locations, backend notes, remaining concerns>
SMOKE-TEST: DONE
```

## 6. Post-merge verification on main

The #367 report noted one Firefox gate attempt discarded after the Mac's temp
volume hit `ENOSPC` before the wait lane; the clean rerun passed, but the host
was left with about 3 GiB free. After the host was cleaned (merged-PR
worktrees and their build outputs, stale smoke roots, and the Homebrew cache
removed; about 55 GiB free afterwards), the full gate is rerun once more on
the merged tree so the closing record was produced on a healthy host.

Run sections 1 to 4 unchanged against the exact head named in the request on
this PR. That head differs from `main` at `82d070c` only by this document:
the gate script, the fixtures, and every Rust source are byte-identical to
the merged tree, so the run verifies what shipped. Post the report on this
PR in the section 5 format, naming both the PR head and `82d070c`.
