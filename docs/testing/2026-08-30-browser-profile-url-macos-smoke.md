# macOS Smoke Plan: Browser Profiles and Address Bar

## Purpose

Validate the browser-profile startup and address-bar behavior changed by this
branch on macOS. This is a temporary delta plan for the pull request. Use it
with the durable [browser panel validation gate](browser-panel-gate.md), which
defines the common MCP, interaction, lifecycle, cleanup, and evidence rules.

The required result is one report covering Chromium, Firefox, and Safari on the
same exact pull-request head. Do not reuse, automate, signal, or close an
existing Horizon or browser process.

## Required Environment

- A logged-in macOS GUI session with Xcode Command Line Tools.
- Chromium or Google Chrome, Firefox, geckodriver, Safari, and safaridriver.
- Accessibility permission for native input and Screen Recording permission
  for exact-window screenshots.
- A clean checkout of the pull-request branch.

Record the following before testing:

```bash
git status --short
git rev-parse HEAD
sw_vers
```

Also record the discovered browser and driver paths and their versions. Do not
pass explicit browser commands for the first run; discovery itself is part of
the test.

## Build and Repository Gate

Run G0 from the permanent browser gate on the exact clean commit, including
the version-sync check, both workspace test tiers, and all three Clippy tiers.
Then build the debug binary:

```bash
cargo build
```

If any fix is pushed, restart this plan from the new exact head. Evidence from
an older behavior-changing head is invalid.

## Reusable Browser Runs

Run the reusable gate with its default isolated root for Chromium and Firefox:

```bash
python3 scripts/browser-smoke/run.py \
  --backend chromium \
  --horizon target/debug/horizon

python3 scripts/browser-smoke/run.py \
  --backend firefox \
  --horizon target/debug/horizon
```

For each run:

1. Drive and capture only the exact candidate PID printed by the runner.
2. Complete the visible handoff and hand-back step; do not use
   `--skip-handoff`.
3. Before resizing, verify the first frame, click, trusted double-click, wheel,
   scrollbar, and viewport size work without a manual resize workaround.
4. Verify the URL remains unchanged during wheel, scrollbar, resize, Fit, and
   canvas movement.
5. Toggle the agent-created panel hidden, visible, hidden, and visible while
   retaining control and page state.
6. Close the exact Horizon window normally when prompted and require a clean
   runner result with no task-owned browser, driver, listener, manifest, or
   disposable profile left behind.

Save and inspect launch and post-interaction screenshots for both runs.

## Default Profile Startup

Use a fresh isolated runner root for each case and pass
`--use-default-profile-root` to `scripts/browser-smoke/run.py`. The flag is
required here: without it the runner configures an explicit `profile_root` and
does not exercise platform-default profile discovery.

### Chromium

1. Confirm Chromium starts through macOS application discovery with no profile
   lock, singleton, or permission error.
2. Record the task-owned profile path from the process arguments or runner
   evidence. It must remain inside the runner's isolated macOS home/profile
   root; the Linux Snap fallback `~/Horizon/browser-profiles` must not be used.
3. Switch the same panel to Firefox and back to Chromium. Both backends must
   start with separate backend subdirectories under one panel profile root.
4. Retry Chromium once after a normal backend stop and confirm the prior
   task-owned process is gone before the replacement becomes ready.

### Firefox

1. Confirm Firefox starts through application discovery with no profile or
   driver error.
2. Switch to Chromium and back to Firefox and verify the first converged frame
   works without resizing.
3. Confirm switching backends does not delete the other backend's profile
   while the panel still exists.

### Permanent Panel Cleanup

Create a second disposable panel, start both Chromium and Firefox in it, and
then permanently close only that panel. After both child processes exit,
confirm its shared panel profile root and both backend subdirectories are
removed. The first panel and unrelated browser state must remain untouched.

## Address-Bar Display Contract

Run these checks on Chromium and Firefox, then repeat the focused display and
submission checks on Safari. The behavior is shared UI code, so all three
backends must agree.

1. Submit `example.com` with no scheme. Require navigation to HTTPS.
2. Move focus into the page. The unfocused bar must display `example.com`:
   no `https://` prefix and no redundant trailing root slash.
