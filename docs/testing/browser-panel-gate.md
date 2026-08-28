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
| `crates/horizon-browser-mcp/**`, agent registration, bundled skills, coordination manifests, audit | MCP contract on one available backend; real bundled-agent discovery and actor forwarding; audit/redaction; handoff/hand-back |
| Semantic snapshot/query/ref or action model | MCP contract on Chromium and Firefox; Safari on macOS if shared WebDriver code changed |
| Click, pointer, keyboard, IME, wheel, scrollbar, focus, or viewport input | Physical input + MCP input on every affected backend; no-manual-resize regression; resize/fit |
| `cdp.rs`, `websocket.rs`, Chromium process/frames/input | Chromium protocol, visual, navigation, input, failure, cleanup; performance if frame delivery changed |
| `webdriver/**`, WebDriver process/actions/screenshots | Firefox and Safari on macOS; Firefox on Linux; session arbitration and exact driver/browser cleanup |
| Firefox BiDi/preload/disclosure | Firefox protocol, disclosure, input, first-frame/no-resize, failure, cleanup on Linux and macOS |
| Safari/classic WebDriver/optional BiDi | Full macOS Safari lane, including capability fallback and single-session arbitration |
| `frames.rs`, adaptive capture, JPEG/decode, wakeups, texture/render code | Visual/layout on all affected backends; static decay, animation, latency, rapid resize, 1/3/5-panel scale |
| Browser widget chrome, canvas, detach, focus, panel layout, visibility | Visible/hidden creation and toggle; hidden-session persistence; visual/layout, physical input, detach motion trace, persistence on the host OS |
| Process ownership, shutdown, Retry, profiles, paths, persistence | Crash/Retry, normal close, relaunch, backend switch, permanent panel close, exact-child/listener/manifest cleanup |
| Automation disclosure or browser launch arguments | Both disclosure policies on Chromium and Firefox; Safari capability/status check; startup/history consistency |
| Browser config schema, defaults, migration, discovery paths | Auto-discovery plus explicit-path launch, persistence/migration, and update this runner's generated config |
| MCP schemas or public tool behavior | MCP contract on all supported backends and bundled-agent discovery; update MCP README and skill together |
| Network events, HTTP response bodies, WebSocket capture/watch, NDJSON writer, capture directory, or retention | High-rate network fixture through MCP on Chromium and Firefox; E24 live-data correctness probe on both when body capture changes and the market is open; Safari unsupported response; cursor/filter/timeout/stop/gap/truncation/drop/lifecycle/audit checks; age/count/aggregate-byte retention probe; hidden capture; normal close during capture; permanent profile cleanup |
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
- launches the exact Horizon binary with an agent panel and no Browser panel;
- invokes only the fourteen public `browser_*` MCP tools;
- proves empty discovery followed by an audited hidden `browser_create` in the
  requesting agent's workspace, controls it while hidden, shows it through
  `browser_visibility`, then checks schemas, navigation,
  snapshot/query/ref lifetime, trusted
  single and double click, Unicode fill, scroll, history, wait/evaluate,
  disclosure, redacted audit, failures, and optional handoff;
- proves an immediate backend navigation rejection returns a typed MCP failure,
  retains the last valid page, and is audited as failed rather than completed;
- disconnects the MCP stdio client, starts a fresh client with the same actor,
  rediscovers the exact live panel, and proves resumed snapshot plus complete
  audit states without restarting the browser;
- starts a filtered capture before navigation to the high-rate WebSocket
  fixture, hides the panel without stopping capture, and verifies a native
  bounded HTTP response body plus sent/received/open/close records, a zero-drop
  4,096-frame burst, and a separate 17-frame reconnect;
- proves `browser_network_watch` delayed and immediate matches, URL/event
  filtering, cursor resume without duplicates, payload exclusion and an
  additional return-size limit, bounded empty timeout, and wake-up on capture
  stop before parsing only the path returned by MCP;
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

### G1b — live E24 market-data correctness probe

Run this targeted online gate when HTTP response-body capture, native network
transport, or high-rate processing changes. It controls the panel only through
MCP: E24 and every script/data response are loaded by the browser. The runner
consumes filtered records through `browser_network_watch`; it may use read-only
Unix tools only on the exact private NDJSON path returned by `browser_network`.
It never fetches market data independently with Python, curl, or another HTTP
client.

During Oslo market hours (weekdays 09:00–16:20), run both supported network
backends from a clean exact-head checkout:

```bash
python3 scripts/browser-smoke/e24_smoke.py --backend firefox --horizon target/debug/horizon
python3 scripts/browser-smoke/e24_smoke.py --backend chromium --horizon target/debug/horizon
```

