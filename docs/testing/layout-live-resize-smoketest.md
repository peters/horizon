# Layout Live Resize Smoke Test

Temporary validation artifact for issue #37.
Use this on the machine that will do the live UI pass, then delete the file
before merge once the checklist has been executed.

This plan is intentionally exhaustive. It should let another agent validate the
layout changes without reading the implementation or the issue thread first.

## Goal

Validate that Horizon now treats `Rows`, `Columns`, and `Grid` as live tiled
layouts during manual resize, that manual drag still exits to freeform mode,
that removed layouts no longer appear in the UI, and that legacy saved runtime
state still loads cleanly.

## Required Artifacts

1. One launch screenshot with the target workspace in manual placement mode.
2. One screenshot after applying each tiled layout: `Rows`, `Columns`, `Grid`.
3. One screenshot after a live resize in each tiled layout.
4. One screenshot after forcing a return to freeform mode.
5. One screenshot after loading a legacy runtime file that previously used a
removed layout.

## Environment

1. Start from a clean runtime state if possible.
2. Use a build from the branch under test.
3. Create at least:
   - one workspace with three terminal panels
   - one neighboring workspace positioned close enough that a large resize
     could collide with it
   - one empty workspace, if practical, so label/toolbar overlap behavior can
     still be sanity-checked
4. Keep the sidebar, minimap, and any always-visible overlays enabled during at
   least one pass.
5. Run at normal zoom first, then repeat the interaction checks once while
   zoomed in or out.

## Baseline

1. Launch Horizon and confirm the first frame is correct before any layout is
   applied.
2. Confirm the workspace frame, workspace label, layout toolbar trigger area,
   panel chrome, terminal body, and scrollbar all render normally.
3. Capture the launch screenshot with the main workspace still in `Default`
   mode.
4. Hover the workspace label and confirm the layout toolbar appears with only
   `Default`, `Rows`, `Cols`, and `Grid`.
5. Open the sidebar workspace context menu and confirm `Stack` and `Cascade`
   are absent there as well.

## Rows

1. Apply `Rows` from the workspace toolbar.
2. Confirm all panels have the same width and height.
3. Confirm all panels remain vertically stacked with a consistent gap.
4. Resize the first panel larger with the bottom-right resize handle.
5. Confirm every sibling panel resizes in tandem.
6. Confirm the workspace remains in `Rows` mode after the resize.
7. Resize the first panel smaller and confirm tandem resize still works in the
   opposite direction.
8. Add a new terminal to the arranged workspace.
9. Confirm the new terminal adopts the current arranged size instead of
   restoring the old default footprint.
10. Close one panel and confirm the remaining panels reflow without leaving
    `Rows`.
11. Capture a screenshot after the live resize and another after add/remove.

## Columns

1. Apply `Columns`.
2. Confirm all panels have the same size and remain side by side.
3. Resize one panel larger.
4. Confirm every sibling panel resizes in tandem and the row stays aligned.
5. Resize one panel smaller.
6. Confirm tandem resize still works and no panel drops out of alignment.
7. Add a new terminal and confirm the arranged size is preserved.
8. Close one terminal and confirm the remaining panels reflow cleanly.
9. Capture a screenshot after the live resize.

## Grid

1. Apply `Grid`.
2. Confirm the panels snap into a square-ish grid with equal cell sizes.
3. Resize one panel larger.
4. Confirm every other panel resizes in tandem and the grid stays aligned.
5. Resize one panel smaller and repeat the same check.
6. Add a new terminal and confirm the existing arranged cell size is preserved.
7. Close a terminal and confirm the remaining panels reflow cleanly.
8. If possible, test both a two-panel grid and a three-or-four-panel grid so
   grid-dimension changes are exercised.
9. Capture a screenshot after the live resize and another after the grid
   cardinality change.

## Freeform Escape

1. With `Rows`, `Columns`, or `Grid` active, drag a panel by its titlebar.
2. Confirm the workspace returns to manual placement mode.
3. Confirm the toolbar now shows `Default` as the selected state.
4. Resize the moved panel and confirm only that panel changes size while the
   workspace remains freeform.
5. Confirm freeform resizing still pushes overlapping sibling panels as before.
6. Capture a screenshot after the freeform escape.

## Workspace Collision Behavior

1. Place another workspace close to the right or bottom edge of the workspace
   under test.
2. Enter each tiled layout and resize it larger.
3. Confirm the enlarged workspace does not visually overlap the neighboring
   workspace; the neighbor should still be pushed away if a collision would
   occur.
4. Confirm shrinking the tiled layout does not corrupt the neighboring
   workspace position or leave panels visually detached from their frame.

## Zoom And Canvas Interaction

1. Zoom in or out from the default canvas scale.
2. Repeat one `Rows` live resize and one `Grid` live resize at the non-default
   zoom level.
3. Confirm the resize handle remains hittable and the resize speed still maps
   correctly to canvas space.
4. Confirm the layout toolbar buttons remain clickable at the non-default zoom
   level.

## Persistence

1. Leave one workspace in a live-resized tiled layout and a second workspace in
   manual placement mode.
2. Close Horizon and relaunch it.
3. Confirm the tiled workspace restores with the same arranged layout and tile
   size.
4. Confirm the freeform workspace remains freeform.
5. Confirm the removed layouts do not reappear after relaunch.

## Legacy Runtime Migration

1. Prepare a runtime file that contains a workspace with `layout: Stack`.
2. Prepare a second runtime file or workspace entry with `layout: cascade`.
3. Launch Horizon against that runtime state.
4. Confirm Horizon starts successfully without a deserialization error.
5. Confirm each migrated workspace opens in manual placement mode.
6. Confirm the toolbar still shows only `Default`, `Rows`, `Cols`, and `Grid`
   after migration.
7. Capture a screenshot of the migrated workspace state.

## Visual Regression Checks

1. Confirm the layout toolbar width still fits tightly around the remaining
   buttons and does not leave the previous large empty area.
2. Confirm no panel overlaps the sidebar, minimap, or attention feed after a
   live resize.
3. Confirm workspace frames still fully enclose their panels after add/remove
   and resize.
4. Confirm terminal content redraws correctly after each resize; look for stale
   text, clipped scrollbars, missing chrome, or partially repainted cells.
5. Compare the baseline and post-resize screenshots to make sure geometry
   changes are true layout updates, not paint glitches.

## Exit Criteria

1. Every section above passes without a correctness regression.
2. The screenshot set is attached to the PR or test handoff.
3. After the live validation is complete, delete this file before merge unless
   the user explicitly wants to keep it.
