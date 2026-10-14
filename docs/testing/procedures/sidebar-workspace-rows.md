---
procedure: sidebar-workspace-rows
feature: The workspace and panel rows of the sidebar
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Sidebar workspace rows test procedure

## 1. Purpose

This procedure makes sure that each workspace row in the sidebar shows a tile in
the workspace color with the initials of its name, and a status badge only when
the workspace is not idle. It also makes sure that the panel rows of the active
workspace show the task of an agent, a working dot, and a close button only on
the hovered or focused row.

## 2. Applicability

- Candidate: a build that shows workspace tiles in the sidebar.
- Platforms: Linux.
- This procedure does not test these functions:
  - The groups **Needs you**, **Cloud** and **Parked**. The
    [cloud list procedure](cloud-list.md) tests them with a real cloud.
  - The status of a cloud workspace. The unit tests of the cloud list cover it.

## 3. Safety

> **CAUTION:** USE ONLY SYNTHETIC CONTENT AND GENERIC WORKSPACE NAMES.
> Screenshots and recordings show the sidebar and the panels.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The [local device smoke fixture](../../../scripts/device-smoke/README.md),
  `scripts/device-smoke/serve.py`, with `--native-view`, and a Horizon host
  with a Device panel for the fixture.
- A script that acts as an agent. The script sets the terminal title to a
  spinner glyph and a task, for example `◐ Fix the sync timeout`. When it works,
  it shows a line such as `⠼ Working… (12s · esc to interrupt)` at the bottom
  of its screen and changes the spinner glyph each 250 ms. When it is idle, it
  shows `> Ask me to do anything` and stops.
- A private configuration with `features: {sidebar_accordion: true}` and these
  workspaces:
  - `Checkout redesign`: a Claude Code panel that runs the script and works,
    and an editor panel `notes.md`.
  - `billing-service`: a command panel that prints one line and waits.
  - `Horizon`: two Claude Code panels that work, one Claude Code panel that is
    idle, and a Device panel.
  - Six or more other workspaces with one or two panels each.

## 5. Setup

1. Start the fixture with the candidate and the private configuration. Open a
   Device panel for its `vnc_address`.

   Result: The Device panel shows the fixture desktop. The Horizon window shows
   the sidebar with the header **Workspaces** and the group **This PC**.

## 6. Tasks

### 6.1 TILES: Workspace rows

1. Look at the row of `billing-service`.

   Result: The row shows a tile with `BS` in the workspace color, the name in
   bold text and, under the name, the line that the panel printed. The panel
   count `1` is in the right column. The tile has no badge.

2. Look at the row of `Checkout redesign`.

   Result: The tile shows `CR` and a green badge on its lower-right corner. The
   line under the name shows the working line of the agent.

3. Look at the counts of all rows.

   Result: All counts are in one column at the right edge of the rows.

4. Look at the group header.

   Result: The header shows `This PC` in sentence case, the count of the
   workspaces and, at the right, `live`.

### 6.2 ACTIVE: Select a workspace

1. Click the row of `Horizon`.

   Result: The row becomes a card with an outline in the workspace color. Four
   panel rows show under it. A tree line connects them to the tile.

2. Look at the panel rows of the agents.

   Result: Each row shows `CC` and the task of the agent, for example
   `Fix recurring sync timeout`. The rows do not repeat the agent name and do
   not show a spinner glyph. The rows of the working agents show a green dot at the right.

### 6.3 PANEL: Hover and focus a panel

1. Move the pointer on the second panel row. This row is a working agent and
   is not the focused row.

   Result: The row becomes lighter. A close button (×) replaces the green dot.

2. Move the pointer away from the row.

   Result: The close button goes away. The green dot shows again.

3. Click the third panel row.

   Result: The row becomes the focused row, with a tint of the workspace color
   and a close button. The canvas shows the panel.

4. Push Tab until a panel row that is not focused has the keyboard focus.

   Result: The row shows its close button while it has the keyboard focus.

5. If the computer has a touch screen, tap the right end of a panel row that is
   not focused and that you did not touch before. If not, use a script that
   moves the pointer and clicks in one step, for example
   `xdotool mousemove <x> <y> click 1` on the fixture display.

   Result: The row becomes the focused row. The panel stays open. The close
   button closes a panel only when it showed before the press.

### 6.4 SCROLL: A long list

1. Make the Horizon window less high than the list. Turn the mouse wheel on the
   sidebar.

   Result: The list scrolls. The rows keep their tiles, names and counts.

### 6.5 LIGHT: Light theme

1. Set the appearance to light and restart the fixture.

   Result: The tiles, the badges, the names and the lines are easy to read on
   the light surface.

### 6.6 NARROW: A narrow sidebar

1. Hold the pointer on a workspace name that ends with `…`, then on a status
   line that ends with `…`, then on a panel row that ends with `…`.

   Result: Each time, a tooltip shows the full text.

2. Right-click the row of `billing-service` and click **Open in New Window**.
   Look at its row in the main window.

   Result: The row shows the `NEW WINDOW` badge before the panel count.

3. Make the Horizon window 900 pixels wide or less. The sidebar becomes 168
   pixels wide.

   Result: The row of `billing-service` shows no `NEW WINDOW` badge. The name
   keeps its room. The group header summary becomes shorter or goes away; it
   does not go to a second line.

4. Hold the pointer on the tile of `billing-service`.

   Result: The tooltip shows `Idle` and `In a new window`.

5. Right-click the row of `billing-service` and click **Move to Main Window**.
   Make the Horizon window wide again.

   Result: The workspace is back in the main window. The row shows no badge.

### 6.7 SCREEN READER: Spoken names

1. Start a screen reader on the fixture desktop, for example Orca. Move the
   keyboard focus through the sidebar.

   Result: A workspace row says its name and its panel count, for example
   `Horizon, 4 panels`. A row in a new window also says `in a new window`.

2. Move the focus to the panel rows of `Horizon`.

   Result: Each row says the kind and the label, for example
   `Claude: Fix recurring sync timeout, working`. The close button says
   `Close panel`.

3. Move the focus to the group header.

   Result: The header says the group name and its count, for example
   `This PC 13`. The summary `live` is read too. On a narrow sidebar where the
   summary does not show, the header says the summary after the count.

## 7. Pass criteria

- 6.1 shows a tile with initials on each row and a badge only on a working row.
- 6.2 shows the active card, the tree line and the agent tasks.
- 6.3 shows the close button only on the hovered or focused row, and a tap on
  a hidden close button focuses the row and does not close the panel.
- 6.4 scrolls the list.
- 6.5 keeps the rows readable in the light theme.
- 6.6 shows the full text of a cut name, line or title on hover, and moves the
  `NEW WINDOW` badge to the tile tooltip on a narrow sidebar.
- 6.7 gives each row, panel row and header a spoken name with the facts that
  the row shows.

## 8. Cleanup

1. Close Horizon normally in the fixture, for example with the window close
   button.

   Result: The candidate process exits. Make sure that no process with the
   PID of the candidate remains.

2. Close the Device panel with the `device_panel` operation `close`.

   Result: The Device panel goes away. The viewer connection closes.

3. Stop the fixture with Ctrl+C in its terminal.

   Result: The fixture stops its child processes and removes its target file.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
