# macOS browser navigation outcome smoke test

Temporary validation artifact for the browser navigation-outcome pull request
(issue #350) and the pull requests stacked on it (#348 create readiness, #349
event-driven wait). Run every lane on a logged-in Mac desktop against the
exact PR head named in the request. Do not merge until the report ends with
`SMOKE-TEST: DONE` and every required lane passes. This dated plan stays in
the repository as the execution record; append findings to the report, do not
rewrite the procedure.

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
state machine) and the MCP test
`server::tests::navigation_outcomes_map_to_typed_tool_results`.

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
SMOKE-TEST REQUEST Mac Studio/macOS — plan: docs/testing/2026-09-02-macos-browser-navigation-outcome-smoke.md — scope: native build and matrix, Safari/Firefox/Chrome MCP gates with typed navigation outcomes, visual and lifecycle review
```

If a behavior change is pushed, rerun every affected lane on the new SHA.

Post the final report as:

```text
SMOKE-TEST REPORT (Mac Studio/macOS <version>, <arch>, <final-sha>)
- exact-head/native build and matrix: pass | fail — ...
- Safari MCP gate (typed navigation outcomes): pass | fail — ...
- Firefox MCP gate (typed navigation outcomes, bounded timeout): pass | fail — ...
- Chrome MCP gate (typed navigation outcomes, bounded timeout): pass | fail — ...
- visual URL chip / redirect / slow page: pass | fail — ...
- lifecycle and process isolation: pass | fail — ...
Summary: <evidence locations, backend notes, remaining concerns>
SMOKE-TEST: DONE
```