Each run captures native, filtered market responses, waits one minute, reloads,
and requires visible DOM prices and daily changes to match readable payload
values loaded by E24 in that browser, both before and after the interval. If a
future wire body is encrypted, locate the already-loaded minified bundle and
observe or invoke its page-world decode path through `browser_evaluate`; do not
independently download the bundle, invent a key, or label the encrypted
envelope as a quote. The run also checks watch cursors,
audit completeness, capture bounds, drops, truncation, normal window close,
manifests, and exact task-owned browser cleanup. The report records timings and
a compact market summary. Use `--allow-dirty` only for provisional diagnosis
and `--keep-open` only when a tester is present to close the exact window.
Because this gate uses a public site, report external markup/feed drift
separately from a browser regression and keep G1 as the deterministic pass/fail
oracle.

Use the periodic mode to prove sustained, low-overhead consumption and produce
an agent-readable summary every 30 seconds for five minutes:

```bash
python3 scripts/browser-smoke/e24_smoke.py --backend firefox --horizon target/debug/horizon \
  --observation-seconds 300 --summary-interval-seconds 30
python3 scripts/browser-smoke/e24_smoke.py --backend chromium --horizon target/debug/horizon \
  --observation-seconds 300 --summary-interval-seconds 30
```

The runner advances the `browser_network_watch` sequence cursor, so each
interval processes only new browser-originated records without rereading the
capture. E24 currently populates `/bors` with readable HTTP/JSONP snapshots
rather than a WebSocket and does not update the table autonomously, so the
runner reloads through MCP at each boundary. Every `e24_market_summary` line includes
price changes, daily gainers and losers, fresh-record counts, capture health,
timing, and an exact DOM/feed comparison. A typed reload timeout gets one
audited `browser_navigate` fallback and remains visible in `reload_retry`; any
failed fallback, missing interval, capture loss, or weak DOM match fails the
gate.
The watch contract may return only bounded metadata/prefixes for E24's very
large non-quote responses. The runner reports those records, never decodes a
record marked `truncated`, and derives quotes only from complete browser-loaded
JSONP. Any truncation or drop in the underlying capture remains a failure.

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
5. Open `upload.html`. If the backend surfaces a native picker, select the
   committed `upload.txt` and verify its name, size, and type in the page
   oracle. Then drag that file from Finder or the host file manager onto the
   drop target. Horizon currently owns host file drops as workspace file-open
   actions: verify an editor panel opens and the page's `dropped` list remains
   empty. Record when a headless backend provides no native picker; do not use
   synthetic JavaScript events as evidence of physical upload or drop support.
6. Navigate among `index.html`, `next.html`, and `alternate.html`; exercise
   back, forward, reload, URL submission, and an unreachable loopback URL. An
   error retains the last valid frame and committed URL and offers Retry.
   Submit a hostname without a scheme and require the bar plus committed page
   to use `https://`; submit the fixture's explicit `http://` URL and require
   that override to remain HTTP.
7. Resize repeatedly, Fit, panel fullscreen, canvas zoom/pan, partially
   off-screen placement, detach, native-window move/resize, and reattach.
   Capture launch and post-interaction screenshots. Record a native position
   trace or short video for detach/resize motion; still screenshots alone do
   not prove motion behavior.
8. Switch backends and save. Normal close and isolated relaunch must restore
   backend, committed URL/title, profile, geometry, and focus. Permanently
   closing a separate panel removes only its profile after the browser exits.
9. Break only a task-owned disposable browser or driver. Verify cleared stale
   pixels, actionable error, bounded teardown, Retry after cleanup, recovery of
   the last committed HTTPS URL, fresh input after reconnect, and no PID reuse
   assumptions. Close during active navigation/capture/resize as a separate
   focused pass. A protocol disconnect must either rebind transparently or
   surface this bounded Retry path; it must never leave a frozen stale frame.
10. After every normal close, verify the exact browser/driver tree, driver
   thread, loopback listeners, live manifest, and disposable profile state.
   Never use a broad process-name kill or inspect/reuse an existing session.

### G3 — performance lane

Use the same exact build, fixture, viewport, display, browser version, quality,
and interaction script for comparisons.

1. Leave `index.html` unchanged. Firefox/Safari adaptive screenshot requests
   and completions must decay to zero; Chromium performs no decode/upload
   without a repaint.
2. Collect at least 20 one-input-to-visible-frame samples and 20 alternating
   `next.html`/`alternate.html` navigation samples. Current engine-side Firefox
   gate: median at most 100 ms and p95 at most 180 ms. ScreenCaptureKit
   end-to-end samples include host capture-delivery cost; when that path misses
   the absolute gate, report both the exact values and a workload-matched base
   build on the same host instead of hiding the miss or inferring a regression.
3. Run `animation.html` for at least three seconds. Adaptive capture stays
   active at no more than 30 fps and decays after returning to the static page.
