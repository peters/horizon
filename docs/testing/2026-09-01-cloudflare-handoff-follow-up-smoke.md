# Cloudflare handoff compatibility follow-up smoke

Temporary validation plan for issue #347. Execute against the exact pull
request head, then delete this file after the final pass.

## Test matrix

- Linux with current Chromium and Firefox/geckodriver is the decisive lane.
- Chromium and Firefox must both satisfy the protected-site acceptance gate.
- Safari on macOS is best effort because public Safari WebDriver does not
  expose the network metadata used by the bounded challenge detector.
- Use a fresh panel-owned profile for each backend, then repeat the decisive
  protected-site pass once with the same profile and process still running.

## Baseline and identity

1. Build and launch the exact candidate Horizon head with an isolated
   temporary Horizon home and default browser configuration.
2. Create one visible browser panel for Chromium and one for Firefox.
3. Open a local identity fixture that records the earliest and current
   `navigator.webdriver`, User-Agent, Client Hints where supported, cookies,
   and trusted-click state.
4. Confirm the default common-signal policy remains active and its existing
   User-Agent, Client Hint, and WebDriver behavior is unchanged.
5. In visible Chromium, confirm `navigator.webdriver` is false and the process
   uses a reserved nonzero loopback DevTools port.
6. Confirm the page remains interactive, screenshots or screencasts update,
   URL and title state converge, and a visible click remains trusted.
7. Start and stop explicit network capture on Chromium and Firefox, then
   navigate again. Confirm stopping capture does not disable the internal
   response observer or leak response headers into the action audit.

## DevTools port retry ownership

1. Run the deterministic
   `devtools_port_retry_requires_the_conflicted_child_to_be_reaped` regression.
2. Exercise the existing Chromium port-conflict fixture and confirm a
   successfully reaped conflicted child permits a bounded retry with a fresh
   reserved port.
3. Confirm the failed-reap decision aborts before a replacement launch can
   register and that the exact prior child handle remains available to the
   shutdown path.
4. Close the test panel and confirm no candidate-owned Chromium parent or
   helper process survives.

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
   first challenge. Confirm successful navigation resets the detector and the
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
   profile directory, IP path, and configured disclosure identity.
5. The protected destination must load and remain usable after a reload in the
   same session. Record backend versions and outcomes without publishing
   cookies, tokens, IP addresses, booking details, or capture paths.
6. A repeated challenge with a bounded explicit rejection is a safe failure,
   not a protected-site pass.
7. For Safari, record the outcome without claiming Chromium or Firefox parity.

## Persistence and visual regression

1. Confirm a legacy configuration without `automation_disclosure` resolves to
   `minimize_common_signals`.
2. Configure `browser_default`, relaunch Chromium and Firefox, and confirm the
   browsers retain their native automation-disclosure behavior.
3. Close and restore a session containing browser panels. Confirm backend and
   profile-root persistence are unchanged and no challenge state survives a
   new browser process.
4. Capture screenshots after launch, handoff, hand-back, and destination
   success or bounded rejection.
5. Resize each panel and verify frames, address bar, handoff banner, error
   text, scrollbar, focus, and pointer mapping remain correct.
6. Confirm ordinary 401/403 responses, subframe challenges, and successful
   documents do not trigger the detector.
7. Close every test panel and verify its process tree and temporary profile are
   removed through normal teardown.
8. Run the full repository validation matrix on the exact final head after all
   smoke-driven fixes.
