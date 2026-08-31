# Cloudflare handoff compatibility smoke

Temporary validation plan for issue #347. Execute against the exact commit that
will be pushed, then delete this file after the final pass.

## Test matrix

- macOS 26.6.2 with current Google Chrome, Firefox/geckodriver, and Safari.
- Chromium and Firefox are decisive compatibility lanes.
- Safari is a best-effort lane because public Safari WebDriver does not expose
  the network metadata used by the bounded challenge detector.
- Use a fresh panel-owned profile for each backend, then repeat the decisive
  protected-site pass once with the same profile/session still running.

## Baseline and identity

1. Launch the exact candidate Horizon binary with an isolated temporary
   Horizon home and default browser configuration.
2. Create one visible browser panel per available backend.
3. Open a local identity fixture that records the earliest and current
   `navigator.webdriver`, User-Agent, Client Hints where supported, cookies,
   and trusted-click state.
4. Confirm the default common-signal policy remains active and its existing
   User-Agent, Client Hint, and WebDriver behavior is unchanged.
5. In visible Chromium, confirm `navigator.webdriver` is false and the process
   uses a reserved nonzero loopback DevTools port.
6. Confirm the page remains interactive, screenshots/screencasts update, URL
   and title state converge, and a visible click remains trusted.
7. Start and stop explicit network capture on Chromium and Firefox, then
   navigate again. Confirm stopping capture does not disable the internal
   response-metadata observer or leak response headers into the action audit.

## Deterministic repeated-challenge flow

1. Serve a local top-level HTML response with status 403 and
   `cf-mitigated: challenge`; include no private tokens or identifying data.
2. Navigate Chromium and Firefox to that exact URL and confirm one challenge
   alone does not produce an error.
3. Request handoff, wait for the visible handoff banner, and click
   **Done — hand back to agent** without changing panels or profiles.
4. Reload the same URL so the documented challenge marker is returned again.
5. Confirm exactly one explicit navigation error appears after the bounded
   delay: Cloudflare rejected the completed verification and presented the
   same challenge again.
6. Reload repeatedly and confirm the error is not emitted repeatedly.
7. Navigate to a successful status-200 document, repeat handoff, then load a
   first challenge. Confirm successful navigation reset the detector and the
   first later challenge does not report a false loop.
8. Navigate to a different challenged URL after handoff and confirm it is not
   classified as rejection of the original challenge.

## Protected-site handoff flow

1. In each backend, follow the Norwegian low-fare-calendar path to the same
   protected booking destination described in issue #347.
2. If Cloudflare presents a human check, request handoff and let the user
   complete it in the original panel. Do not create a helper panel, change the
   network path, copy cookies, or inspect challenge tokens.
3. Hand back only after the challenge UI reports success.
4. Confirm Chromium and Firefox keep the exact process, panel identity,
   profile directory, IP path, and configured disclosure identity across
   handoff.
5. Pass criterion: the protected destination loads and remains usable after a
   reload in the same session. Record the backend version and outcome without
   publishing cookies, tokens, IP addresses, booking details, or capture paths.
6. If the destination is challenged again, confirm the bounded explicit
   rejection appears and no second user handoff is treated as a successful
   resolution. This is a safe failure, not a protected-site pass.
7. Safari: record whether the protected destination loads. If it loops, record
   the limitation without claiming Chromium/Firefox-equivalent diagnostics.

## Persistence and migration

1. Deserialize a legacy browser configuration with no
   `automation_disclosure` field and confirm it resolves to
   `minimize_common_signals`.
2. Explicitly configure `browser_default`, relaunch Chromium and Firefox, and
   confirm the unmodified browser behavior remains available.
3. Close and restore a session containing browser panels. Confirm backend and
   profile-root persistence are unchanged and no challenge state survives a
   new browser process.
4. Close every test panel and verify its process tree and temporary profile are
   removed through normal teardown.

## Visual and regression checks

1. Capture a screenshot after launch, after handoff begins, after hand-back,
   and after either destination success or bounded rejection.
2. Resize each panel and verify frames, address bar, handoff banner, error text,
   scrollbar, focus, and pointer mapping remain correct.
3. Verify ordinary 401/403 responses without `cf-mitigated: challenge`,
   subframe challenges, and successful documents do not trigger the detector.
4. Confirm explicit HTTP/WebSocket capture still records one response-start
   event per Firefox request and remains bounded after repeated start/stop.
5. Run the repository's full pre-push validation matrix on the same final
   commit after all smoke-driven fixes.
