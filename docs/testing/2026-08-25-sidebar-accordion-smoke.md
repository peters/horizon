# Smoke Plan: Sidebar Accordion Workspaces

## Purpose

Validate that the workspace sidebar collapses inactive workspaces so a board with many workspaces stays scannable, without changing click, drag, detach, or context-menu behavior.

## Machine Requirements

- Linux desktop session with a mapped Horizon window (X11 or Wayland)
- Local build prerequisites already installed
- Mouse available for click, context-menu, and drag testing

## Build + Launch

1. Open a terminal in this checkout.
2. Run `cargo build`.
3. Create a disposable config directory outside the repo, for example `/tmp/horizon-sidebar-accordion-smoke`.
4. Launch the exact candidate with an isolated shell and no inherited session:

   ```bash
   env -u HORIZON HOME=/tmp/horizon-sidebar-accordion-smoke \
     target/debug/horizon --config /tmp/horizon-sidebar-accordion-smoke/config.yaml --ephemeral
   ```

5. Keep the sidebar visible for the full pass (`Ctrl+Shift+B` if it is hidden).

## Test Data Setup

1. Create at least eight workspaces with **New**.
2. Put two or more panels in the first three workspaces.
3. Leave at least one workspace empty.
4. Detach one workspace to a new window.
5. If attention feed is enabled, produce at least one attention item in a non-active workspace.

## Baseline

1. Confirm only the active workspace shows panel rows.
2. Confirm every workspace row still shows its name, accent bar, and a panel count.
3. Confirm inactive workspaces show no panel rows.
4. Confirm the empty workspace shows count `0` and no panel rows when active.
5. Confirm the detached workspace shows `NEW WINDOW` and stays collapsed while another workspace is active.
6. Capture a screenshot of the sidebar with eight-plus workspaces.

## Primary Flows

1. Click an inactive workspace that has multiple panels.
   - It becomes active and expands.
   - The previously active workspace collapses.
   - The board pans to that workspace as before.
2. Click an inactive workspace that has exactly one panel.
   - That panel is focused.
   - That workspace expands and the previous one collapses.
3. Click a panel row under the active workspace.
   - That panel focuses and the workspace stays expanded.
   - Other workspaces stay collapsed.
4. Click a panel on the canvas that belongs to a collapsed workspace.
   - That workspace becomes active and expands in the sidebar.
5. Open the workspace context menu from a collapsed row.
   - Arrange / detach / close-all still work.
6. Drag a collapsed workspace row before or after another row.
   - Drop indicator still appears.
   - Order updates in the sidebar and on the board.

## Edge Cases

1. Create a new workspace. Confirm it becomes the expanded workspace and others collapse.
2. Close all panels in the active workspace. Confirm the row stays expanded with count `0`.
3. Reattach the detached workspace. Confirm accordion expand/collapse still follows the active workspace.
4. Hide and show the sidebar. Confirm the active workspace is still the only expanded one.
5. With many workspaces, confirm the list stays short enough that most names remain on screen without scrolling past collapsed rows.
6. Resize the window narrower. Confirm long workspace names truncate instead of overlapping the count.

## Persistence / Restore

Accordion expansion is derived from the active workspace, not stored separately.

1. Set a non-first workspace active, then quit Horizon cleanly.
2. Relaunch into the same isolated config if using a persisted session, or repeat the setup in a fresh ephemeral session and skip this lane.
3. Confirm the restored active workspace is the one that expands.

## Visual

1. Screenshot after launch with several workspaces.
2. Screenshot after switching the active workspace.
3. Confirm inactive rows stay at the existing 32px workspace-row height, with dim counts and no panel rows between them.

## Pass Criteria

- Only the active workspace lists panels.
- Click, drag, detach, and context-menu behavior from the workspace row still match pre-change behavior.
- Panel counts and `NEW WINDOW` remain readable at the default sidebar width.
