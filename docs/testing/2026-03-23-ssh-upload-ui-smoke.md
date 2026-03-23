# SSH Upload UI Redesign Smoke Plan

Use this checklist to manually validate the redesigned SSH upload modal.

## Goal

Verify that the new upload modal:

- renders correctly in all states (preparing, ready, uploading, finished, failed)
- preserves all functional behavior from the previous implementation
- looks polished with proper alignment, spacing, and colors
- handles edge cases (many files, long names, errors) gracefully

## Recommended Binary

```bash
cargo build -p horizon-ui
target/debug/horizon --new-session
```

## Setup

Choose an SSH target you can connect to. Prepare:

1. One small text file (e.g. `hello.txt`, a few bytes).
2. One larger file so progress is visible (e.g. 5+ MB).
3. Several files with varying name lengths, including one with a long name (20+ chars).
4. A writable destination directory on the remote host.
5. A second remote directory for browser navigation checks.

For artifact capture on Linux:

```bash
mkdir -p /tmp/horizon-smoke-artifacts
# Still screenshot
gnome-screenshot -f /tmp/horizon-smoke-artifacts/modal-ready.png
# Or use scrot/flameshot
```

## 1. Baseline

- [ ] Launch Horizon, verify the app window renders correctly
- [ ] Open an SSH panel and connect to the target host
- [ ] Verify the terminal session is functional (run a command)

## 2. Modal Appearance -- Ready State

- [ ] Drag one small file onto the SSH panel
- [ ] Verify the modal appears centered with a dark backdrop
- [ ] **Header**: dark background strip with blue upload arrow icon, "Upload to <host>" title, and file summary text
- [ ] **File pills**: cyan dot, file name, and file size in a rounded pill
- [ ] **Transfer method**: segmented control with "SSH (scp)" active by default (or Taildrop if available)
- [ ] **Remote destination**: text input inside a bordered container with folder icon, Browse and Refresh buttons
- [ ] **Action bar**: separator line, ghost "Cancel" button on left, blue "Start Upload" button on right
- [ ] Verify the "Start Upload" button is enabled only when the destination input is non-empty
- [ ] Capture a screenshot of the ready state

## 3. File Pills -- Multiple Files

- [ ] Close the modal (Cancel), then drag 3 files onto the SSH panel
- [ ] Verify 3 file pills appear, each with a cyan dot, name, and size
- [ ] Close, then drag 7+ files
- [ ] Verify 6 pills are shown plus a "+N more" overflow pill
- [ ] Verify a file with a long name (20+ chars) is truncated with "..."
- [ ] Capture a screenshot of the multi-file state

## 4. Directory Browser

- [ ] Open the modal with a file drop, click "Browse"
- [ ] Verify the browser panel appears below the destination input
- [ ] Verify each directory row shows a yellow folder icon and the directory name
- [ ] Verify ".." row text is dimmer than regular directories
- [ ] Hover over a directory row -- verify highlight appears and cursor changes to pointer
- [ ] Click a directory to navigate into it -- verify the destination input updates
- [ ] Click ".." to navigate up
- [ ] Click "Use this folder" -- verify the destination input updates to the current browser path
- [ ] Click "Close" to hide the browser
- [ ] Click "Refresh" -- verify the listing reloads

## 5. Transport Choice (if Taildrop available)

- [ ] If Taildrop is detected, verify a second segment appears: "Taildrop (<target>)"
- [ ] Click the Taildrop segment -- verify the destination input disappears and a cyan info box appears
- [ ] Click back to SSH -- verify the destination input reappears
- [ ] If SSH is unavailable, verify the SSH segment is visually disabled

## 6. Upload Progress

- [ ] Select a destination and click "Start Upload" with the larger file
- [ ] Verify the state changes to show:
  - "Uploading..." title
  - Current file name with a blue dot indicator
  - Thin custom progress bar filling left to right with accent color
  - Glow effect at the progress bar tip
  - File count (N / M files) on the left, byte count on the right
  - Detail text below
- [ ] Verify the "Cancel Upload" ghost button is visible
- [ ] Capture a screenshot mid-upload

## 7. Upload Completion

- [ ] Let the upload finish
- [ ] Verify the finished state shows:
  - Green checkmark icon in a tinted circle
  - "Upload complete" title
  - Detail text and byte summary
  - Separator line
  - Blue "Done" button right-aligned
- [ ] Click "Done" -- verify the modal closes
- [ ] Verify the uploaded file exists on the remote host
- [ ] Capture a screenshot of the finished state

## 8. Upload Cancellation

- [ ] Start another upload with the larger file
- [ ] Click "Cancel Upload" while in progress
- [ ] Verify the cancelled state shows:
  - Yellow dash icon in a tinted circle
  - "Upload cancelled" title
  - Blue "Done" button
- [ ] Click "Done" -- verify the modal closes

## 9. Failure State

- [ ] Trigger a failure (e.g. set destination to a non-existent or unwritable path like `/root/nope`)
- [ ] Verify the failed state shows:
  - Red X icon in a tinted circle
  - "Upload failed" title in red
  - Error message in a red-tinted frame
  - Separator line
  - Ghost "Close" button on left, blue "Try Again" button on right
- [ ] Click "Try Again" -- verify the modal returns to the ready state
- [ ] Click "Close" -- verify the modal closes
- [ ] Capture a screenshot of the failed state

## 10. Preparing State

- [ ] Drop a file on an SSH panel that requires a slower connection probe
- [ ] Verify the preparing state shows a loading spinner with "Preparing upload..." text
- [ ] Verify it transitions to the ready state once probing completes

## 11. Destination Persistence

- [ ] Complete a successful upload to a specific directory
- [ ] Close the modal, then drop another file on the same SSH panel
- [ ] Verify the destination input is pre-filled with the previously used directory

## 12. Edge Cases

- [ ] Drop files with no SSH connection (local panel) -- verify the upload modal does NOT appear
- [ ] While an upload is in progress, try dropping more files -- verify the drop is ignored
- [ ] Resize the Horizon window while the modal is open -- verify the modal stays centered
- [ ] Verify the backdrop covers the full viewport and blocks interaction with panels behind it

## 13. Visual Regression Checks

- [ ] Compare ready-state screenshot against the previous implementation -- verify improved visual quality
- [ ] Verify all text is legible (proper contrast against backgrounds)
- [ ] Verify rounded corners are consistent (14px modal, 10px inputs/browser, 8px pills)
- [ ] Verify the color palette is consistent with the rest of the Horizon theme