3. Click the address bar. It must reveal and select the full canonical URL
   `https://example.com/` so typing replaces the whole target.
4. Return focus to the page without navigating. It must compact to
   `example.com` again.
5. Navigate to an HTTPS URL containing a path, query, and fragment. The
   unfocused form must omit only the HTTPS scheme while retaining the path,
   query, and fragment; focus must reveal the full canonical URL. Include
   `https://example.com?q=/` and `https://example.com#/`; their trailing slash
   must remain part of the query or fragment.
6. Start a task-owned loopback HTTP fixture and submit its explicit
   `http://127.0.0.1:<port>/` URL. The bar must retain the `http://` prefix both
   focused and unfocused. Do not compact an insecure URL into an ambiguous
   hostname.
7. Submit a hostname without a scheme again and confirm HTTPS remains the
   default after the explicit HTTP navigation.
8. Exercise Back, Forward, and Reload. The displayed value must track the
   committed page, and merely focusing or defocusing the bar must not cause a
   reload or add a history entry.
9. Type the hostless secure targets `https://user@/` and `https://:443/` and
   confirm neither is misleadingly compacted. The existing typed
   error/last-valid-page behavior must remain bounded and recoverable.
10. Scroll, drag the page scrollbar, resize the panel, and resize the native
    Horizon window. The committed URL and compact/full presentation must remain
    stable throughout.

For Safari, also record the capabilities and whether `webSocketUrl`
negotiated. A clean classic-only fallback is acceptable. If Safari automation
is not enabled, report the one-time operator prerequisite and do not run
`safaridriver --enable` from Horizon or the test harness.

## Persistence and Relaunch

Using one runner-created non-ephemeral root:

1. Leave a Chromium panel on an HTTPS path and close Horizon normally.
2. Relaunch with the same printed root and `--skip-mcp`.
3. Confirm the backend, canonical committed URL, compact unfocused display,
   title, panel geometry, and profile identity restore.
4. Focus the address bar and confirm the full canonical URL is selected.
5. Switch to Firefox, close normally, and relaunch once more. Confirm Firefox
   restores and Chromium can still be selected without a profile collision.

## Failure and Cleanup Checks

1. Break only a task-owned disposable Chromium child and exercise Retry.
   Confirm stale pixels clear, the last valid HTTPS target is retained, and the
   new child uses the same isolated panel profile without a singleton error.
2. Repeat the bounded disconnect/Retry check for a task-owned Firefox or
   geckodriver child.
3. Close each exact Horizon window through the normal macOS window path.
4. Verify no task-owned browser/driver process, listener, live manifest, or
   temporary profile survives. Do not use broad process-name termination.

## Required Evidence and Report

The pull-request comment must include:

- exact tested commit SHA and clean-checkout result;
- macOS, browser, and driver versions;
- G0 result;
- Chromium reusable-gate result and artifact root;
- Firefox reusable-gate result and artifact root;
- Safari focused address-bar and lifecycle result;
- screenshots proving compact HTTPS, focused full HTTPS, and explicit HTTP;
- profile locations stated relative to the isolated root, with no private host
  path published;
- backend-switch, persistence, Retry, and cleanup results;
- every fix pushed, remaining limitation, or skipped prerequisite.

Use this exact report shape and finish with the required marker:

```text
SMOKE-TEST REPORT (macOS)
- exact head and clean checkout: pass | fail — <SHA and note>
- repository gate: pass | fail — <note>
- Chromium profile/startup and reusable gate: pass | fail — <note>
- Firefox profile/startup and reusable gate: pass | fail — <note>
- Safari shared address-bar check: pass | fail — <note>
- HTTPS compact/full display and explicit HTTP: pass | fail — <note>
- backend switch, persistence, and Retry: pass | fail — <note>
- exact-process cleanup: pass | fail — <note>
Summary: <fixes pushed, remaining limitations, and evidence locations>
SMOKE-TEST: DONE
```

Do not post `SMOKE-TEST: DONE` until all required lanes have reached a terminal
result. A failure still needs precise evidence and must not be reported as a
passing smoke.
