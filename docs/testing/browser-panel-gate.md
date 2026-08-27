# Browser panel validation gate

This is the durable test gate for Horizon's first-party browser panel, the
`horizon-browser` engine, and the `horizon-browser` MCP contract. Use the gate
selector below for focused changes. Run every lane for a full Linux or macOS
browser validation.

The deterministic site and MCP client live under
`scripts/browser-smoke/`. They are repository test infrastructure, not a
second browser-control API and not part of the future `horizon-browser` crate
package. Agents must control live panels only through the public MCP tools.

Historical evidence is recorded separately in
[`2026-08-26-horizon-browser-panel-smoke.md`](2026-08-26-horizon-browser-panel-smoke.md).
It proves only the exact SHA, host, and versions named there; it is not the
current procedure.

## Gate selector

Always run the repository gate. Add every applicable focused lane; rows are
cumulative.

| Change | Required smoke lanes |
| --- | --- |
| Documentation or fixture text only | Fixture self-check; validate every command or selector changed |
| `crates/horizon-browser-mcp/**`, agent registration, bundled skills, coordination manifests, audit | MCP contract on one available backend; discovery; audit/redaction; handoff/hand-back |
| Semantic snapshot/query/ref or action model | MCP contract on Chromium and Firefox; Safari on macOS if shared WebDriver code changed |
| Click, pointer, keyboard, IME, wheel, scrollbar, focus, or viewport input | Physical input + MCP input on every affected backend; no-manual-resize regression; resize/fit |
| `cdp.rs`, `websocket.rs`, Chromium process/frames/input | Chromium protocol, visual, navigation, input, failure, cleanup; performance if frame delivery changed |
| `webdriver/**`, WebDriver process/actions/screenshots | Firefox and Safari on macOS; Firefox on Linux; session arbitration and exact driver/browser cleanup |
| Firefox BiDi/preload/disclosure | Firefox protocol, disclosure, input, first-frame/no-resize, failure, cleanup on Linux and macOS |
| Safari/classic WebDriver/optional BiDi | Full macOS Safari lane, including capability fallback and single-session arbitration |
| `frames.rs`, adaptive capture, JPEG/decode, wakeups, texture/render code | Visual/layout on all affected backends; static decay, animation, latency, rapid resize, 1/3/5-panel scale |
| Browser widget chrome, canvas, detach, focus, panel layout | Visual/layout, physical input, detach motion trace, persistence on the host OS |
| Process ownership, shutdown, Retry, profiles, paths, persistence | Crash/Retry, normal close, relaunch, backend switch, permanent panel close, exact-child/listener/manifest cleanup |
| Automation disclosure or browser launch arguments | Both disclosure policies on Chromium and Firefox; Safari capability/status check; startup/history consistency |
| Browser config schema, defaults, migration, discovery paths | Auto-discovery plus explicit-path launch, persistence/migration, and update this runner's generated config |
| MCP schemas or public tool behavior | MCP contract on all supported backends and bundled-agent discovery; update MCP README and skill together |
| Network events, WebSocket capture, NDJSON writer, or capture directory | High-rate network fixture through MCP on Chromium and Firefox; Safari unsupported response; lifecycle/status/file/drop/truncation/audit checks; normal close during capture |
| Dependency, feature, packaging, or public crate boundary | Repository gate, rustdoc, clean package dry-run, macOS and Windows cross-target checks |
| Release/support claim or broad browser refactor | Full Linux and full macOS gates on the exact candidate head; Windows follow-up where support is claimed |

If a change spans rows, run the union. A fix found during smoke invalidates the
affected evidence until the lane is repeated on the new exact head.

## Gate levels

### G0 — repository gate, always

Run in the exact checkout and commit that will be pushed:

```bash
cargo fmt --all -- --check
./scripts/check-maintainability.sh
./scripts/check-version-sync.sh
RUSTFLAGS="-D warnings" cargo test --workspace
RUSTFLAGS="-D warnings" cargo test --workspace --features speech
cargo clippy --all-targets --features speech,trace-profiling -- -D warnings
cargo clippy --workspace --lib --bins --examples --features speech -- -D warnings -D clippy::unwrap_used -D clippy::expect_used
cargo clippy --workspace --all-targets --features speech -- -D warnings -W clippy::pedantic
```

For package-boundary changes, also run:

