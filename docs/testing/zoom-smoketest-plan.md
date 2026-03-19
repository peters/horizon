# Zoom Smoke Test Plan

This plan covers the correctness-sensitive paths introduced by canvas zoom in
issue #18. Run it against a build from the branch under test, and capture a
launch screenshot plus at least one screenshot each for zoomed-in and zoomed-out
states.

## Environment

1. Start with a clean persistent session and at least two workspaces.
2. Put at least one terminal panel, one non-terminal panel, and one empty
workspace on the board.
3. Verify both a mouse wheel path and, if the platform supports it, a trackpad
pinch path.

## Baseline

1. Launch Horizon and confirm the first frame is correct before any interaction.
2. Verify the grid, workspace frames, workspace labels, panel chrome, panel
content, and minimap all render at default zoom.
3. Press `Ctrl+Shift+0` and confirm the view stays at identity when already reset.

## Zoom In And Out

1. Hover empty canvas and zoom in several steps with `Ctrl+Scroll`.
2. Hover empty canvas and zoom back out past the default level.
3. Verify the grid spacing, workspace frames, labels, panel chrome, and panel
content all visually scale instead of only recomputing layout.
4. Hover directly over a terminal panel and repeat the zoom-in and zoom-out
sequence.
5. Hover directly over an editor or usage panel and repeat the same sequence.
6. If available, repeat the same checks with a pinch gesture.
7. Use `Ctrl+Shift+Plus` and `Ctrl+Shift+Minus` and confirm the viewport-center shortcut
path behaves the same as cursor-anchored zoom.
8. Zoom to the minimum clamp and confirm further zoom-out input is ignored
cleanly.
9. Zoom to the maximum clamp and confirm further zoom-in input is ignored
cleanly.

## Anchor Correctness

1. Pick a visible panel corner, hover that exact point, zoom in, and verify the
corner stays under the cursor.
2. Repeat the same test near the edge of the canvas viewport.
3. Repeat over a workspace label.
4. Repeat over terminal text near the scrollbar.

## Panning And Reset

1. Pan at default zoom with middle-drag and with `Space+Drag`.
2. Pan at a zoomed-in level and confirm movement remains smooth and screen-space
consistent.
3. Pan at a zoomed-out level and confirm no jump or drift occurs.
4. Scroll-pan on empty canvas without `Ctrl` and confirm that still pans rather
than zooms.
5. Press `Ctrl+Shift+0` from a zoomed, panned state and confirm both pan and zoom
reset together.

## Minimap

1. Zoom in and confirm the minimap viewport rectangle shrinks appropriately.
2. Zoom out and confirm the minimap viewport rectangle grows appropriately.
3. Click the minimap at several points while zoomed in and verify the clicked
region centers in the main viewport without changing zoom.
4. Drag across the minimap while zoomed out and confirm the viewport tracks
correctly.
5. Verify minimap panel rectangles still align with the corresponding panels on
the main canvas.

## Interaction Correctness

1. Drag a panel while zoomed in and confirm the panel follows the pointer
without overshooting.
2. Drag the same panel while zoomed out and confirm movement stays accurate.
3. Resize a panel while zoomed in and verify resize speed matches pointer
movement in canvas space.
4. Resize a panel while zoomed out and repeat the check.
5. Drag a workspace label while zoomed in and out and confirm the whole
workspace moves correctly.
6. Use the workspace layout toolbar while zoomed in and out and confirm the
buttons remain clickable and apply the expected arrangement.
7. Ctrl-double-click the canvas while zoomed in and while zoomed out and verify
the new workspace lands at the clicked canvas location.
8. Drop a markdown file onto the canvas while zoomed in and confirm the editor
panel opens where expected.

## Terminal-Specific Checks

1. Click to focus a terminal at default zoom, zoomed in, and zoomed out.
2. Select terminal text at multiple zoom levels and verify selection boundaries
track the cursor correctly.
3. Scroll terminal scrollback without `Ctrl` and confirm it still affects the
terminal instead of the canvas.
4. Scroll with `Ctrl` over a terminal and confirm the canvas zooms instead of
sending wheel input to the terminal.
5. Ctrl-click a URL or file path while zoomed in and out and confirm hit-testing
still lands on the correct target.
6. Drag the scrollbar thumb while zoomed in and out and confirm scrollback maps
to the expected position.

## Persistence And Resize

1. Leave the app in a non-default pan and zoom state, close Horizon, relaunch,
and confirm the exact view state is restored.
2. Repeat the same persistence check after using the minimap to change the
viewport.
3. Resize the application window while zoomed in and confirm the canvas,
workspace frames, and minimap stay correct.
4. Resize while zoomed out and repeat the same checks.
5. Close workspaces until one remains and verify the automatic refocus path
preserves the current zoom while panning to the surviving workspace.

## Visual Regression Checks

1. Confirm no panel or workspace paints across the toolbar or sidebar after
zooming and panning.
2. Confirm off-screen culling still hides fully invisible panels and workspaces.
3. Confirm the canvas HUD reports a sensible origin and zoom percentage.
4. Compare the launch, zoomed-in, and zoomed-out screenshots to ensure no layer
is missing, double-painted, or clipped incorrectly.
