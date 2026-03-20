# Detached Workspace Interactions Smoke Test

## Scope

Validate that a detached workspace remains fully interactive after the detached
viewport state fix. The detached window must keep its own canvas pan and zoom
state, allow workspace and panel manipulation, and continue syncing native
window bounds without regressing reattach behavior.

## Environment

- Build from the `fix/detached-workspace-interactions` worktree.
- Run Horizon with a temporary `HOME` so the test does not mutate the primary
  runtime state.
- Start with a board that has:
  - at least two workspaces
  - one workspace that will remain attached in the root window
  - one workspace that will be detached
  - at least two visible panels inside the workspace that will be detached
- Ensure the detached workspace has enough free canvas space around the panels
  so panning and drag feedback are visible.

## Baseline

1. Launch Horizon and confirm both workspaces render in the root window.
2. Confirm the target workspace can be panned, panel-dragged, and panel-resized
   before detaching.
3. Capture a screenshot of the root window before detaching.

## Primary Flow

1. Detach the target workspace into its own native window.
2. Confirm the detached window opens with the workspace visible and framed below
   the detached toolbar.
3. In the detached window, pan the canvas with middle-mouse drag.
4. Release the pointer and confirm the canvas stays at the new offset on the
   next frame.
5. Pan again with `Space` + primary drag and confirm it also persists.
6. Scroll on empty canvas and confirm the canvas pans instead of snapping back.
7. Zoom in and zoom out in the detached window and confirm the zoom level is
   preserved after pointer release.
8. Drag the detached workspace label and confirm the workspace remains in the
   new position.
9. Drag one panel inside the detached workspace and confirm the move persists.
10. Resize one panel inside the detached workspace and confirm the new size
    persists.
11. Interact with a terminal in the resized panel and confirm the terminal
    remains responsive after the resize commit.

## Native Window Resize

1. Resize the detached native window wider and taller.
2. Confirm the detached workspace remains visible and interactive after the
   native resize.
3. Repeat panel drag and panel resize after the native window resize.
4. Confirm the detached window does not snap back to an older saved outer
   position while dragging the native window.

## Reattach And Restore

1. Click `Attach to Main Window` in the detached toolbar.
2. Confirm the detached window closes and the workspace reappears in the root
   window.
3. Detach the workspace again, move and resize content, then close Horizon.
4. Relaunch from the same temporary session state.
5. Confirm the workspace restores in its detached window.
6. Confirm pan, zoom, workspace drag, panel drag, and panel resize still work
   after restore.

## Edge Cases

1. Repeat the detached interactions with the workspace reduced to a single
   panel.
2. Repeat the detached interactions with an empty workspace.
3. Confirm the attached root window remains interactive while another workspace
   is detached.
4. Confirm command-palette flows that intentionally exclude detached workspaces
   still behave as before.

## Evidence

1. Capture a screenshot after the initial detach.
2. Capture a screenshot after moving and resizing content in the detached
   window.
3. Capture motion-sensitive evidence for detached drag behavior, preferably a
   short video or native window position trace.
4. Capture a screenshot after restore/relaunch with the detached workspace still
   interactive.
