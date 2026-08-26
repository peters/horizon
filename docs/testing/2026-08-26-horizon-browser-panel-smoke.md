# Horizon browser panel smoke test

Temporary exact-head validation plan for the first-party `horizon-browser`
engine and Horizon panel integration. Record the checked-out commit before each
lane. Never reuse, signal, close, or inspect a pre-existing Horizon or normal
browser session.

## Linux execution record (2026-08-26 to 2026-08-27)

- Host: Ubuntu 24.04.4 LTS, Linux 7.0.0-30-generic x86_64, isolated Xvfb.
- Browsers: Chromium 151.0.7922.108 snap; Firefox 154.0; geckodriver 0.35.0.
- Chromium CDP: pass for visible push frames, committed URL/title, Fit/resize,
  Unicode agent input, claim/lease, user preemption, handoff/handback, redacted
  mode-`0600` audit, and exact process/manifest cleanup. A drag on the visible
  native vertical scrollbar moved the deterministic page from `scrollY=0` to
  `scrollY=1255`; a following wheel event moved it to `scrollY=1287` on the
  final rebuilt Linux candidate.
- Firefox WebDriver BiDi: pass for visible adaptive screenshots, committed
  URL/title, Fit/resize, Unicode agent input, claim/lease, user preemption,
  handoff/handback, redacted mode-`0600` audit, and exact
  Horizon-to-geckodriver-to-Firefox cleanup. After a live Chromium-to-Firefox
  switch, the first page click worked without resizing the panel. Firefox wheel
  input worked, its screenshot-frame scrollbar overlay was visible, and dragging
  that indicator through Firefox's real WebDriver gutter moved the page from
  `scrollY=32` to `scrollY=1317` after a wheel step on the final rebuilt Linux
  candidate. A normal exact-window close removed the Horizon, geckodriver, and
  Firefox process tree and the live manifest.
- Finding fixed and rerun: the browser could acknowledge a viewport command
  while continuing to publish an older-sized frame. Pointer input correctly
  waited for authoritative geometry, but no later resize was sent, so Firefox
  appeared inert until the user resized the panel. Backend switches now clear
  all session-owned geometry/input caches and the UI resends viewport commands
  at a bounded interval until the frame converges, with a finite retry budget.
- Finding fixed and rerun: headless Chromium paints a native scrollbar but CDP
  pointer dispatch does not operate browser-owned scrollbar chrome. Presses in
  the measured gutter now use engine-owned thumb/track scrolling while ordinary
  right-edge content clicks remain normal CDP input.
- Finding fixed and rerun: Firefox accepts real WebDriver pointer input in its
  reserved scrollbar gutter, but WebDriver screenshots omit the native
  scrollbar pixels. `horizon-browser` now publishes validated root scroll
  geometry and Horizon paints a non-injected indicator over the omitted gutter;
  Firefox remains authoritative for the drag. Scroll-state-only changes wake
  the UI even when screenshot pixels hash identically.
- Finding fixed and rerun: when geckodriver exited before a Firefox descendant,
  retaining only the driver leader could leave that descendant alive. Cleanup
  now observes an exited process-group leader and reaps every remaining owned
  member before Retry or shutdown completes. Killing only the disposable
  geckodriver produced a cleared frame and Retry state, left no Firefox child,
  and Retry created one fresh driver/browser tree that normal close also reaped.
- Independent review finding fixed: post-reap descendant cleanup is Unix-only.
  A retained Windows child handle is exact while live, but after exit its
  numeric PID can be reused; the candidate therefore never passes an exited PID
  to `taskkill`. The Unix cleanup regression and Windows MSVC cross-check passed.
- Common disclosure minimization: pass on the deterministic local page for
  Chromium and Firefox. The earliest author script, current document, and a
  dynamically created iframe each observed `navigator.webdriver == false`.
  Chromium's user agent contained `Chrome`, not `HeadlessChrome`, while its
  browser-owned Client Hint brands and full version remained populated;
  Firefox retained its native user agent. Visible Horizon-originated clicks
  remained trusted DOM events. This proves the narrow active-session contract,
  not that either browser is undetectable.
