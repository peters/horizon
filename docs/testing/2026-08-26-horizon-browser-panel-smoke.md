# Horizon browser panel Linux execution record

Historical exact-head evidence for the first-party `horizon-browser` engine
and Horizon panel integration. This record is intentionally dated and is not a
current test procedure.

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
- Finding fixed and rerun: `browser_act` now accepts `count: 1..=3` for semantic
  clicks. Firefox batches the pointer move plus every down/up pair into one
  WebDriver Actions command, preserving Firefox's click tracker without DOM
  event injection or single-click latency; Chromium sends the corresponding
  CDP sequence with click counts one then two. On the rebuilt candidate,
  Firefox 154/geckodriver 0.35 and Chromium 151 each reported
  `clicks=0 trusted=true double=1 globalUp=2` after an MCP-only `count: 2`
  action. Both audits recorded `queued`, `dispatched`, and `completed` with
  `count=2`, and both exact process trees and live manifests cleaned up after
  the normal window-manager close path. Consecutive physical UI clicks remain
  streamed as separate commands, so this fix is deliberately the atomic agent
  action rather than a delay added to every user single click.
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

## Reusable gate

Future validation uses [`browser-panel-gate.md`](browser-panel-gate.md) and the
committed fixtures and MCP harness under `scripts/browser-smoke/`. Preserve this
file as evidence for the recorded Linux run; do not copy its former one-off
checklist into a new temporary plan.
