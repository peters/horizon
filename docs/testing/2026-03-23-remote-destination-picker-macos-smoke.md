# Remote Destination Picker Smoke Plan (macOS)

Use this checklist to validate the new reusable destination picker used by the SSH upload flow on macOS.

## Goal

Verify that SSH upload destination selection now uses the shared picker modal, and that it works correctly for:

- opening from the upload dialog
- keyboard navigation and completion
- mouse selection
- manual path entry
- remembered destinations after a successful upload
- failure recovery without breaking the SSH panel or upload flow

Also confirm that local directory picking for workspace/panel creation still behaves normally after the refactor.

## Recommended Build

Unless you are explicitly validating release packaging, use the debug binary:

```bash
cargo build -p horizon-ui
target/debug/horizon --new-session
```

## Test Setup

Prepare:

1. A macOS machine running Horizon from this branch/commit.
2. One reachable SSH target with a writable directory.
3. One nested remote directory tree with at least:
   `/tmp/horizon-upload-a`
   `/tmp/horizon-upload-a/nested`
4. One small local file and one larger local file.
5. A way to verify the remote filesystem after upload, either an SSH shell or another terminal.

Optional artifacts:

```bash
mkdir -p /tmp/horizon-remote-picker-smoke
screencapture -x /tmp/horizon-remote-picker-smoke/launch.png
```

## Baseline Launch

1. Launch Horizon in a clean or disposable environment.
2. Verify the main window appears and the initial workspace renders normally.
3. Capture a launch screenshot.
4. Resize the window smaller and larger again.
5. Use `Fit Workspace` once.
6. Capture a post-resize screenshot.

## Local Picker Regression

1. Trigger the existing local directory picker by creating a workspace or panel that asks for a directory.
2. Verify the picker opens and still shows local search results.
3. Type a path prefix and confirm results update.
4. Use arrow keys to move selection.
5. Press `Tab` and verify the selected path completes into the input with a trailing slash.
6. Press `Escape` and confirm the picker closes cleanly.

## SSH Upload Picker Primary Flow

1. Open an SSH panel and connect successfully.
2. Drop a local file onto the SSH panel.
3. Wait for the upload modal to reach the ready state.
4. Verify the remote destination field is populated.
5. Click `Browse`.
6. Verify a centered picker modal opens above the upload dialog.
7. Verify the picker heading reads as a remote destination chooser and that the input is focused.
8. Verify the status line shows the currently browsed remote directory.
9. Capture a screenshot of the picker modal.

## Keyboard Navigation And Completion

1. In the picker, type `/tmp/horizon-upload-a/`.
2. Verify child directories are listed.
3. Use arrow keys to highlight `nested`.
4. Press `Tab`.
5. Verify the input becomes `/tmp/horizon-upload-a/nested/`.
6. Verify the result list refreshes for that directory.
7. Press `Enter`.
8. Verify the picker closes and the upload dialog destination field is updated to `/tmp/horizon-upload-a/nested/` or the selected resolved directory path.

## Mouse Selection

1. Reopen the picker from the upload dialog.
2. Type `/tmp/`.
3. Click `horizon-upload-a`.
4. Verify the picker closes immediately and the upload dialog destination field is updated to that selected directory.

## Manual Path Entry

1. Reopen the picker.
2. Type a valid directory path that is not currently selected in the results.
3. Click `Use typed path`.
4. Verify the picker closes and the upload dialog keeps the typed value.
5. Reopen the picker, type another valid path, and press `Enter`.
6. Verify `Enter` also accepts the typed path.

## Upload Success And Persistence

1. Start an upload to the chosen destination.
2. Verify progress UI appears and then completes successfully.
3. Confirm the remote file exists in the chosen directory.
4. Drop another file onto the same SSH panel.
5. Verify the last successful destination is remembered in the upload dialog.

## Invalid Path And Recovery

1. Reopen the picker and use a typed path that should fail, such as a non-writable or non-existent directory.
2. Start the upload.
3. Verify the upload fails with a visible error.
4. Return to the ready state.
5. Open the picker again and choose a valid path.
6. Verify a retry succeeds and the panel remains usable throughout.

## Cancel And Dismissal Behavior

1. Open the picker and press `Escape`.
2. Verify only the picker closes; the upload dialog should remain open.
3. Open the picker again and click outside it.
4. Verify the picker closes without closing the upload dialog.
5. Start an upload with a larger file and cancel it mid-transfer.
6. Verify cancellation still works and reopening the picker afterward still behaves normally.

## Taildrop Regression

Run this section only if the target also exposes Taildrop.

1. Drop a file onto the SSH panel.
2. Verify Taildrop still appears when available.
3. Switch to Taildrop.
4. Verify the remote destination picker is not required in Taildrop mode.
5. Complete a Taildrop transfer and confirm it still succeeds.

## Record Results

Capture at least:

1. Launch screenshot.
2. Post-resize screenshot.
3. SSH upload dialog before opening the picker.
4. Picker modal screenshot while browsing remote directories.
5. Success-state screenshot after upload completes.
6. Failure-state screenshot for the invalid-path case.

Record:

1. macOS version.
2. Horizon commit SHA.
3. SSH target type: local, remote, or Tailscale-backed.
4. Whether Taildrop was available.
5. Whether the local directory picker regression pass still succeeded.