- Decisive Chromium disclosure/startup rerun: when the browser-owned user agent
  contained `HeadlessChrome`, the engine created a temporary hidden
  `chrome://version/` target, read Chromium's native high-entropy Client Hint
  values there, and closed it before attaching the caller page. A live
  `Target.getTargets` query after startup contained no bootstrap target, and
  `Page.getNavigationHistory` contained exactly `about:blank` followed by the
  deterministic fixture. The resulting identity retained Chromium's native
  architecture, bitness, brands, platform, and full version
  (`151.0.7922.108`) while removing only the headless user-agent token.
- Exact final-build interaction rerun: Chromium converged to a
  `1144×698` viewport and matching emulated screen without a manual panel
  resize, wheel-scrolled from `scrollY=0` to `scrollY=960`, and reported
  `clicks=1 trusted=true`. The unchanged scrollbar code had already moved from
  `scrollY=0` to `scrollY=931` through the visible native scrollbar on the
  preceding rebuilt candidate. A live Chromium-to-Firefox switch then produced
  a usable first converged Firefox frame without resizing the panel. Firefox
  wheel-scrolled from `0` to `960`,
  reported `clicks=1 trusted=true`, returned to `0` with Home, and its visible
  WebDriver scrollbar interaction moved through `652` to `1887`.
- Chromium retained its native Client Hint brands and full version
  (`151.0.7922.108`) after removing only the `HeadlessChrome` token. Firefox
  retained its native Firefox 154 user agent and a plausible screen larger
  than its content viewport. The fixture recorded these values instead of
  adding broad fingerprint spoofing.
- Browser-default disclosure rerun: both engines reported `BrowserDefault`.
  The earliest author script, current document, and a dynamically created
  same-origin iframe each observed `navigator.webdriver == true`, confirming
  that Horizon had not installed its minimization preload. Chromium retained
  its native `HeadlessChrome` user agent in this mode and Firefox retained its
  native Firefox user agent.
- Firefox double-click limitation: Horizon correctly emitted consecutive
  trusted WebDriver presses with click counts one and two, but stock Firefox
  154/geckodriver 0.35 did not emit a DOM `dblclick` across separate W3C Actions
  commands. WebDriver BiDi Actions has no click-count parameter. Buffering every
  first click would add double-click-interval latency, while synthesizing a DOM
  event would make it untrusted, so neither workaround was accepted. Single
  click, drag, simultaneous-button, wheel, and keyboard assertions passed.
- Exact final-build teardown: normal window close removed the task-owned
  Horizon, Chromium, geckodriver, and Firefox processes, CDP/WebDriver/BiDi
  listeners, and live manifest. The private audit journal remained at mode
  `0600` with the wheel, click, Home, and scrollbar drag represented as
  ordered redacted user actions.
- Chromium no longer receives a default `--disable-gpu` launch argument. The
  isolated Xvfb environment still selected SwiftShader and disabled renderer
  GPU compositing, so this lane does not claim hardware acceleration or explain
  the observed Firefox-versus-Chromium difference by protocol alone.
- Finding fixed and rerun: Chromium retained a target URL-like title after a
  credential-bearing navigation because Chrome omits URL user-info from
  `location.href`. Metadata matching now ignores only authority user-info;
  query, host, path, and path/query `@` characters remain significant.
- Finding fixed and rerun: changing a live panel from Chromium to Firefox did
  not dirty Horizon's runtime state, so the panel restored as Chromium. Backend
  changes now produce a one-shot persistence signal. On code commit `472451c`,
  the live manifest and runtime YAML recorded Chromium plus Firefox, normal
  close removed both task-owned manifests and process trees, and relaunch of
  that same isolated session visibly restored both engines with Firefox BiDi.
