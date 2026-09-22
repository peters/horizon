# Automatic native Device presentation

Retain this regression plan with the Cloud Workspaces MVP. Track implementation,
headless verification and native acceptance separately. No human visibility
confirmation, developer-desktop automation or restart of the active host.

## Candidate and live-view prerequisites

Freeze the application and public MCP executable from the same source. Record
hashes, source manifest, actual child PID, task-owned display, VNC endpoint and
private evidence directory. View that desktop through the existing native
Device panel in the user's workspace. Save three timestamped public inspections
while the fixture's independent heartbeat changes; confirm connected, received,
displayed and advancing uploaded frames. Static content is inconclusive about
freshness. Use the bounded recovery procedure in scripts/device-smoke/README.md.

## Scenarios

1. In the isolated candidate, create a native viewer from a real agent identity
   through public device_panel. The target is a separate task-owned heartbeat
   desktop. No browser viewer, raw browser control or private request-file edits.
2. Inspect before the first image, after decoded frames arrive, and after paint.
   Distinguish decoded frames from texture uploads. Inspection must not consume
   the pending image. Check observation timestamp, connection generation, frame
   ages and presentation reason. Create/Reveal are not live-image proof.
3. Pan the viewer off canvas, then inspect. Sampling must report paused and
   presentation not_rendered without claiming a dead transport. Reveal once;
   verify pan/fit, displayed pixels and advancing frames without a new connection
   generation. Record the flow at 4K and decode representative video frames.
4. Repeat for hidden viewers, collapsed workspaces/cloud groups and fullscreen.
   Reveal may change presentation but must preserve the keyboard-focus panel and
   active workspace, including when they belong to another workspace. Confirm
   subsequent panel creation still uses the previous active workspace.
5. Detach the viewer workspace and move its viewer off its own canvas. Reveal
   must change that detached canvas, preserve the root canvas and issue no OS
   Focus command. Verify actual decoded image presentation and repeat after
   resize/fit. A retained frame or unchanged static desktop is not motion proof.
6. Stop only the task-owned VNC server. Confirm disconnect reporting; use one
   justified reconnect after restoring it. Its generation changes and old pixels
   must not count as a new received/displayed frame. Never loop reconnects.
7. Another agent cannot Reveal, hide, close or reconnect an owned viewer. Other
   workspaces remain inaccessible. A restored viewer requires explicit ownership
   acquisition through Reconnect; Reveal never acquires it implicitly.
8. Legacy host observations lacking diagnostics still deserialize. Unsupported
   Reveal reports a host limitation; it never triggers private-file fallback,
   manual confirmation or replacement of the user's running Horizon.

Run the device protocol, socket, presentation, ownership and detached regressions,
the full repository matrix and independent review. Repeat the affected native
lane after changes. Close only task-owned test viewers/windows and preserve the
user's original Horizon and target processes.

Current status: implemented; device tests and the ten prior matrix lanes passed
(`final-validation-32/` private evidence). Independent source review is clear.
A separate user-authorized ephemeral viewer now proves connected, displayed and
advancing native frames through public MCP, without restarting the original host.
That establishes the viewing prerequisite. The detailed isolated Reveal, detached,
focus-preservation and disconnect scenarios above remain pending. Validation 33
qualifies the later minimum-window correction; do not conflate prior matrix proof
with the new candidate.


Native checkpoint 21 September 09:35 UTC: hidden/off-canvas/fullscreen Reveal,
non-owner mutation refusal, detached off-canvas recovery with root OS focus and
window geometry preserved, disconnection reporting and generation-reset reconnect
pass on frozen candidate `2a1411c736376304964614b7c448e9681c0631526a18c987b0b0d41c13971200`.
Public receipts and inspected 4K recordings are private `development-71/`.
Cloud collapse, another workspace's active selection and restored ownership retain
headless coverage; their native variants are not claimed complete by this pass.
