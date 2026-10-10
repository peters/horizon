---
procedure: move-workspace-search
feature: Move to Workspace menu search field
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Move to Workspace search field test procedure

## 1. Purpose

This procedure tests the menu search field in the **Move to Workspace** menu.
The field is an inset well with a search mark. The field does not show a
full-opacity accent ring.

## 2. Applicability

- Candidate: a Horizon build that contains this change.
- Platforms: Linux, with the local device fixture.
- This procedure does not test the move rules for a cloud panel.

## 3. Safety

> **CAUTION:** USE ONLY THE ISOLATED DESKTOP. A click on the developer desktop can move a real panel.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The local device fixture with `--native-view`.
- A live Device panel in the current workspace.
- At least three workspaces. One workspace holds the panel for this run.

## 5. Setup

1. Build the candidate. Copy it to a task-owned directory.

   Result: The copy has a SHA-256. The candidate does not change during the run.

2. Start the fixture with the frozen candidate and `--native-view`.

   Result: The fixture prints a loopback VNC address.

3. Open a Device panel on that address in the current workspace.

   Result: The live view shows the isolated desktop. The frame sequence advances.

## 6. Tasks

### 6.1 DARK — Dark theme field

1. Right-click the title bar of a panel.

   Result: The **Move to Workspace** menu opens. The menu search field has focus.

2. Look at the field.

   Result: The field is an inset well. A search mark is on the left. The hint
   **Search workspaces…** is dim. The outline is a soft accent edge. The outline
   is not a bright full-opacity blue ring.

3. Type part of the name of another workspace.

   Result: The list shows only workspaces with that text in the name. The count
   line changes. The typed text is bright. The search mark stays on the left.

4. Press Down, then Enter.

   Result: The panel moves to the selected workspace. The menu closes.

### 6.2 LIGHT — Light theme field

1. Open **Settings** in the toolbar. Under Appearance, choose **Light**.

   Result: The board uses the light theme.

2. Right-click a panel title bar.

   Result: The menu search field is an inset well on the light menu. The focus
   outline is visible. The outline is not a hard blue wire.

3. Type a name that no workspace uses.

   Result: The menu shows **No matching workspaces. Try another name.** Enter
   does not move the panel.

### 6.3 KEYS — Keyboard and close

1. Open the menu again.

   Result: The field is empty. Focus is in the field.

2. Press Escape.

   Result: The menu closes. The panel stays in its workspace.

3. Open the menu from the sidebar row for the same panel. Click the search mark.

   Result: The menu stays open. The field keeps focus.

## 7. Pass criteria

- The field is an inset well in the dark theme and in the light theme.
- The field has no full-opacity accent ring.
- The filter, the arrow keys, Enter, and Escape do the actions in the tasks.
- A click on the field does not close the menu.

## 8. Cleanup

1. Close the candidate on the isolated desktop. Stop only the fixture processes.

   Result: The Horizon process of the developer still runs.

## 9. Record of results

Put the results in the pull request. Keep private evidence out of the repository.
