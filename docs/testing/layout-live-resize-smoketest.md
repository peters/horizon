# Layout Live Resize Smoke Test

This plan covers the correctness-sensitive UI paths for issue #37. Run it
against a build from the branch under test and capture screenshots at launch,
after each tiled layout is applied, and after each live-resize interaction.

## Environment

1. Start from a clean runtime state if possible.
2. Create one workspace with at least three terminal panels.
3. Create a second workspace with at least one terminal panel so workspace
movement and focus changes can still be checked.
4. Keep the workspace layout toolbar visible during at least one pass and use
the sidebar context menu during another pass.

## Baseline

1. Launch Horizon and confirm the first frame is correct.
2. Verify the workspace frame, layout toolbar, panel chrome, and terminal
content all render normally before any arrangement is applied.
3. Capture a baseline screenshot with the target workspace in manual placement
mode.

## Rows

1. Apply `Rows` from the workspace toolbar.
2. Confirm every panel in the workspace has the same width and height.
3. Resize the first panel with the bottom-right resize handle.
4. Confirm every sibling panel resizes in tandem and remains vertically
stacked with consistent gaps.
5. Add a new terminal to the arranged workspace.
6. Confirm the new terminal adopts the current arranged size instead of
reverting the workspace to the old default size.
7. Close one panel and confirm the remaining panels reflow without leaving
layout mode.
8. Capture a screenshot after the resize and after the add/remove checks.

## Columns

1. Apply `Columns`.
2. Confirm every panel has the same size and remains side by side.
3. Resize the first panel.
4. Confirm every sibling panel resizes in tandem and remains horizontally
aligned with consistent gaps.
5. Add a new terminal and confirm the arranged size is preserved.
6. Capture a screenshot after the resize.

## Grid

1. Apply `Grid`.
2. Confirm the panels snap into a square-ish grid with equal cell sizes.
3. Resize one panel.
4. Confirm every other panel resizes in tandem and the grid stays aligned.
5. Add a new terminal and confirm the existing arranged cell size is preserved.
6. Close a terminal and confirm the remaining panels reflow cleanly.
7. Capture a screenshot after the resize and after the add/remove checks.

## Freeform Escape

1. With `Rows`, `Columns`, or `Grid` active, drag a panel by its titlebar.
2. Confirm the workspace returns to manual placement mode.
3. Confirm the layout toolbar now shows `Default` as the selected state.
4. Resize the moved panel and confirm only that panel changes size while the
workspace remains in freeform mode.

## Removed Layouts

1. Open the workspace layout toolbar.
2. Confirm only `Rows`, `Cols`, `Grid`, and `Default` are shown.
3. Open the sidebar workspace context menu.
4. Confirm `Stack` and `Cascade` are absent there as well.

## Persistence

1. Leave one workspace in a live-resized tiled layout and a second workspace in
manual placement mode.
2. Close Horizon and relaunch it.
3. Confirm the tiled workspace restores with the same arranged layout and tile
size.
4. Confirm the manual workspace remains freeform.

## Legacy Runtime Migration

1. Prepare a runtime file that contains a workspace with `layout: Stack` or
`layout: cascade`.
2. Launch Horizon with that runtime file.
3. Confirm Horizon starts successfully without a deserialization error.
4. Confirm the migrated workspace opens in manual placement mode.

## Visual Regression Checks

1. Confirm the layout toolbar width still fits tightly around the remaining
buttons.
2. Confirm no panel overlaps the sidebar, minimap, or attention feed after a
live resize.
3. Confirm terminal content redraws correctly after each resize; look for stale
text, clipped scrollbars, or missing chrome.
4. Compare the baseline and post-resize screenshots to make sure panel geometry
changes are real layout updates, not paint glitches.
