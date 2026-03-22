# SSH Upload Smoke Plan

## Baseline

1. Launch Horizon in a clean temporary HOME with no restored crash state.
2. Verify the app window maps correctly and the default workspace renders.
3. Verify a normal shell panel still accepts drag-and-drop as pasted local paths.
4. Capture a screenshot after launch.
5. Resize the window, run fit/focus interactions, and capture a second screenshot.

## Primary Flows

1. Open or create an SSH panel that connects successfully.
2. Drop one local file onto the SSH panel and verify an upload modal appears instead of terminal path paste.
3. Wait for preparation to finish and verify:
   - Taildrop is offered only when detected
   - SSH upload is always offered
   - the destination field is prefilled
4. For SSH upload:
   - open the remote directory browser
   - navigate into a child directory and back to parent
   - choose the current directory
   - start the upload
   - verify the progress UI updates and the modal reaches a success state
5. For Taildrop, if available:
   - choose Taildrop
   - verify the destination picker is replaced by Taildrop messaging
   - start the transfer
   - verify the modal reaches a success state

## Edge Cases

1. Drop multiple files and verify the summary and completed/total count update.
2. Cancel an upload in progress and verify the modal reports cancellation cleanly.
3. Force an SSH upload failure with an invalid destination and verify the modal reports the error and allows returning to Ready state.
4. Drop a directory or a non-path payload and verify the modal reports that only local filesystem files are supported.
5. Verify closing the modal does not close or corrupt the SSH panel.

## Persistence And Regression

1. Complete an SSH upload, close the modal, then drop another file onto the same SSH panel and verify the last successful destination is reused.
2. Verify the remote directory browser still works after a failed upload.
3. Verify normal editor file drops still open editor panels when no terminal target is used.
4. Verify remote hosts overlay, command palette, and normal panel rendering still behave after an upload flow.