- Full persistence rerun: Chromium navigated to the second deterministic page,
  switched to Firefox while preserving the committed URL, title, profile
  identity, geometry, and focus, then saved and restored as Firefox BiDi. It
  subsequently switched back, saved, and restored as Chromium CDP. Every old
  exact process tree was gone before profile reuse and every normal close exited
  cleanly. Permanently closing a separate Firefox panel removed its driver,
  browser, live manifest, and task-owned persistent profile. An unreachable
  Firefox navigation retained the last valid frame and displayed an actionable
  error without replacing the saved committed URL.
- MCP-only steering rerun: both engines completed initialization and all ten
  public MCP tools, including semantic navigation, snapshot/query/ref actions,
  Unicode fill, scroll, evaluation, history/reload, stale-ref errors, redacted
  audit correlation, user preemption, and handoff/handback. MCP stdout remained
  protocol-only and exposed no raw browser endpoint or runtime path. No browser
  control CLI is part of the contract.
- Bundled discovery rerun: an isolated default agent panel received only the
  transient `horizon-browser` stdio registration plus a stable actor identity;
  no registration was written to its persistent configuration. The installed
  discovery skill and Claude descriptor pointed to the exact Horizon executable
  with only `--browser-mcp`, and a regression test proves custom agent commands
  are unchanged. The isolated nested agent reached its fresh-login screen, so
  an authenticated in-agent call was not attempted with operator credentials;
  discovery wiring and the same MCP snapshot flow were verified independently.
- Measured frame latency passed the documented gates. Firefox input samples
  had median 80 ms, p95 123 ms, and maximum 178 ms; navigation samples had
  median 78 ms, p95 129 ms, and maximum 215 ms. Chromium input samples had
  median 32 ms, p95 48 ms, and maximum 55 ms; navigation samples had median
  48 ms, p95 54 ms, and maximum 68 ms. Firefox animation produced 48 frames in
  three seconds, stayed below its 30 fps cap, and decayed after returning to a
  static page; Chromium's CDP push path produced 181 frames in three seconds.
- Firefox scale samples remained bounded and approximately linear: one panel
  used 13 child processes, 346 threads, and 1,539,052 KiB summed RSS; three used
  37 processes, 916 threads, and 3,632,280 KiB; five used 61 processes, 1,477
  threads, and 6,089,768 KiB. Summed RSS double-counts shared pages and the
  five-panel CPU sample included an active animation, so these measurements are
  comparison evidence rather than a general resource claim. Normal WM close
  left no process survivors in all three cases.
- Harness note: directly destroying an X11 drawable is not a normal application
  close and caused winit `BadWindow` errors under load. Those observations were
  discarded. Exact-window `WM_DELETE_WINDOW` through the task-owned window
  manager exited with status zero and fully reaped the one-, three-, and
  five-panel trees.
- The complete repository pre-push matrix passed on the final source candidate:
  formatting, maintainability, both workspace test lanes, blocking Clippy,
  strict no-unwrap/no-expect Clippy, and pedantic Clippy. `horizon-browser`
  rustdoc, macOS plus Windows GNU/MSVC crate cross-target checks, and a clean
  package dry-run also passed; the archive contained 39 files and verified from
  its staged contents. No crate was published. Workspace rustdoc is not a
  release gate and remains blocked by a pre-existing broken intra-doc link in
  `horizon-cursor`. The macOS tester must use the final remote SHA, not an
  earlier push made before the persistence, viewport/input, or disclosure-startup
  findings.
- Runtime screenshots and journals are temporary task artifacts outside the
  repository; the durable evidence is this record plus the exact pushed SHA.

## Pre-PR macOS branch handoff

No PR exists yet. After the Linux gate is green, the implementing agent pushes
`feature/horizon-browser`. The macOS tester must:

1. Fetch `origin/feature/horizon-browser` into an isolated clean worktree and
   record `git rev-parse HEAD` before building.
2. Verify the fetched SHA is still the branch head immediately before smoke;
   never test a stale local branch or a different checkout.
