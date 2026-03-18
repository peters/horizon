# Workspace Close Smoke Test Plan

This plan covers the workspace-level `Close All Panels` flow from issue #35.
Run it against the branch under test on macOS.

## macOS Notes

1. Use a secondary click, two-finger click, or `Control`-click to open context
menus.
2. If you use keyboard shortcuts during the test, substitute `Cmd` for `Ctrl`.
3. Capture at least one screenshot before closing panels and one after the
workspace becomes empty so you can compare layout and empty-workspace state.

## Environment

1. Launch Horizon from the branch under test.
2. Start from a clean session or a throwaway config so previous workspace state
does not hide regressions.
3. Create at least three workspaces:
   `alpha` as the target workspace, `beta` as a control workspace, and
   `gamma` as an intentionally empty workspace.
4. In `alpha`, create at least four panels so rows, columns, and grid have a
meaningful reflow case.
5. In `beta`, create at least one panel so you can confirm other workspaces are
untouched by the bulk-close action.

## Canvas Workspace Context Menu

1. On the canvas, open the context menu from the `alpha` workspace label.
2. Verify the menu includes the arrange actions plus `Close All Panels`.
3. Click `Close All Panels`.
4. Verify every panel in `alpha` closes.
5. Verify the `alpha` workspace frame and label remain visible after the last
panel closes.
6. Verify `beta` still contains its original panels.
7. Verify there is no orphaned panel chrome, stale scrollbar, or stray terminal
content left behind where `alpha` panels used to be.

## Sidebar Parity

1. Recreate at least two panels inside `alpha`.
2. Open the sidebar workspace context menu for `alpha`.
3. Click `Close All Panels`.
4. Verify the result matches the canvas action:
   `alpha` becomes empty but still exists, and `beta` remains unchanged.

## Empty Workspace Reuse

1. After bulk-closing `alpha`, confirm `alpha` is the active workspace.
2. Create a new terminal panel with `Cmd+N`.
3. Verify the new panel appears inside `alpha`, not inside `beta` or `gamma`.
4. Close that new panel again with the panel close button and verify normal
single-panel close still works.

## Layout Reflow Checks

These checks verify that closing panels still compacts the remaining panels for
the selected layout. Use the panel close button on a middle panel where
possible.

### Rows

1. Put at least three panels in `alpha`.
2. Apply the `Rows` layout.
3. Close the middle panel.
4. Verify the remaining panels shift upward so there is no empty slot left in
the stack.
5. Verify the workspace still reports the `Rows` layout visually.

### Columns

1. Recreate at least three panels in `alpha`.
2. Apply the `Columns` layout.
3. Close the middle panel.
4. Verify the remaining panels compact left-to-right with no empty column left
behind.

### Grid

1. Recreate at least four panels in `alpha`.
2. Apply the `Grid` layout.
3. Close one non-edge panel if the current arrangement makes that possible.
4. Verify the remaining panels repack into the grid without overlap and without
panels drifting outside the workspace frame.

### Stack

1. Recreate at least three panels in `alpha`.
2. Apply the `Stack` layout.
3. Close a middle or top panel.
4. Verify the remaining panels still form a layered stack with the expected
offset and no duplicated panel position.

### Cascade

1. Recreate at least three panels in `alpha`.
2. Apply the `Cascade` layout.
3. Close a middle panel.
4. Verify the remaining panels still cascade diagonally in order.

## Zoom And Hit Testing

1. Zoom in on the canvas and open the workspace label context menu for `alpha`.
2. Verify `Close All Panels` is still clickable and acts on the correct
workspace.
3. Zoom back out and repeat the same check.
4. Verify the workspace layout toolbar still appears correctly after recreating
panels in `alpha`.

## Persistence

1. Leave `alpha` empty after a bulk close.
2. Quit Horizon cleanly.
3. Relaunch Horizon.
4. Verify `alpha` is still present as an empty workspace.
5. Create a new panel in `alpha` after relaunch and verify the workspace is
still reusable.

## Regression Sweep

1. Repeat one bulk close while a terminal in `beta` is actively producing
output.
2. Verify closing `alpha` does not interrupt `beta`.
3. Toggle the sidebar on and off and verify the empty workspace still renders
cleanly.
4. Pan around the board and confirm the empty workspace frame, label, and any
other workspaces remain stable.