```bash
cargo doc -p horizon-browser --no-deps
cargo package -p horizon-browser
cargo check -p horizon-browser --target x86_64-apple-darwin
cargo check -p horizon-browser --target x86_64-pc-windows-gnu
cargo check -p horizon-browser --target x86_64-pc-windows-msvc
```

The targets and their linkers/toolchains must already be installed. A clean
package dry-run is required; do not publish the crate.

### G1 — deterministic fixture and MCP contract

The reusable runner:

- creates a task-owned root with isolated home, config, profiles, logs, and
  proof directories;
- rejects a dirty checkout so recorded final evidence identifies one exact
  commit (use `--allow-dirty` only for an explicitly provisional diagnosis);
- serves the committed fixtures on a random loopback port;
- launches the exact Horizon binary and one configured Browser panel;
- invokes only the eleven public `browser_*` MCP tools;
- checks discovery, schemas, navigation, snapshot/query/ref lifetime, trusted
  single and double click, Unicode fill, scroll, history, wait/evaluate,
  disclosure, redacted audit, failures, and optional handoff;
- starts a filtered capture before navigation to the high-rate WebSocket
  fixture, verifies HTTP plus sent/received/open/close records and a zero-drop
  4,096-frame tail, then stops and parses only the path returned by MCP;
- waits for the tester to close the exact Horizon window normally; and
- fails if the candidate exits badly or a task-owned browser process or live
  manifest remains.

Build once, then run one backend:

```bash
cargo build
python3 scripts/browser-smoke/run.py \
  --backend chromium \
  --horizon target/debug/horizon
```

Use `--chromium-command`, `--firefox-command`, `--geckodriver-command`, or
`--safaridriver-command` only when auto-discovery is ambiguous. The runner
prints the exact candidate PID and artifact root. Scope every screenshot and
native action to that PID. It pauses at MCP handoff until the tester clicks
**Done — hand back to agent** in that exact window.

Use `--skip-handoff` only when handoff is outside the selected gate. Use
`--ephemeral` for focused non-persistence work. To test restore, omit
`--ephemeral`, close normally, then rerun with the printed root:

```bash
python3 scripts/browser-smoke/run.py \
  --backend firefox \
  --horizon target/debug/horizon \
  --root /tmp/horizon-browser-smoke.EXAMPLE \
  --skip-mcp
```

The second run reuses the deterministic port and isolated state. Confirm the
saved backend, committed URL, title, profile identity, geometry, and focus
before changing anything.

### G2 — backend UI and lifecycle lane

Run these checks on each selected backend while G1 is open:

1. Before resizing, verify a crisp initial frame, accurate committed URL/title,
   working click, wheel, and focus. Switching Chromium to Firefox must also
   work on its first converged frame; resizing is never a workaround.
2. Type ASCII, punctuation, Shift-modified text, and a multi-character Unicode
   string. Exercise Tab, Shift+Tab, Enter, Escape, arrows, Home/End,
   Backspace/Delete, and held-key repeat. Text and key events appear once and
   do not leak into Horizon shortcuts or the URL bar.
3. Click, physical double-click, drag/release outside, use simultaneous button
   masks where supported, wheel both axes, and lose/refocus the window. No
   button or modifier remains stuck. Then let G1 prove the atomic agent
   `count: 2` action and its audit entry separately.
4. On the tall fixture, wheel, drag the visible scrollbar thumb, and click its
   track. Click **Right-edge content** to prove gutter handling does not steal
   page input. Firefox must show Horizon's validated overlay while its real
   WebDriver gutter remains authoritative.
5. Navigate among `index.html`, `next.html`, and `alternate.html`; exercise
   back, forward, reload, URL submission, and an unreachable loopback URL. An
   error retains the last valid frame and committed URL and offers Retry.
6. Resize repeatedly, Fit, panel fullscreen, canvas zoom/pan, partially
   off-screen placement, detach, native-window move/resize, and reattach.
   Capture launch and post-interaction screenshots. Record a native position
   trace or short video for detach/resize motion; still screenshots alone do
   not prove motion behavior.
7. Switch backends and save. Normal close and isolated relaunch must restore
   backend, committed URL/title, profile, geometry, and focus. Permanently
   closing a separate panel removes only its profile after the browser exits.
8. Break only a task-owned disposable browser or driver. Verify cleared stale
   pixels, actionable error, bounded teardown, Retry after cleanup, and no PID
   reuse assumptions. Close during active navigation/capture/resize as a
   separate focused pass.