3. Run the macOS Chromium and Firefox lanes, then the full Safari lane below,
   including classic-only fallback, optional BiDi negotiation, one-session
   arbitration, adaptive screenshot latency, normal-window isolation, and
   exact-child cleanup.
4. Fix only in-scope findings on a normally named local branch. Before pushing
   a fix to `feature/horizon-browser`, fetch again and require a regular
   fast-forward push; never force-push over a newer head.
5. Report every step as pass/fail, list any fix commits, and end the completed
   report with exactly `SMOKE-TEST: DONE`. A behavior-changing macOS fix
   invalidates affected Linux evidence and must be handed back for rerun.

## Scope and pass rule

- Linux: Chromium CDP and Firefox WebDriver BiDi, including visible pixels,
  navigation, input, resize, persistence, recovery, and exact-child cleanup.
- macOS: repeat Chromium and Firefox, then Safari classic WebDriver with
  optional BiDi capability reporting and one-session arbitration.
- Windows: Chromium and Firefox compile/launch follow-up before advertising
  those backends for Windows.
- A lane passes only on the exact candidate head with screenshots from launch
  and post-interaction state, no unexplained error banner, and no surviving
  task-owned process/profile/listener after normal close.
- Safari is not release-supported until its exact-head macOS report is complete.

## Isolation preflight

1. Record `git rev-parse HEAD`, `git status --short`, OS version, display
   backend, browser versions, and driver versions.
2. Create a task-owned temporary root with separate `home`, `config`,
   `profiles`, `state`, `logs`, and `proof` directories.
3. Use a task-owned Xvfb plus lightweight window manager on Linux. Confirm the
   display number is unused before launch. On macOS use a separate candidate
   process and identify its windows by exact PID.
4. Unset `HORIZON` for the candidate. Launch with the temporary HOME/config,
   `--ephemeral` where compatible, and a disposable shell. Do not inherit or
   resume the operator's normal Horizon session.
5. Serve deterministic static, input, scrolling, CSS-animation, canvas, and
   navigation pages on a task-owned loopback port. Do not use public web state
   as correctness evidence.
6. Capture the process tree and listening loopback ports before launch. The
   only new browser/driver processes and listeners must belong to this lane.

## Configuration matrix

Run once per backend with only that backend selected in the isolated config.
Use explicit binary paths when auto-discovery would be ambiguous.

| Backend | Required configuration | Expected delivery |
| --- | --- | --- |
| Chromium | `backend: chromium`, explicit candidate Chromium if needed | `PushJpeg`, BiDi false |
| Firefox | `backend: firefox`, explicit Firefox and geckodriver if needed | `AdaptiveScreenshot`, BiDi true |
| Safari | `backend: safari`, `/usr/bin/safaridriver` unless testing Technology Preview | `AdaptiveScreenshot`, negotiated BiDi reported exactly |

## Baseline launch and visual integrity

For every backend:

1. Launch the exact candidate and record its PID before interacting.
2. Create or restore a workspace containing one Browser panel pointed at the
   deterministic input page.
3. Verify the panel shows a URL strip, back/forward/reload controls, the active
   backend, and a crisp frame that fills the body without stretching,
   clipping, stale borders, or a one-frame placeholder loop.
4. Verify the panel title and URL reflect committed browser state, not merely
   submitted text. Confirm `about:blank` does not become a misleading saved
   URL.
5. Capture and inspect a launch screenshot. Record viewport pixel dimensions
   and frame metrics where available.

## Navigation, input, and focus

1. Before manually resizing the panel, click a deterministic page button and
   wheel-scroll in Firefox. Then switch an unchanged-geometry panel from
   Chromium to Firefox and repeat the click. Both actions must work on the first
   converged Firefox frame; resizing is not an acceptable workaround.
2. Focus a text input inside the page, type ASCII and punctuation, then commit
   a multi-character Unicode/IME string. Verify text appears once and key-up
   does not leak into the URL bar or Horizon shortcuts.
3. Exercise Tab, Shift+Tab, Enter, Escape, arrows, Home/End, Backspace/Delete,
   and one function key handled by the page. Confirm backend capability claims:
   Firefox and Safari must not claim physical-key or clipboard behavior they do
   not preserve.
