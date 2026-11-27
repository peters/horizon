---
procedure: orchestrator-dock
feature: Orchestrator dock
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Orchestrator dock test procedure

## 1. Purpose

This procedure tests the frame of the orchestrator dock at the right edge of the
root window. It proves that the dock draws only after the switch turns it on,
that the canvas ends at its left edge, that the minimap stays on the canvas, and
that each workspace keeps its own draft. No agent, process or message is part of
this part.

## 2. Applicability

- Candidate: a Horizon build that contains this change.
- Platforms: Linux, with the local device fixture.
- This procedure does not test a message to an orchestrator. The send control is
  disabled in this part.

## 3. Safety

> **CAUTION:** USE ONLY THE ISOLATED DESKTOP. A click on the developer desktop changes a real board.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The local device fixture with `--native-view`.
- A live Device panel in the current workspace.
- An evidence folder for the run.

## 5. Setup

1. Copy the candidate to a task-owned folder. Write down its SHA-256.

   Result: The candidate does not change during the run.

2. Start the fixture with the switch of the dock on.

   ```sh
   HORIZON_ORCHESTRATOR_DOCK=1 python3 scripts/device-smoke/serve.py \
     --horizon <candidate>/horizon --native-view --state <evidence>/dock-on
   ```

   The fixture keeps the environment variables of the shell that starts it, and
   it removes only `HORIZON`, `HORIZON_HOME`, `HORIZON_BROWSER_*` and
   `HORIZON_WORK_*` (`scripts/device-smoke/sandbox.py`). It gives the candidate
   the rest of that environment (`scripts/device-smoke/serve.py`).

   Result: The manifest prints an address for the native view, and the dock
   stands at the right edge of the Horizon window.

3. Open a Device panel on that address in the current workspace.

   Result: The live view shows the isolated desktop. The frame sequence advances.

## 6. Tasks

### 6.1 OD-1 — The dock is off without the switch

1. Stop the fixture. Start it again with the same arguments, but without
   `HORIZON_ORCHESTRATOR_DOCK=1`, and use `--state <evidence>/dock-off`.

   Result: The Horizon window shows no column at its right edge.

2. Look at the canvas and at the minimap.

   Result: The canvas reaches the right edge of the window. The minimap stands in
   the bottom right corner of the canvas.

### 6.2 OD-2 — The dock on the right edge

1. Look at the window of the run that holds the switch, `<evidence>/dock-on`.

   Result: A column stands at the right edge of the window. It starts under the
   toolbar and it ends at the bottom of the window. A soft line is at its left
   edge. A soft line is under its header and a soft line is above its composer.

2. Read the header.

   Result: The tile of the active workspace is on the left. The name of that
   workspace is beside it. The line under the name reads
   `Orchestrator · not connected`.

3. Read the three parts of the dock from the top down.

   Result: The header is at the top. The empty state stands in the middle, with
   its title `Say what you want done.` and the two sentences under it. The
   composer is at the bottom edge of the dock.

4. Look at the composer.

   Result: The field is at the left, 16 points from the left edge of the dock. The
   hint `Write to the orchestrator` starts 14 points from the left edge of the
   field and 12 points under its top edge. The send control is a square of 32
   points at the right of the field, and its centre is the centre of the field.
   The three chips stand under the field, left aligned, 8 points apart, and their
   bottom edge is 14 points above the bottom edge of the dock.

5. Look at the canvas beside the dock.

   Result: The canvas ends at the left edge of the dock. No panel and no part of
   the canvas decoration is under the dock.

6. Look at the bottom right corner of the canvas.

   Result: The default window of the fixture is 1480 points wide, so the canvas
   beside the dock is about 870 points wide, which is less than the 996 points
   that the minimap needs. No minimap draws. Section 6.1 shows the minimap of a
   canvas that is wide enough.

7. Close every workspace, or open a new session with no workspace. Look at the
   header of the dock.

   Result: The header shows no tile. The name line reads `No work item is open`.
   There is no second line. The field is disabled.