9. After every normal close, verify the exact browser/driver tree, driver
   thread, loopback listeners, live manifest, and disposable profile state.
   Never use a broad process-name kill or inspect/reuse an existing session.

### G3 — performance lane

Use the same exact build, fixture, viewport, display, browser version, quality,
and interaction script for comparisons.

1. Leave `index.html` unchanged. Firefox/Safari adaptive screenshot requests
   and completions must decay to zero; Chromium performs no decode/upload
   without a repaint.
2. Collect at least 20 one-input-to-visible-frame samples and 20 alternating
   `next.html`/`alternate.html` navigation samples. Current Firefox gate:
   median at most 100 ms and p95 at most 180 ms.
3. Run `animation.html` for at least three seconds. Adaptive capture stays
   active at no more than 30 fps and decays after returning to the static page.
4. Stress rapid input, scroll, and resize. At most one adaptive capture is in
   flight, newest demand wins, queues remain bounded, and stale-size/navigation
   frames are not published.
5. Compare one, three, and five panels for process, thread, CPU, and RSS growth.
   Report exact measurements; do not generalize beyond the measured host.
6. For Chromium, record renderer/GPU diagnostics. Removing `--disable-gpu`
   does not prove hardware acceleration under Xvfb, remote desktop, or a VM.

## Full Linux gate

Prerequisites: debug build, Chromium, Firefox, geckodriver, Xvfb, a lightweight
window manager, `xdotool`, and a screenshot tool. Prefer a task-owned display.
Choose an unused display number rather than copying this example blindly:

```bash
test ! -e /tmp/.X11-unix/X91
Xvfb :91 -screen 0 1600x1000x24 -nolisten tcp > /tmp/horizon-browser-xvfb.log 2>&1 &
browser_xvfb_pid=$!
DISPLAY=:91 openbox > /tmp/horizon-browser-openbox.log 2>&1 &
browser_wm_pid=$!
```

Run G0 once and G1–G3 for both backends:

```bash
DISPLAY=:91 python3 scripts/browser-smoke/run.py --backend chromium --horizon target/debug/horizon
DISPLAY=:91 python3 scripts/browser-smoke/run.py --backend firefox --horizon target/debug/horizon
```

Drive and inspect only the printed candidate PID. Close each Horizon window
through the window manager and wait for the runner's cleanup report. After both
runs finish, stop only the display and window-manager PIDs created above:

```bash
kill "$browser_wm_pid" "$browser_xvfb_pid"
```

A full Linux pass includes both disclosure policies, persistence relaunch,
crash/Retry, one/three/five-panel scaling, screenshots, motion evidence, and
agent discovery. The runner's default covers `minimize_common_signals`; repeat
the focused disclosure lane with `--automation-disclosure browser_default`.

## Full macOS gate

Prerequisites: logged-in GUI session, Xcode Command Line Tools, Chromium,
Firefox, geckodriver, Safari, and Accessibility permission for the selected
native automation tool. Identify windows by exact PID, not application name.

Run G0 once, then run G1–G3 for all three backends:

```bash
python3 scripts/browser-smoke/run.py --backend chromium --horizon target/debug/horizon
python3 scripts/browser-smoke/run.py --backend firefox --horizon target/debug/horizon
python3 scripts/browser-smoke/run.py --backend safari --horizon target/debug/horizon
```

For Chromium, first run without `--chromium-command` to prove discovery from
`/Applications`; do the same for Firefox where the host installation permits.
Capture exact-PID screenshots with `screencapture` and use native automation
for motion traces.

Safari requirements:

1. Record macOS, Safari, and `safaridriver --version`. If automation is not
   enabled, stop and ask the operator to perform Apple's one-time enablement.
   Horizon and this harness must never run `safaridriver --enable`.
2. Record returned capabilities and whether `webSocketUrl` negotiated. If it is
   rejected, expect one clean retry in classic-only mode; never claim BiDi from
   a requested capability alone.
3. Repeat no-manual-resize click/wheel/scrollbar/viewport convergence, atomic
   trusted MCP double-click, navigation, layout, persistence, performance, and
   cleanup.
4. Start a second Safari panel. It must report a safe busy state without
   disturbing the first. Close the first normally, wait for cleanup, then Retry
   the second.