4. Click, double-click, drag out of the viewport, release outside, wheel both
   axes, and use simultaneous button masks where supported. Verify no stuck
   press after focus loss.
5. On a tall page, verify a vertical scrollbar indicator is visible, drag its
   thumb from top toward the middle, click its track, and wheel once. Confirm
   page scroll position and thumb position change after each operation. Also
   click page content within 32 pixels of the right edge to prove the Chromium
   gutter fast path does not steal content input.
6. Focus the URL bar, submit the second deterministic page, and verify committed
   URL/title/frame. Exercise back, forward, and reload.
7. Submit an invalid/unreachable URL and verify an actionable navigation error
   without losing the last valid frame or queued retry target.
8. Capture and inspect a post-input/navigation screenshot.

## Automation disclosure and fingerprint consistency

Use a deterministic loopback page whose inline script runs before any deferred
or external script. Do not use a public search engine, anti-bot service, or
CAPTCHA as the pass/fail oracle.

1. With `automation_disclosure: minimize_common_signals`, record the active
   capability status. Chromium and Firefox must report
   `CommonSignalsMinimized`; Safari must report `UnsupportedByBackend` rather
   than silently claiming parity.
2. In Chromium, verify the earliest inline script, the current document, and a
   dynamically created same-origin iframe each observe
   `navigator.webdriver == false`. Verify the user agent has no
   `HeadlessChrome` token and Client Hint brands/version data are nonempty and
   coherent with the browser version.
3. Repeat the early/current/iframe checks in Firefox after a fresh session.
   This must work on the first navigation because BiDi preload installation is
   ordered before it.
4. Record, without broadly spoofing, languages, plugins, viewport, screen,
   device scale, and WebGL renderer. Treat contradictions as findings; do not
   turn the test into an expanding fingerprint-evasion list.
5. Confirm a visible click routed through Horizon produces a trusted DOM input
   event on both Linux backends and still changes the page. This validates real
   protocol input, not human origin.
6. Repeat with `automation_disclosure: browser_default` in a focused engine
   test and verify the active status is `BrowserDefault` and engine-managed
   minimization is not installed.
7. Treat any rejected required preload or user-agent command as a startup
   failure. A session must never claim `CommonSignalsMinimized` after a silent
   downgrade.

## Agent steering, user handoff, and audit

Run this lane through the `horizon-browser` MCP server over stdio. MCP is the
only agent-facing browser contract: do not use a browser-control CLI, import
Horizon's manifest helpers, edit a live manifest, or connect to a raw CDP,
BiDi, or WebDriver endpoint. The manifest, result files, and engine endpoints
are private host plumbing and may be inspected only after the lane as
implementation-level security and cleanup evidence.

1. Start the exact candidate as the MCP stdio command and complete
   `initialize`, `notifications/initialized`, and `tools/list` with protocol
   versions `2025-06-18` and `2026-07-28`. Verify the ten public tools have
   structured schemas and neither schemas nor results expose raw endpoints or
   runtime paths.
2. Call `browser_list`, select the live panel by its safe id, and confirm
   `browser_panel` reports the same backend, protocol, URL, title, ownership,
   handoff state, and semantic capabilities. Verify MCP automatically claims
   the panel under the calling Horizon agent's stable identity and keeps the
   lease alive during a long bounded action.
3. Through MCP only, navigate to the deterministic page, wait for its marker,
   take a bounded semantic snapshot, query a CSS selector, click its returned
   ref, fill Unicode text through a fresh ref, scroll, evaluate deterministic
   page state, and exercise reload/back/forward. Verify each tool result and
   the visible page result on Chromium and Firefox. Repeat the same semantic
   sequence in Safari during the macOS lane.
4. Mutate or navigate the document, then try an old ref. It must fail as stale;
   take a new snapshot or query and verify the replacement ref succeeds.
