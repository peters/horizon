# Ended browser panel cleanup smoke

Use an isolated Linux desktop and private Horizon state, viewed live through a
task-owned native VNC Device panel in the current workspace. Follow
`scripts/device-smoke/README.md`; never manipulate the developer's live panels.
Freeze the candidate binary and record its hash and actual application PID.
Use only synthetic URLs and names. Keep screenshots and video private.

## Automated checks

- Run the `browser_cleanup` regression tests. Stopped and failed browser panels
  must leave the board, workspace membership, render caches and saved state.
- Hidden and visible live browsers, browsers still starting, and other panel
  kinds must remain. A navigation failure with a Ready backend must remain.
- Pending browser creates must publish their original typed failure before
  cleanup. Browser teardown and unresolved remote allocations stay tracked.
- Restore a runtime snapshot with ended remote browser panels. On the first
  host maintenance tick they must disappear, without allocating new sessions.
- Closing the focused/fullscreen browser must clear fullscreen and preserve a
  usable remaining panel. Repeating cleanup must be harmless.

## Live native UI

1. Start the frozen candidate on a fixture-owned display with a heartbeat
   terminal, an editor, and saved remote browser entries (synthetic target names;
   no remote credentials). Verify no remote allocation is requested.
2. Establish the native viewer and save three inspections two seconds apart:
   connected, image received/displayed and advancing frame sequence.
3. Record the fixture desktop before launching the scenario. Confirm the ended
   browser rows and canvas panels vanish, while the terminal/editor remain.
4. Add a local browser whose configured executable is missing in this private
   fixture. Confirm the failed panel disappears instead of leaving a WEB row.
5. Exercise sidebar focus, Fit and window resize through the fixture device
   target. Capture screenshots after launch and resize/Fit; confirm layout and
   remaining panel interaction work. Confirm another panel kind is not removed.
6. Close the exact candidate normally and relaunch its saved private session.
   Verify ended entries do not return. Do not edit running private state files.
7. Finalize the video and decode frames from before/during/after the flow. Record
   candidate hash, scenario and results. Close the task-owned viewer and fixture;
   verify application/children exited and device target expired.

Browser-page interaction, if used, must use public Horizon `browser_*` MCP tools.
No real provider allocation is needed for this cleanup scenario. Unit coverage
verifies remote holds survive teardown; do not claim a live provider release.