### 6.3 OD-3 — The command palette toggles the dock

1. Press the command palette chord of the settings, type `orchestrator dock`, and
   press Enter.

   Result: The dock is gone. The canvas reaches the right edge of the window.

2. Do step 1 again.

   Result: The dock is back at the right edge of the window.

3. Type text in a terminal panel of the workspace, before and after step 2.

   Result: Every key that the person types reaches the terminal panel. The dock
   does not take the keyboard when it appears.

### 6.4 OD-4 — A narrow window

1. Resize the Horizon window to about 800 points wide with the window manager.

   Result: The dock is gone. The canvas keeps the whole window.

2. Widen the window to about 1600 points, then to about 2000 points.

   Result: The dock is back at each step. Its width is 432 points at 1600 and 540
   points at 2000, so the dock is wider at 2000.

3. Look at the minimap at each width.

   Result: The canvas beside the dock of a window of 1600 points is 958 points
   wide, so no minimap draws. The canvas of a window of 2000 points is 1250
   points wide, so the minimap draws in its bottom right corner.

### 6.5 OD-5 — The Settings window beside the dock

1. Open **Menu › Settings** in the toolbar.

   Result: The Settings panel stands to the left of the dock. The dock keeps the
   right edge of the window. The canvas ends at the left edge of the Settings
   panel.

2. Read the header of the dock and the rows of the Settings panel.

   Result: The name and the tile of the active workspace stay in the header. No
   row of the Settings panel is under the dock.

3. Close the Settings window.

   Result: The canvas ends at the left edge of the dock.

### 6.6 OD-6 — A draft for each workspace

1. Click in the field of the dock. Type `a plan for the first workspace`.

   Result: The text stands in the field at the bottom of the dock. No character
   goes to a terminal panel. The send control stays disabled, and its hover text
   reads `The orchestrator is not connected yet`.

2. Select a second workspace in the sidebar. Click in the field of the dock.

   Result: The header shows the tile and the name of the second workspace. The
   field is empty, and it shows the hint `Write to the orchestrator`.

3. Type `a plan for the second workspace`. Select the first workspace again.

   Result: The field shows `a plan for the first workspace` again.

4. Type five more lines in the field, then five lines again.

   Result: The field grows one row for each line, and its top edge moves up. The
   send control keeps its centre on the centre of the first row. The chips keep
   their place at the bottom of the dock. After six rows the field stops growing
   and it clips the lines under the sixth row.

### 6.7 OD-7 — Light and dark theme

1. Open **Menu › Settings**, and choose the light theme under Appearance.

   Result: The dock is a light surface. The tile, the name and the chips stay
   readable. The field, the send control and the chips show their outlines.

2. Choose the dark theme.

   Result: The dock is a dark surface, above the canvas surface. The tile, the
   name and the chips stay readable.

## 7. Pass criteria

- The dock draws only after the switch turns it on.
- The header is at the top of the dock, the empty state is in the middle, and the
  composer keeps the bottom edge.
- The field stands at the bottom of the dock, the send control stands on the row
  of the field at its right, and the three chips stand under the field.
- The canvas ends at the left edge of the dock, and at the left edge of the
  Settings panel when both show.
- The minimap keeps the rule of the minimap: it draws only in a canvas of at
  least 996 by 384 points. It stands on the canvas and never on the dock.
- A terminal panel keeps the keyboard when the dock appears, and the field takes
  the keyboard only after a click in it.
- Each workspace keeps its own draft.
- The dock draws nothing in a window where the canvas would keep less than 320
  points.
- The text of the dock is readable in the light theme and in the dark theme.

## 8. Cleanup

1. Close the Device panel of this run through the `device_panel` tool.

   Result: The viewer connection ends. The fixture and the candidate keep
   running.

2. Stop the fixture with Ctrl-C in its terminal.

   Result: The isolated desktop, the candidate and the private state end. The
   developer desktop and the developer Horizon are unchanged.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