5. Correlate each MCP-returned action id with `queued`, `dispatched`, and
   terminal `completed` or `failed` entries returned by `browser_audit`.
   Verify a failed selector action is reported as an MCP tool error and has a
   corresponding terminal audit entry.
6. Interact with the page as the user, then immediately call an MCP action. It
   must return `WouldBlock`. Repeat with URL submission and browser navigation
   controls, not only page clicks or keys.
7. Call `browser_handoff` with a visible reason. Verify the panel shows the
   paused banner and rejects MCP actions until the user activates **Done — hand
   back to agent**. Poll `browser_list` until `handoff_pending` is false, take a
   fresh snapshot, and resume. Verify the exact request is acknowledged and a
   replacement request cannot be cleared accidentally.
8. Read the journal through `browser_audit`. Verify actor, action id, ordering,
   and status; mode `0600` on Unix; URL user-info, query values, fragments,
   selectors, and script/data payloads redacted; and filled text represented
   only by character count. Confirm the MCP response itself contains no raw
   runtime path.
9. Verify queue bounds, stale-owner rejection, lease expiry, malformed-action
   rejection, and crash/relaunch behavior. A `dispatched` record proves the
   command reached the backend adapter; page-level success still requires the
   corresponding URL/title/frame or error evidence.
10. Close normally and verify the live panel disappears from `browser_list`
   while its append-only audit journal remains available to the user through a
   later live instance of that saved panel.

## Bundled agent discovery

Run after the MCP semantic lane on Linux and repeat on macOS.

1. Launch the exact Horizon candidate with the isolated home and verify it
   installs the bundled `horizon-browser` discovery skill for Codex and Claude
   Code without changing the operator's persistent MCP configuration.
2. Verify the generated Claude plugin descriptor points to the exact Horizon
   executable with only the private `--browser-mcp` transport bootstrap.
3. Create a default Codex agent panel. Inspect its exact task-owned process
   arguments and environment: the launch must add a transient
   `mcp_servers.horizon-browser` stdio registration and a stable
   `HORIZON_BROWSER_ACTOR`, without writing that registration to the user's
   config file. A custom agent command must remain unchanged.
4. From the launched agent, confirm the bundled skill discovers
   `browser_list`, then invokes `browser_snapshot` against the sibling panel.
   The agent must not need to know a manifest path, raw endpoint, or secondary
   CLI command.

## Frame delivery and performance

1. Static page: after the adaptive idle-decay period, Firefox/Safari capture
   request and completion counts must stop increasing. Chromium must do no
   decode/upload work without a CDP repaint.
2. Input latency: collect at least 20 deterministic one-key-to-visible-frame
   samples per backend from the shared `FrameMetrics` command-to-published-frame
   counter. Firefox's gate is median <=100 ms and p95 <=180 ms; use the same
   viewport/page/display when comparing it with Chromium or Safari.
3. Navigation latency: collect at least 20 alternating local navigation-to-frame
   samples against pages with visibly distinct pixels, with the same thresholds.
4. Animation: run CSS and canvas animation for at least three seconds. Adaptive
   capture must remain active, never exceed the 30 fps cap, and decay to zero
   again after returning to a static page. Exercise a local muted looping video
   when the platform supplies a deterministic test asset.
5. Rapid input/scroll and repeated resize: verify at most one adaptive capture
   is in flight, newest demand wins, command coalescing grows instead of an
   unbounded queue, and no old-size or old-navigation frame is published.
6. Hold one panel at a size where the backend initially reports a different
   frame height. Verify viewport commands retry no faster than every 250 ms,
   pointer input remains gated while geometry differs, the frame converges
   without manual resize, and retries cease after convergence. If an exact
   viewport is impossible, verify the finite retry budget prevents a permanent
   repaint/resize loop.
7. Compare one, three, and five panels for bounded memory/thread/process growth.
   Record child RSS/CPU and frame metrics; do not generalize beyond the measured
   browser/version/viewport.
