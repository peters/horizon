# Temporary smoke plan: cloud card header strip

Temporary validation artifact for the production cloud card redesign. Delete it
after the UI validation pass.

## Setup

- Build `target/debug/horizon` from the branch, copy it to a task-owned
  directory and record its SHA-256.
- Run it on an isolated Xvfb desktop with a private `HOME`, viewed live through a
  native VNC Device panel (`scripts/device-smoke/README.md`).
- A saved session (not `--ephemeral`) with cloud settings; a scratch repository
  with `.horizon/cloud.yml` (provider `runpod`, one CPU profile).

## Baseline

1. A new production cloud shows its title, then `RunPod / <profile> · <place> ·
   <vCPU> · <GB>` under it, and no "Empty" or cost badge.
2. The status line reads `Not deployed · Nothing is allocated or billed yet`, the
   header offers `Deploy cloud`, spend reads `No charges yet`, and the stage track
   under the header is empty.
3. The body shows the step list (all pending) and an empty Output with
   "No output yet." and "Panels open here once the cloud is ready."
4. No "Unavailable", "Show controls", "Hide controls" or separate Activity window
   appears anywhere.

## Deployment

5. Deploy: the status line changes per step (`Validating`, `Building image
   N/M steps`, `Pushing image X / Y · rate`, `Requesting worker`, `Worker
   starting`, `Preparing worktrees`, `Starting sessions`), with `Stage i/8 · time
   elapsed` on the right and `Cancel` as the main action.
6. The running track segment fills with measured progress and shows a moving
   sheen; finished segments keep their colour.
7. The body's step list opens the running step in place (detail, bar, numbers,
   ETA) and the Output groups lines under step headings with times.
8. Scroll the Output up while lines arrive: the view stays; scroll back to the end
   and it follows again.
9. Wheel over the body or drawer never pans the canvas.

## Failure

10. Make the push fail (unreachable or unauthorized registry). The header reads
    `Push failed · <decisive line> · after <time> — <meaning>`, the failed track
    segment is red and the right side says `Stopped at stage 3/8 · output kept`.
11. The body's failed step shows the cause in red, its meaning, `Retry deploy` and
    `Copy error`; Output pins `Root cause` under the log. Copy error puts the cause
    and summary on the clipboard.
12. `Retry deploy` from the header, the step and Overview all start a new attempt.

## Ready

13. Ready without panels: `Ready · No panels yet · up …`, `Ready in <time> · No
    panels`, `Stop…` as the main action; the hint says Ctrl-double-click adds a
    panel, and the gesture works over the body.
14. Add a terminal: the body gives way to the panel; the status line counts
    terminals; the drawer now offers Overview and Output too.
15. Drawer tabs: Overview (stepper, ready timeline), Output, Machine (profile, size,
    worker id, SSH endpoint, disks), Cost (rate, run, since creation, billed bars,
    run and total lines), Connections (terminals, sessions, desktop viewer, local
    network switch, release devices, companions), Manage (layout, full screen,
    stop, rebuild, resize, delete). Each tab teaser matches its content.
16. `Stop…` opens Manage on the stop confirmation.

## Other states

17. Stopped: `Stopped · Storage kept · billable`, faded track, `Resume worker`.
18. Restored unconfirmed worker: `Needs provider check`, `Check provider`.
19. Deleting: deletion track of three steps; `Cancel` only while releasing devices.
20. Deleted: `Worker deleted`, `Redeploy…` opens Manage on the confirmation.
21. Saved clouds from before this change: sessions move up under the status strip
    once; a manual gap the person made is kept.

## Visual regressions

22. Narrow cloud (548 px): spend drops first, then the connection icons; the main
    action and expand stay; nothing overlaps the close button or the title.
23. Zoom 0.5 and 1.8: header text readable at 1.8, track visible at 0.5.
24. Collapse the cloud: header and track stay; the drawer closes.
25. Full screen: header strip and drawer work the same.
26. Local and design-fixture clouds keep their old header badges and empty text.
