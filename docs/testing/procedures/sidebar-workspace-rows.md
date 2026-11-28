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

This procedure makes sure that each work item row of the sidebar shows a tile in
the workspace color with the initials of its name, a state word under its
name, and no badge on its tile. It also makes sure that the panel rows of a work
item stay folded until the fold control opens them, and that a panel row shows
the task of an agent, a working dot, and a close button only on the hovered or
focused row.

## 2. Applicability

- Candidate: a build that shows workspace tiles in the sidebar.
- Platforms: Linux.
- This procedure does not test the group **Needs you** of the sidebar.

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
   the sidebar with the caption `WORK ITEMS`, the count of the workspaces, and
   the icon controls **Fit all in view** and **New work item** at its right.

## 6. Tasks

### 6.1 TILES: Workspace rows

1. Look at the row of `billing-service`.

   Result: The row shows a tile with `BS` in the workspace color and the name in
   bold text. The row has no second line: it shows no text of the screen of its
   panel. The tile has no dot and no badge. The row shows no state word under its
   name while no panel waits and no orchestrator speaks.

2. Look at the row of `Checkout redesign`.

   Result: The tile shows `CR`, and it has no badge. The row shows the word
   `needs you` with a yellow dot at its right edge while an agent of the
   workspace asks a question. A row shows a second line only for a fact about
   its workspace: the count of the agents that wait, such as `1 needs you`, and
   the machine of the workspace, such as `on build-1`. At a window of 800 x 600
   that line is the same.

3. Look at the words of the state of all rows.

   Result: All words are in one column at the right edge of the rows. A word is
   `working` in the accent color, `needs you` in yellow, `failed` in red or
   `done` in the accent color, and each word has a dot of its color 5 points
   before it.

4. Look at the caption of the group.

   Result: The caption is `WORK ITEMS` in upper case with wide letter spacing.
   The count of the workspaces follows it. The icon controls **Fit all in view**
   and **New work item** stand at the right edge. The text buttons **New** and
   **Fit** are gone.

### 6.2 ACTIVE: Select a workspace

1. Click the row of `Horizon`.

   Result: The row takes a tint of the workspace color and a bar 3 points wide
   at the left edge of the sidebar. The panel rows of the row stay folded, and
   the chevron at the right end of the row points down.

2. Move the pointer on the last 24 points of the row of `Horizon` and click the
   chevron.

   Result: The chevron points down and four panel rows show under the row. A
   tree line connects them to the tile. A second press on the chevron folds them
   away, and the chevron points right again.

3. Look at the panel rows of the agents.

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

### 6.6 NARROW: A small window

1. Hold the pointer on a workspace name that ends with `…`, then on a second
   line that ends with `…`, then on a panel row that ends with `…`.

   Result: Each time, a tooltip shows the full text.

2. Right-click the row of `billing-service` and click **Open in New Window**.
   Look at its row in the main window.

   Result: The row shows the `NEW WINDOW` badge before the panel count.

3. Make the Horizon window 800 x 600 pixels. Click the row of `Horizon`.

   Result: At a window of 800 x 600 the sidebar is 210 pixels wide, as it is in
   a wide window. The caption shows `WORK ITEMS` whole, and the two icon
   controls stand at its right edge without a cover of the caption. Each panel
   row of a working agent shows the whole word `Working` after its name. A name
   that does not fit ends with `…` before that word, and the pointer on the row
   shows the whole name.

4. Hold the pointer on the row of `billing-service`.

   Result: The row shows the `NEW WINDOW` badge at its right end, left of the
   chevron of the fold control.

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

3. Move the focus to the icon controls of the caption of `WORK ITEMS`, and then
   to the link `Set up` of the footer.

   Result: The controls say `Fit all in view` and `New work item`. The link says
   `Set up`. The row `Front desk` says `Front desk`, and the chevron of a work
   item says `Show panels` or `Hide panels`.

### 6.8 PICTURES: The four pictures of the sidebar

Make each picture with `hz-look` from the root of the worktree, at the screen
size 2400 x 1100, with the orchestrator dock on and the stand-in program in
`HORIZON_ORCHESTRATOR_PROGRAM`. Give all actions of one picture in one call.

1. Make the picture `sb-desk` of the start.

   Command: `hz-look sb-desk --screen 2400x1100` with
   `--env HORIZON_ORCHESTRATOR_PROGRAM=<path of the stand-in>`.

   Result: The row `Front desk` is selected at the top of the sidebar. The
   captions `WORK ITEMS` and `MACHINES` show with their counts. One work item
   row shows no active mark, its panel rows are folded, and its state word stands
   under its name. The dashed control `+ Rent a machine` shows under the
   machine rows. The four chips of `AGENTS` show in two lines. The footer shows
   `Server: not set` and the link `Set up` at the bottom.

2. Make the picture `sb-work` of an active work item.

   Command: `hz-look sb-work --screen 2400x1100 --click <x>,<y> --type "question"
   --key Return`, where the click lands on the work item row.

   Result: The row of the work item has the tint and the bar of the active row,
   and its state word is `needs you`. The group `NEEDS YOU` with the count `1`
   stands above `WORK ITEMS`. The row `Front desk` is not selected.

3. Make the picture `sb-open` of an open fold.

   Command: `hz-look sb-open --screen 2400x1100 --click <x>,<y>`, where the click
   lands on the chevron at the right end of the work item row.

   Result: The panel rows of the work item show under its row. The chevron
   points down.

4. Make the picture `sb-light` in the light theme.

   Command: `hz-look sb-light --screen 2400x1100 --theme light --click <x>,<y>
   --type "question" --key Return`, with the same click as task 2.

   Result: The picture shows the same form as task 2 on the light surface. Each
   text is easy to read on its surface.

## 7. Pass criteria

- 6.1 shows a tile with initials on each row, no badge on the tile, and the
  state word under the name of a row that has a state.
- 6.2 shows the active tint and bar, the folded panel rows, and the chevron that
  opens and folds them.
- 6.3 shows the close button only on the hovered or focused row, and a tap on
  a hidden close button focuses the row and does not close the panel.
- 6.4 scrolls the list.
- 6.5 keeps the rows readable in the light theme.
- 6.6 shows the full text of a cut name, line or title on hover. At a window
  of 800 x 600 the sidebar keeps its width, its caption shows whole, and a panel
  row shows the whole word of its state.
- 6.7 gives each row, panel row, icon control and link a spoken name with the
  facts that the row shows.
- 6.8 shows the four pictures of the form of the sidebar, and each picture
  agrees with section 7 of the design guidelines.

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
