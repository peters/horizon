# Horizon browser panel smoke test

Temporary exact-head validation plan for the first-party `horizon-browser`
engine and Horizon panel integration. Record the checked-out commit before each
lane. Never reuse, signal, close, or inspect a pre-existing Horizon or normal
browser session.

## Linux execution record (2026-08-26)

- Host: Ubuntu 24.04.4 LTS, Linux 7.0.0-30-generic x86_64, isolated Xvfb.
- Browsers: Chromium 151.0.7922.108 snap; Firefox 154.0; geckodriver 0.35.0.
- Chromium CDP: pass for visible push frames, committed URL/title, Fit/resize,
  Unicode agent input, claim/lease, user preemption, handoff/handback, redacted
  mode-`0600` audit, and exact process/manifest cleanup.
- Firefox WebDriver BiDi: pass for visible adaptive screenshots, committed
  URL/title, Fit/resize, Unicode agent input, claim/lease, user preemption,
  handoff/handback, redacted mode-`0600` audit, and exact
  Horizon-to-geckodriver-to-Firefox cleanup.
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
- The complete repository validation matrix, package dry-run, documentation,
  and cross-target checks are blocking before the final branch head is pushed
  for macOS. The macOS tester must use the final remote SHA, not an earlier
  push made before the persistence finding.
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

1. Focus a text input inside the page, type ASCII and punctuation, then commit
   a multi-character Unicode/IME string. Verify text appears once and key-up
   does not leak into the URL bar or Horizon shortcuts.
2. Exercise Tab, Shift+Tab, Enter, Escape, arrows, Home/End, Backspace/Delete,
   and one function key handled by the page. Confirm backend capability claims:
   Firefox and Safari must not claim physical-key or clipboard behavior they do
   not preserve.
3. Click, double-click, drag out of the viewport, release outside, wheel both
   axes, and use simultaneous button masks where supported. Verify no stuck
   press after focus loss.
4. Focus the URL bar, submit the second deterministic page, and verify committed
   URL/title/frame. Exercise back, forward, and reload.
5. Submit an invalid/unreachable URL and verify an actionable navigation error
   without losing the last valid frame or queued retry target.
6. Capture and inspect a post-input/navigation screenshot.

## Agent steering, user handoff, and audit

Run this lane through the public Horizon manifest helpers, never by editing a
live manifest directly.

1. Discover the live panel, claim it with a unique agent name, and refresh the
   lease. Verify another name cannot take a fresh lease and the owner chip
   identifies the current agent.
2. Queue backend-neutral navigate, back/forward/reload, pointer, and text-input
   actions. Correlate each returned action id with a `queued` and `dispatched`
   audit record and verify the visible page result on Chromium and Firefox.
   Repeat on Safari during the macOS lane.
3. Interact with the page as the user, then immediately try to queue an agent
   action. It must return `WouldBlock`. Repeat with URL submission and browser
   navigation controls, not only page clicks or keys.
4. Request a handoff with a visible reason. Verify the panel shows the paused
   banner and rejects queued agent work until the user activates **Done — hand
   back to agent**. Verify the exact request is acknowledged and a replacement
   request cannot be cleared accidentally.
5. Read the private JSONL journal through the public audit reader. Verify
   actor, action id, ordering, and status; mode `0600` on Unix; URL user-info,
   query values, fragments, and script/data payloads redacted; and typed or
   pasted text represented only by character count.
6. Verify queue bounds, stale-owner rejection, lease expiry, malformed-action
   rejection, and crash/relaunch behavior. A `dispatched` record proves the
   command reached the backend adapter; page-level success still requires the
   corresponding URL/title/frame or error evidence.
7. Close normally and verify the live manifest disappears while its append-only
   audit journal remains available to the user.

## Frame delivery and performance

1. Static page: after the adaptive idle-decay period, Firefox/Safari capture
   request and completion counts must stop increasing. Chromium must do no
   decode/upload work without a CDP repaint.
2. Input latency: collect at least 20 deterministic one-key-to-visible-frame
   samples. Firefox's gate is median <=100 ms and p95 <=180 ms.
3. Navigation latency: collect at least 20 alternating local navigation-to-frame
   samples against pages with visibly distinct pixels, with the same thresholds.
4. Animation: run CSS and canvas animation for at least three seconds. Adaptive
   capture must remain active, never exceed the 30 fps cap, and decay to zero
   again after returning to a static page. Exercise a local muted looping video
   when the platform supplies a deterministic test asset.
5. Rapid input/scroll and repeated resize: verify at most one adaptive capture
   is in flight, newest demand wins, command coalescing grows instead of an
   unbounded queue, and no old-size or old-navigation frame is published.
6. Compare one, three, and five panels for bounded memory/thread/process growth.
   Record child RSS/CPU and frame metrics; do not generalize beyond the measured
   browser/version/viewport.

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
   teardown steps in Safari's isolated automation window.
4. Start a second Safari panel. It must show a safe busy error while the first
   session remains intact. Close the first, wait for exact cleanup, then Retry
   the second successfully.
5. Manually break the task-owned automation glass pane, verify permanent session
   disconnect plus bounded cleanup, then Retry only after cleanup.
6. Before and after the lane, verify no normal Safari window, normal profile,
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