8. For Chromium, inspect renderer/GPU diagnostics as well as the root launch
   arguments. Absence of `--disable-gpu` does not prove hardware acceleration,
   especially under Xvfb, remote desktops, or virtual machines.

## Layout and persistence

1. Resize the panel repeatedly, fit the workspace, enter/exit panel fullscreen,
   zoom the canvas, move the panel partly off-screen, and return. Verify current
   pixels and pointer coordinates at each step.
2. Detach the workspace, move and resize the native window, then reattach. Use
   exact-PID native window assertions and a motion trace for movement-sensitive
   behavior.
3. Save and reload the isolated session. Verify panel kind, backend, committed
   URL, profile identity, geometry, focus, and title restore. A requested URL
   that never committed must not replace the saved committed URL.
4. Switch Chromium -> Firefox -> Chromium from the backend picker. Each old
   exact child must finish before the shared panel profile/session is reused;
   the selected backend and profile state must persist.
5. Permanently close a panel and verify its task-owned persistent profile is
   removed only after the browser releases it. Ordinary app exit must preserve
   profiles used by saved panels.

## Failure and teardown

1. Normal panel close, session switch, and app exit: verify each exact browser
   and driver child exits, the driver thread completes, manifests disappear,
   and no task listener remains.
2. Kill only a task-owned disposable browser child, never a pre-existing one.
   Verify the panel shows Retry, stale pixels clear, and Retry waits for teardown
   before starting a replacement.
3. Break only the task-owned driver connection and verify a bounded error state,
   cleanup, and successful retry without PID reuse assumptions.
4. Close Horizon while navigation/capture/resize is active. Verify the bounded
   shutdown path reaps the exact child and does not publish a late frame.
5. Confirm profile and manifest paths reject traversal-like panel IDs and remain
   under the isolated root with private permissions.

## macOS Safari lane

Run only after Linux Chromium and Firefox have passed on the candidate head.

1. Record macOS, Safari, and `safaridriver --version`. If automation is not
   enabled, stop and ask the operator to perform Apple's explicit one-time
   enablement; Horizon must never run `safaridriver --enable`.
2. Prove the returned session capabilities and whether `webSocketUrl` was
   actually negotiated. If the installed Safari rejects that capability,
   verify one clean retry without it and report classic-only mode.
3. Run all baseline, input, navigation, viewport, performance, persistence, and
   teardown steps in Safari's isolated automation window. Explicitly repeat the
   no-manual-resize click, wheel, visible-scrollbar, thumb-drag, track-click, and
   viewport-convergence regressions in Chromium, Firefox, and Safari before
   treating the macOS lane as complete.
4. Repeat the disclosure-consistency lane in macOS Chromium and Firefox. For
   Safari, verify the active status is `UnsupportedByBackend` under the default
   minimization policy and `BrowserDefault` when explicitly selected; do not
   claim Safari provides an equivalent preload contract.
5. Start a second Safari panel. It must show a safe busy error while the first
   session remains intact. Close the first, wait for exact cleanup, then Retry
   the second successfully.
6. Manually break the task-owned automation glass pane, verify permanent session
   disconnect plus bounded cleanup, then Retry only after cleanup.
7. Before and after the lane, verify no normal Safari window, normal profile,
   history, AutoFill data, cookies, settings, or default browser choice changed.

## Evidence and handoff

- Linux report includes exact SHA, versions, commands, screenshots, metrics,
  steering/audit evidence, process/listener cleanup, and each step as
  pass/fail.
- Before a PR exists, hand off the exact pushed
  `origin/feature/horizon-browser` SHA together with:
  `SMOKE-TEST REQUEST macOS — plan: docs/testing/2026-08-26-horizon-browser-panel-smoke.md — scope: Chromium, Firefox, Safari`.
- The macOS agent checks out that exact remote branch head, fixes only in-scope
  issues, reruns affected lanes, and ends its report with exactly
  `SMOKE-TEST: DONE`. Do not create a PR as part of the smoke handoff.
- Any behavior-changing push invalidates older smoke evidence and requires the
  affected lane again.