4. Stress rapid input, scroll, and resize. Include a paced trace that moves the
   pointer while sending same-direction wheel bursts, reverses direction, and
   captures the exact window during and after the gesture. The page must keep
   visibly advancing, settle promptly after input stops, and retain the exact
   committed URL throughout. At most one adaptive capture is in flight, newest
   demand wins, queues remain bounded, direction changes remain ordered, and
   stale-size/navigation frames are not published.
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
Firefox, geckodriver, Safari, Accessibility permission for native input, and
Screen Recording permission for visible-frame latency. Identify windows by
exact PID, not application name.

Compile the committed helpers into a temporary directory. The native helper
accepts global display-point coordinates and always activates a root or
detached window belonging to the exact PID. The latency helper binds both the
PID and Core Graphics window ID; its crop is window-local capture pixels.

```bash
browser_smoke_tools=$(mktemp -d)
xcrun swiftc -warnings-as-errors -parse-as-library \
  scripts/browser-smoke/macos_native.swift \
  -o "$browser_smoke_tools/native"
xcrun swiftc -warnings-as-errors -parse-as-library \
  scripts/browser-smoke/macos_latency.swift \
  -o "$browser_smoke_tools/latency"
```

Run G0 once, then run G1–G3 for all three backends:

```bash
python3 scripts/browser-smoke/run.py --backend chromium --horizon target/debug/horizon
python3 scripts/browser-smoke/run.py --backend firefox --horizon target/debug/horizon
python3 scripts/browser-smoke/run.py --backend safari --horizon target/debug/horizon
```

For each runner PID, enumerate its windows before driving it and capture the
exact window ID with `screencapture`. Useful native actions are intentionally
small and composable:

```bash
"$browser_smoke_tools/native" "$candidate_pid" any windows
screencapture -x -l "$window_id" browser-launch.png
"$browser_smoke_tools/native" "$candidate_pid" root click X Y
"$browser_smoke_tools/native" "$candidate_pid" root double X Y
"$browser_smoke_tools/native" "$candidate_pid" root text 'Az09!?ø漢🙂'
"$browser_smoke_tools/native" "$candidate_pid" root key 48 shift
"$browser_smoke_tools/native" "$candidate_pid" root repeat 0 '' 8
"$browser_smoke_tools/native" "$candidate_pid" root buttons X Y
"$browser_smoke_tools/native" "$candidate_pid" root scroll X Y DX DY
"$browser_smoke_tools/native" "$candidate_pid" root drag X1 Y1 X2 Y2 30 shift-release
"$browser_smoke_tools/native" "$candidate_pid" root close
```

Use `key` for physical key/modifier coverage and `text` for an exact Unicode
oracle; punctuation produced by a physical key code depends on the active
keyboard layout. The drag form above deliberately changes to Shift before an
outside release and therefore catches gesture-ownership bugs. After detaching,
wait five seconds for native-window membership to settle, use `detached`, and
run a trace concurrently with a title-bar or resize-edge drag:

```bash
"$browser_smoke_tools/native" "$candidate_pid" detached trace 300 5 > motion.csv &
browser_motion_trace_pid=$!
"$browser_smoke_tools/native" "$candidate_pid" detached drag X1 Y1 X2 Y2 30
wait "$browser_motion_trace_pid"
```

The trace must show monotonic progress without alternating coordinates or snap
back. Re-resolve coordinates after Fit, detach, zoom, or resize; never reuse a
stale screenshot. Move or fit a panel before lower-right assertions if the
Horizon minimap covers the page. To collect the two G3 latency sets:

```bash
"$browser_smoke_tools/latency" "$candidate_pid" "$window_id" input \
  INPUT_GLOBAL_X INPUT_GLOBAL_Y CROP_X CROP_Y CROP_W CROP_H input.json
"$browser_smoke_tools/latency" "$candidate_pid" "$window_id" navigation \
  URL_GLOBAL_X URL_GLOBAL_Y CROP_X CROP_Y CROP_W CROP_H navigation.json \
  "$fixture_base_url"
```

Inspect every crop once before trusting the numbers. The native helper drives
the foreground GUI, so do not work in the same desktop while it samples.

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
3. Repeat hidden creation, show/hide persistence, no-manual-resize
   click/wheel/scrollbar/viewport convergence, atomic trusted MCP double-click,
   navigation, layout, performance, and cleanup. Both `browser_network start`
   and `browser_network_watch` must return the typed unsupported-backend result.
4. Start a second Safari panel. It must report a safe busy state without
   disturbing the first. Close the first normally, wait for cleanup, then Retry
   the second.
5. Break only the task-owned automation session, verify bounded cleanup and
   Retry, and prove that normal Safari windows, history, cookies, AutoFill,
   settings, profiles, and default-browser choice did not change.
6. Under signal minimization Safari reports that the policy is unsupported by
   the backend; do not claim parity with Chromium/Firefox preload behavior.
