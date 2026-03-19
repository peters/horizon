# Detached Workspace Smoke Test

## Goal

Validate the detached-workspace drag fix, confirm repeated detached-window resizing does not corrupt the terminal buffer, and confirm workspace layout controls remain visible for any non-empty workspace, including single-panel workspaces.

## Environment

- Build from the branch under test in a desktop session with multiple native windows enabled.
- Prefer Linux/X11 or Wayland because the detached-window drag bug was observed there.
- Start from a fresh runtime state if possible, or note any existing detached workspaces before testing.

## Setup

1. Run `cargo run --release`.
2. Create at least three workspaces:
   - Workspace A with one panel.
   - Workspace B with two panels.
   - Workspace C empty.
3. Position the workspaces so they are all visible in the main window.
4. If needed, resize Workspace A's single panel so the workspace label and toolbar are easy to inspect.

## Baseline Checks

1. Confirm Workspace A shows the layout toolbar beside its label without requiring hover or focus changes.
2. Confirm Workspace B also shows the layout toolbar.
3. Confirm Workspace C does not show the layout toolbar.
4. Confirm the toolbar includes `Default`, `Rows`, `Cols`, `Grid`, and `Detach`.
5. Capture a screenshot of the main window with all three workspaces visible.

## Primary Detached-Window Flow

1. Click `Detach` on Workspace A.
2. Verify a separate native window opens for Workspace A.
3. Drag the detached window continuously in small circles for 5-10 seconds.
4. Drag the detached window quickly across the screen, then stop abruptly.
5. Verify the detached window tracks the pointer smoothly and does not:
   - jitter
   - accelerate away from the pointer
   - snap back to prior positions
   - oscillate after the mouse is released
6. Resize the detached window from two different edges and verify the content keeps rendering correctly.
7. With visible shell output in the detached terminal, resize the detached window repeatedly for 5-10 seconds from alternating edges and corners.
8. Verify the terminal buffer remains sane throughout the repeated resize sequence:
   - no large blank bands appear between prompt and content
   - no stale lines are duplicated far below the cursor
   - the prompt stays anchored near the current input line instead of drifting vertically
   - newly typed characters appear on the active prompt line
9. Capture a screenshot of the detached window after dragging and after the repeated resize sequence.

## Persistence And Restore

1. Leave Workspace A detached and move it to a distinctive screen position.
2. Close Horizon normally.
3. Relaunch Horizon.
4. Verify Workspace A reopens as a detached window.
5. Verify the detached window restores near the last position instead of falling back to the main window origin.
6. Drag the restored detached window again and confirm the drag remains stable after restore.

## Reattach Flow

1. In the detached window, click `Attach to Main Window`.
2. Verify the detached window closes and Workspace A returns to the main canvas.
3. Verify Workspace A still shows the layout toolbar immediately after reattaching.
4. Detach Workspace A a second time and repeat a short drag test to confirm the one-shot restore path still works after reattach/detach cycling.

## Layout Control Behavior

1. With Workspace A back in the main window, click `Rows`, `Cols`, and `Grid` in sequence.
2. Verify each click is accepted even when Workspace A contains only one panel.
3. Add a second panel to Workspace A.
4. Verify the previously selected layout still applies correctly once a second panel exists.
5. Click `Default` and verify manual placement mode is restored.

## Edge Cases

1. Detach Workspace B while Workspace A is already detached.
2. Drag both detached windows independently and verify neither affects the other.
3. Close one detached window with the native window close control and verify it reattaches cleanly to the main window.
4. Rename a workspace, then detach it, and verify the detached window title updates correctly.
5. Run a full-screen terminal UI with a bottom status or input bar, resize the detached window repeatedly, and verify the UI reflows without leaving a large blank band between the conversation/output area and the bottom bar.
6. Confirm the toolbar does not render for workspaces hidden behind fixed overlays if the label itself is intentionally suppressed there.

## Regression Watchlist

- Detached window drag becomes unstable only after persistence restore.
- Detached window resize leaves the PTY buffer visually desynchronized from the rendered grid.
- Full-screen terminal UIs keep their bottom composer or status bar pinned but leave a large stale blank region above it after resize.
- Detached window position resets every frame while dragging.
- Layout toolbar disappears for single-panel workspaces after focus changes.
- Empty workspaces incorrectly show layout controls.
- Main-window hit-testing or panel interactions regress while detached viewports are open.

## Report Template

- Platform/session type:
- Commit under test:
- Pass/fail:
- Screenshots captured:
- Notes on drag smoothness:
- Notes on restore position:
- Notes on layout toolbar visibility:
