---
procedure: remove-empty-workspace
feature: Workspace row menu
platforms: [linux, macos]
cost: none
destructive: yes
secrets: none
owner: peters
---

# Remove Empty Workspace test procedure

## 1. Purpose

This procedure makes sure that a person can remove a workspace that holds
nothing from the menu of its sidebar row, that the menu is fully visible, and
that a workspace with a panel, a cloud or a remote workspace cannot be removed
this way.

## 2. Applicability

- Candidate: a build whose workspace row menu offers **Remove Empty Workspace**.
- Platforms: Linux and macOS.
- This procedure does not test: what **New cloud** leaves behind, and the
  arrangement entries of the same menu.

## 3. Safety

> **CAUTION:** REMOVING A WORKSPACE DELETES ITS SAVED LAYOUT. Do not remove a
> workspace that another person or an agent still uses. This procedure removes
> only a workspace that you made for the test.

## 4. Equipment and preconditions

- One Horizon window, at least 1000 by 700 pixels, with one workspace that holds
  at least one terminal panel.
- No cloud is needed.

## 5. Setup

1. Note the count beside **This PC** in the sidebar.

   Result: The count shows the number of workspaces on the board, for example
   `1`.

## 6. Tasks

### 6.1 REMOVE-EMPTY-1 — The menu is visible and offers the entry

1. Press **New** beside **Workspaces**.

   Result: A new row appears in the sidebar, and the count is one higher.

2. Right-click the new row.

   Result: The whole menu is visible beside the row, with **Arrange Panels**,
   **Remove Empty Workspace** and **Close All Panels**. No part of the menu is
   cut off by the sidebar.

### 6.2 REMOVE-EMPTY-2 — The empty workspace goes

1. Press **Remove Empty Workspace**.

   Result: The row disappears, the count is one lower, and the canvas shows only
   the remaining workspace.

2. Open the sidebar again after a restart of Horizon.

   Result: The removed workspace does not come back.

### 6.3 REMOVE-EMPTY-3 — A workspace that holds something stays

1. Right-click the row of the workspace that holds a terminal panel.

   Result: **Remove Empty Workspace** is dimmed, and a hover text says that the
   workspace holds a panel, a cloud or a remote workspace, or is the last one.

2. Right-click the only row of a board with one workspace.

   Result: **Remove Empty Workspace** is dimmed, because the board keeps one
   workspace.

## 7. Pass criteria

- The row menu is fully visible outside the sidebar.
- **Remove Empty Workspace** removes an empty workspace and nothing else.
- The board keeps one workspace, and a workspace with a panel, a cloud or a
  remote workspace cannot be removed this way.
- The removed workspace stays removed after a restart.

## 8. Cleanup

1. Confirm that the workspace that holds the terminal panel is still on the
   canvas and its panel still runs.

   Result: Nothing else was removed.

## 9. Record of results

Put the results in the pull request. Keep no screenshots of other workspaces.