7. Run the MCP gate's 11-second delayed navigation. It must complete through
   Safari's bounded page-load policy instead of failing at the ordinary
   10-second WebDriver I/O limit; the following invalid navigation must still
   return a typed failure and retain the delayed page.

Never terminate or automate an unrelated Safari window. Break only the
`safaridriver` descendant owned by the printed candidate PID, and re-enumerate
the exact Horizon windows after Retry.

Safari is not release-supported until this exact-head lane passes on macOS.

## MCP discovery, steering, and audit lane

Run once per host OS after the semantic gate:

1. Launch Horizon with an isolated home. Verify boot synchronizes the bundled
   `horizon-browser` skill and Claude plugin descriptor without adding a
   permanent MCP registration to the operator's configuration.
2. Create a default supported agent panel with no Browser panel. The default
   launch receives the transient `horizon-browser` stdio MCP registration and
   stable `HORIZON_BROWSER_ACTOR`, and that exact variable is forwarded to the
   stdio MCP child. The default Codex registration sets only this MCP server's
   tool approval mode to `approve`, so browser calls proceed without repeated
   operator prompts while unrelated tool approvals keep their normal policy; a
   custom agent command remains unchanged. Prove the forwarding with a real
   in-panel `browser_create`, not only by testing a synthetic MCP process with
   an actor injected directly.
3. From the agent, start with `browser_list`, observe the empty result, and call
   `browser_create` with `visible: false` in the same workspace. Verify its
   queued/dispatched/completed `session_created` audit records, control it while
   hidden, show it with `browser_visibility`, take a snapshot/query, interact by
   fresh ref, verify the result, hide and show it again, request handoff, wait
   for hand-back, and inspect `browser_audit`. Save and relaunch an isolated
   session once to prove hidden state restores without losing the live browser
   contract. The default Codex panel must complete this sequence without
   per-call MCP approval prompts. It must not know a manifest path or raw
   endpoint and must not invoke a browser-control CLI.
4. Confirm user click, key, URL submission, and navigation preempt agent
   actions for the bounded user-active interval. Handoff visibly blocks further
   agent actions until the exact request is acknowledged.
5. Verify audit order and action IDs; queued/dispatched/terminal states;
   mode `0600` on Unix; URL credentials/query/fragment, selectors, scripts, and
   filled text redaction; and no runtime path or raw endpoint in MCP results.
6. Inspect the `network_capture` discovery object. On Chromium it must report
   protocol-native frames and CDP response bodies; on Firefox it must disclose
   page instrumentation for frames and native WebDriver BiDi response bodies;
   on Safari it must report unsupported. Follow the advertised
   `browser_network start` before navigation, opt-in body,
   `browser_network_watch` cursor loop, optional status/tail, and stop workflow.
   Hide the panel during part of the stream and prove counters and cursor
   delivery continue. Verify immediate and delayed matches, an empty timeout,
   capture-stop wake-up, URL/event filtering, no duplicate sequence after
   resume, payload exclusion by default, explicit returned-payload truncation,
   partial/malformed final-line handling, sequence-gap and connection-map
   truncation reporting, and explicit capture replacement/reset state.
   The returned capture file must be private, monotonic NDJSON; URL query data
   must be redacted; `records_dropped`, `writer_failed`, and
   `file_limit_reached` must remain false for the deterministic 4,096-frame
   stream. The automated MCP lane seeds an expired export, 65 fresh exports,
   and a sparse aggregate-byte pressure file before another start. Prove the
   expired and oversized files are pruned, at most 64 exports remain, the
   newest completed export survives, and the next capture's full requested
   budget fits inside the 1 GiB aggregate limit. Repeat normal close while a
   capture is active and prove it flushes and leaves no writer thread or live
   manifest. Permanently close a disposable panel (or delete its saved
   session) and prove its profile-root `captures/` directory is removed with
   the browser profile.

## Fixture inventory

| Page | Purpose |
| --- | --- |
| `index.html` | Static baseline, early/current/iframe disclosure, trusted click/double-click, text/key events, drag/release, horizontal and vertical wheel, right-edge content, scrollbar, fingerprint consistency |
| `scroll.html` | Minimal tall semantic page for wheel/scrollbar and trusted-click isolation |
| `next.html` | Blue committed navigation/history target |
| `alternate.html` | Red alternating navigation-latency target |
| `animation.html` | Deterministic CSS plus canvas repaint workload |
| `websocket.html` | Native HTTP response body plus deterministic WebSocket disconnect/reconnect lifecycle, sent frames, a 4,096-frame burst plus 17-frame reconnect, URL redaction, bounded NDJSON export |
| `upload.html` + `upload.txt` | Native file-picker oracle and the documented host file-drop/workspace-open boundary |

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
