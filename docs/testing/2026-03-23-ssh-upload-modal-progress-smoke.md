# SSH Upload Modal + Progress Smoke Plan

Use this checklist to validate the March 23, 2026 SSH upload fix that:

1. keeps the upload dialog above workspace chrome and panel surfaces
2. reports live SSH upload progress instead of jumping from 0% to 100%

## Goal

Verify that drag-and-drop uploads onto SSH panels:

1. open a modal that visually sits above workspace toolbars, panel titlebars, and terminal content
2. show incremental byte progress during SSH uploads
3. cancel cleanly without leaving the UI or remote destination in a bad state
4. still handle Taildrop, remembered destinations, errors, and detached workspaces correctly

## Recommended Binary

Use the debug binary unless you are explicitly validating release-only behavior:

```bash
cargo build -p horizon-ui
target/debug/horizon --new-session
```

## Suggested Test Setup

Prepare:

1. One SSH target that already accepts non-interactive `ssh` commands from this machine.
2. One writable remote directory and one read-only or invalid directory path for failure checks.
3. Three local files:
   - a tiny text file
   - a medium file around 25-100 MB
   - a larger file around 250 MB or larger so progress remains visible for several seconds
4. If possible, one detached workspace scenario and one Taildrop-capable host.

Useful file-generation commands:

```bash
mkdir -p /tmp/horizon-upload-smoke
printf 'hello from horizon\n' > /tmp/horizon-upload-smoke/tiny.txt
dd if=/dev/zero of=/tmp/horizon-upload-smoke/medium.bin bs=1M count=64
dd if=/dev/zero of=/tmp/horizon-upload-smoke/large.bin bs=1M count=512
```

Optional artifact capture:

```bash
mkdir -p /tmp/horizon-upload-artifacts
```

If the progress motion is subtle, record a short video instead of relying on still images alone.

## Baseline Launch

1. Launch Horizon from a clean temporary HOME or another clean runtime state.
2. Verify the main window appears and the default workspace is usable.
3. Capture a launch screenshot.
4. Resize the window smaller, then larger again.
5. Use `Fit Workspace`.
6. Capture a post-resize screenshot.
7. Drop a file onto a local shell panel and verify Horizon still pastes the local path instead of opening the upload modal.

## Primary Modal Stacking Check

1. Open an SSH panel inside the root window.
2. Arrange the canvas so panel chrome and workspace controls are visually near the drop target.
3. Drop `tiny.txt` onto the SSH terminal body.
4. Verify the upload modal appears in the same window that received the drop.
5. Verify the dimmed backdrop covers panel and workspace surfaces.
6. Verify no workspace layout toolbar, panel titlebar, terminal glyphs, or floating HUD appears above the modal card.
7. Capture a screenshot showing the modal fully above overlapping panel chrome.

## SSH Progress Check

1. Drop `large.bin` onto the same SSH panel.
2. Wait for preparation to finish and confirm the destination field is populated.
3. Start the SSH upload.
4. Verify the progress bar advances before the upload completes.
5. Verify the transferred byte text increases during the transfer.
6. Verify the current file label remains stable and readable while bytes advance.
7. Capture either:
   - a short video of the bar advancing, or
   - multiple screenshots that clearly show increasing byte counts
8. Confirm the final state reports success.
9. Confirm the remote file exists in the chosen directory.
10. Confirm the remote file size matches the local file size.

Suggested remote verification:

```bash
ssh <target> "stat -c '%n %s' <remote-path>"
```

## Cancellation Check

1. Start another upload of `large.bin`.
2. Wait until the byte counter has visibly advanced beyond zero.
3. Click `Cancel Upload`.
4. Verify the modal reports cancellation instead of success.
5. Verify Horizon remains responsive and the SSH panel stays connected.
6. Verify a retry from the same panel can start immediately afterward.
7. If the remote host is Unix-like, verify no obvious temporary partial file remains in the destination directory.

## Failure Handling

1. Drop `tiny.txt` again.
2. Change the destination to a non-writable or invalid path.
3. Start the upload.
4. Verify the modal reports the failure clearly.
5. Verify the modal can return to the ready state.
6. Retry with a valid destination and verify the upload succeeds.

## Remembered Destination

1. Successfully upload a file to a non-default destination.
2. Close the modal.
3. Drop another file onto the same SSH panel.
4. Verify the previously successful destination is prefilled.

## Detached Workspace Coverage

1. Detach the workspace that contains an SSH panel.
2. In the detached window, drop `medium.bin` onto the SSH panel.
3. Verify the modal opens in the detached window, not the root window.
4. Verify the modal still renders above detached-window panel chrome and workspace UI.
5. Start the upload and verify the progress bar advances before completion.
6. Capture a screenshot of the detached-window modal while the upload is active.

## Taildrop Regression Check

Run this only if Taildrop is available for the target host.

1. Drop `tiny.txt` onto the SSH panel.
2. Verify `Taildrop (...)` still appears as a selectable transport.
3. Switch between `SSH` and `Taildrop` and confirm the UI updates correctly.
4. Start a Taildrop transfer.
5. Verify it still reaches a success or explicit failure state.
6. Confirm the file arrives through the remote device’s Taildrop inbox.

## Overlay Regression Check

1. After closing the upload modal, open the command palette.
2. Verify it still renders correctly.
3. Open the remote hosts overlay.
4. Verify it still renders correctly.
5. Confirm these overlays still appear above normal board content and do not leave stale backdrops behind.

## Record Results

Capture at minimum:

1. Launch screenshot.
2. Post-resize screenshot.
3. Root-window modal stacking screenshot.
4. Active SSH upload progress artifact.
5. Success-state screenshot.
6. Detached-window modal screenshot if that path was exercised.

Record:

1. OS and desktop environment.
2. Horizon commit SHA.
3. SSH target type: local, remote, jump-host, or Tailscale-backed.
4. Whether detached workspace coverage passed.
5. Whether Taildrop was available.
