# Minimap Workspace Label Smoke Plan

Validate that workspace names render correctly inside the minimap overlay.

## Goal

Verify that:

- each workspace rectangle in the minimap shows its name
- labels scale with workspace rect size
- labels clip cleanly and do not overflow
- the minimap remains visible during zoom and pan operations

## Recommended Binary

```bash
cargo build -p horizon-ui
target/debug/horizon --new-session
```

## Setup

Create a board with at least three workspaces, each containing one or more
panels. Give each workspace a distinct name (short, medium, and long) so label
clipping and sizing can be observed.

Example:

1. Workspace "Dev" -- one panel
2. Workspace "Monitoring Dashboard" -- two panels side by side
3. Workspace "A" -- one small panel

## Baseline

1. Launch Horizon with the setup above.
2. Confirm the minimap appears at the bottom-right corner.
3. Confirm every workspace rectangle in the minimap has a name label.
4. Confirm the label text matches the workspace name.
5. Confirm labels use the workspace accent color (brighter for the active
   workspace, dimmer for inactive ones).

## Label Scaling

6. With only one workspace visible (delete or collapse others), confirm the
   label font is noticeably larger than when three workspaces are spread far
   apart.
7. Create five or more workspaces spread across a wide canvas area. Confirm
   labels shrink proportionally as workspace rects get smaller.
8. When a workspace rect is very small (< 18 px in either dimension), confirm
   the label is hidden rather than rendered illegibly.

## Clipping

9. Give a workspace a very long name (e.g., 40+ characters).
10. Confirm the label text is clipped at the workspace rect boundary and does
    not bleed into neighboring workspace rects or the minimap background.

## Zoom and Pan

11. Zoom in (Ctrl+Scroll or pinch) until the viewport indicator in the minimap
    is small. Confirm the minimap and all labels remain visible.
12. Zoom out to minimum zoom (0.25x). Confirm the minimap and labels remain
    visible.
13. Pan the canvas (middle-click drag or Space+drag). Confirm the minimap
    viewport indicator moves but labels stay stable.
14. Use the "Fit Workspace" shortcut. Confirm the minimap updates the viewport
    indicator and labels remain unchanged.
15. Click inside the minimap to jump to a different canvas position. Confirm
    labels are not disrupted.

## Workspace Rename

16. Rename a workspace via the sidebar.
17. Confirm the minimap label updates to reflect the new name on the next
    frame.

## Detached Viewports

18. Detach a workspace into its own window.
19. Confirm the detached workspace's minimap (if visible) shows the correct
    label.
20. Confirm the main window's minimap no longer shows the detached workspace
    label.

## Single Workspace / Empty State

21. Close all panels so only one empty workspace remains (if retained).
    Confirm the minimap shows that workspace with its label and does not
    disappear.
22. Close all workspaces. Confirm the minimap hides gracefully (no crash, no
    stale labels).

## Visual Regression

23. Take a screenshot at default zoom with three workspaces.
24. Compare label positioning: labels should be horizontally centered at the
    top of each workspace rect with a small vertical offset.
25. Confirm label color matches the workspace accent and is legible against the
    minimap background.
