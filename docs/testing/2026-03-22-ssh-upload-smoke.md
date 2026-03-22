# SSH Upload Smoke Plan (macOS)

Use this checklist to manually validate the SSH drag-and-drop upload flow on macOS.

## Goal

Verify that dropping local files onto an SSH panel:

- opens the upload flow instead of pasting local paths
- uploads successfully over SSH to the selected remote destination
- shows a visible success or failure state
- keeps normal local-terminal and non-terminal drag/drop behavior intact

If Tailscale Taildrop is available for the target host, validate that path too.

## Recommended Binary

Unless release-specific behavior is what you are testing, use the debug build for faster iteration:

```bash
cargo build -p horizon-ui
target/debug/horizon --new-session
```

## Suggested macOS Setup

Choose one of these SSH targets:

1. Another machine you can SSH into normally.
2. A local macOS loopback target if you have an SSH server enabled.
3. A Tailscale SSH target if you also want to validate Taildrop detection.

Prepare:

1. One small text file you can identify easily after upload.
2. One larger file so progress UI is visible for longer.
3. A writable destination directory on the remote host.
4. A second remote directory for destination-browser navigation checks.

If you want artifacts:

```bash
mkdir -p /tmp/horizon-smoke-artifacts
screencapture -x /tmp/horizon-smoke-artifacts/launch.png
```

For motion-sensitive regressions, capture a short video instead of relying only on still screenshots:

```bash
screencapture -V 10 /tmp/horizon-smoke-artifacts/motion.mov
```

## Baseline

1. Launch Horizon in a clean temporary HOME or otherwise clean state.
2. Verify the app window appears and the default workspace renders correctly.
3. Capture a launch screenshot.
4. Resize the window smaller, then larger again.
5. Use `Fit Workspace` once after resizing.
6. Capture a post-resize screenshot.
7. Open a normal local shell panel and drop a file onto it.
8. Verify the local shell still receives pasted local file paths rather than opening the upload modal.

## SSH Upload Primary Flow

1. Open an SSH panel that connects successfully to your test target.
2. Drop a small local file onto the SSH panel body.
3. Verify an upload modal appears instead of terminal path paste.
4. Wait for preparation to finish.
5. Verify the destination field is prefilled.
6. Verify SSH upload is available.
7. Open the remote directory browser.
8. Navigate into a child directory, then back to parent.
9. Choose the intended destination directory.
10. Start the upload.
11. Verify the modal shows progress or an in-progress state.
12. Verify the modal reaches a success state.
13. Confirm the uploaded file exists on the remote host in the selected directory.
14. Confirm the file contents match the local source file.

## Progress And Cancellation

1. Drop a larger file onto the same SSH panel.
2. Start the upload.
3. Verify the modal shows progress details, current file information, or completed/total counts.
4. If the upload lasts long enough, cancel it mid-transfer.
5. Verify the modal reports cancellation cleanly.
6. Retry the same upload and verify it can complete successfully afterward.

## Failure Handling

1. Drop a file onto the SSH panel again.
2. Change the destination to an invalid or non-writable remote path.
3. Start the upload.
4. Verify the modal reports the failure clearly.
5. Verify you can return to the ready state and try again with a valid destination.
6. Verify the SSH panel itself remains connected and usable after the failure.

## Persistence / Remembered Destination

1. Complete an SSH upload to a non-default destination.
2. Close the modal.
3. Drop another file onto the same SSH panel.
4. Verify the previously successful destination is reused.

## Taildrop Path (if available)

Run this only if the SSH host is also a Tailscale device and Taildrop is enabled.

1. Drop a file onto the SSH panel.
2. Verify Taildrop appears as an available transfer method.
3. Select Taildrop.
4. Verify the destination picker is replaced by Taildrop-specific messaging.
5. Start the transfer.
6. Verify the modal reaches a success state.
7. Confirm the file arrives through the device’s Taildrop inbox.

## Regression Checks

1. Drop a file onto a non-terminal area and verify normal editor/open behavior is unchanged.
2. Open the command palette and remote hosts overlay after an upload flow and verify they still behave normally.
3. Verify closing the upload modal does not close the SSH panel.
4. If you use detached workspaces, repeat one SSH upload there and verify the modal appears in the same window that received the drop.

## Record Results

Capture at least:

1. Launch screenshot.
2. Post-resize screenshot.
3. SSH upload modal screenshot before starting upload.
4. Success-state screenshot after upload completes.
5. Failure-state screenshot if you exercised the invalid-destination path.

For each run, note:

1. macOS version.
2. Horizon commit SHA.
3. Whether the SSH target was local, remote, or Tailscale-backed.
4. Whether Taildrop was available.