5. Break only the task-owned automation session, verify bounded cleanup and
   Retry, and prove that normal Safari windows, history, cookies, AutoFill,
   settings, profiles, and default-browser choice did not change.
6. Under signal minimization Safari reports that the policy is unsupported by
   the backend; do not claim parity with Chromium/Firefox preload behavior.

Safari is not release-supported until this exact-head lane passes on macOS.

## MCP discovery, steering, and audit lane

Run once per host OS after the semantic gate:

1. Launch Horizon with an isolated home. Verify boot synchronizes the bundled
   `horizon-browser` skill and Claude plugin descriptor without adding a
   permanent MCP registration to the operator's configuration.
2. Create a default supported agent panel beside a Browser panel. The default
   launch receives the transient `horizon-browser` stdio MCP registration and
   stable `HORIZON_BROWSER_ACTOR`; a custom agent command remains unchanged.
3. From the agent, start with `browser_list`, take a snapshot/query, interact by
   fresh ref, verify the result, request handoff, wait for hand-back, and inspect
   `browser_audit`. It must not know a manifest path or raw endpoint and must not
   invoke a browser-control CLI.
4. Confirm user click, key, URL submission, and navigation preempt agent
   actions for the bounded user-active interval. Handoff visibly blocks further
   agent actions until the exact request is acknowledged.
5. Verify audit order and action IDs; queued/dispatched/terminal states;
   mode `0600` on Unix; URL credentials/query/fragment, selectors, scripts, and
   filled text redaction; and no runtime path or raw endpoint in MCP results.
6. Inspect the `network_capture` discovery object. On Chromium it must report
   protocol-native frames; on Firefox it must disclose page instrumentation;
   on Safari it must report unsupported. Follow the advertised
   `browser_network start` before navigation, status/tail, and stop workflow.
   The returned capture file must be private, monotonic NDJSON; URL query data
   must be redacted; `records_dropped`, `writer_failed`, and
   `file_limit_reached` must remain false for the deterministic 4,096-frame
   stream. Repeat normal close while a capture is active and prove it flushes
   and leaves no writer thread or live manifest.

## Fixture inventory

| Page | Purpose |
| --- | --- |
| `index.html` | Static baseline, early/current/iframe disclosure, trusted click/double-click, text/key events, drag/release, horizontal and vertical wheel, right-edge content, scrollbar, fingerprint consistency |
| `scroll.html` | Minimal tall semantic page for wheel/scrollbar and trusted-click isolation |
| `next.html` | Blue committed navigation/history target |
| `alternate.html` | Red alternating navigation-latency target |
| `animation.html` | Deterministic CSS plus canvas repaint workload |
| `websocket.html` | High-rate deterministic WebSocket lifecycle, sent frame, 4,096 received frames, URL redaction, bounded NDJSON export |

Keep selectors stable. When a browser behavior needs a new deterministic
oracle, extend these fixtures and the gate together instead of creating another
temporary `/tmp` page. Do not use public search engines, anti-bot sites, or
CAPTCHAs as pass/fail oracles.

## Pass rule and evidence

A lane passes only when all of the following are true on the recorded exact
head:

- every selected assertion passes and required filters/backend/capabilities
  were visibly active;
- launch and post-interaction screenshots were inspected, with motion evidence
  for motion-sensitive behavior;
- there is no unexplained error banner, stale frame, stuck input, or manual
  resize workaround;
- the app was closed through the normal exact-window path; and
- no task-owned browser/driver process, listener, live manifest, or unintended
  disposable profile remains.

Record exact SHA, OS/display, browser and driver versions, commands, fixture
URL, runner artifact root, screenshots/video, metrics, MCP summary, audit
evidence, and cleanup result. Separate facts from interpretation: a trusted
DOM event proves backend-native protocol input, not human origin or
undetectability.

For an open PR, request cross-machine work in a PR comment using the repository
format and link this document plus the exact selected lanes. Before a PR exists,
hand off the exact pushed branch SHA out of band and do not use the completed
`SMOKE-TEST: DONE` marker until the tester has actually finished. Any pushed
fix requires affected lanes to be rerun on the new head.

Feature-specific temporary plans may contain only delta steps and a link to
this gate. Fold reusable scenarios into this document/fixture set, preserve
dated results as evidence, and delete the temporary delta plan after validation
unless the user explicitly asks to keep it.
