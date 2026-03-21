# Detached Window Navigation Smoke Plan

## Goal

Verify that detached workspaces remain recoverable after aggressive panning and that the new detached-window navigation controls do not regress existing detach/reattach behavior.

## Environment

- Branch/worktree under test: `codex/detached-window-controls`
- Platform: Linux desktop session with a visible window manager
- Build: `cargo run --release`

## Baseline

1. Launch Horizon with at least two workspaces and multiple panels in one workspace.
2. Confirm the main window still shows its normal toolbar and minimap behavior.
3. Detach one populated workspace into a separate native window.

## Detached Window Checks

1. Verify the detached window toolbar shows:
   - Workspace title
   - Detached workspace label
   - `Show Minimap` or `Hide Minimap`
   - `Fit Workspace`
   - `Attach to Main Window`
2. Toggle the minimap off and on from the detached toolbar.
3. Pan the detached canvas far away from the workspace until the panels are no longer visible.
4. Use the detached minimap to jump back to the workspace.
5. Pan away again, then use `Fit Workspace` to recover the workspace.
6. Repeat the minimap click-and-drag interaction near all four minimap edges to confirm viewport framing stays stable.

## Persistence And Relaunch

1. With a detached window open, move the detached native window to a different screen position.
2. Pan the detached canvas away from the workspace and leave the minimap visible.
3. Close Horizon cleanly.
4. Relaunch Horizon.
5. Confirm the detached workspace reopens in its own native window.
6. Confirm the detached toolbar controls still render and the minimap still recovers the workspace after relaunch.

## Attach / Reattach

1. Click `Attach to Main Window` from the detached toolbar.
2. Confirm the detached native window closes and the workspace returns to the main window.
3. Detach the same workspace again and confirm the new controls reappear.

## Edge Cases

1. Test a detached workspace with no panels and confirm the toolbar still renders without crashing.
2. Test a very large workspace that requires zooming out to fit.
3. Resize the detached native window to a narrow width and confirm the toolbar buttons remain clickable.
4. Resize the detached native window taller and shorter while the minimap is visible.

## Visual Regression Checks

1. Capture a screenshot immediately after detaching.
2. Capture a screenshot after a large pan plus minimap recovery.
3. Capture a screenshot after `Fit Workspace`.
4. Compare against the main window to confirm detached chrome still matches the established titlebar styling.
