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

This procedure tests the orchestrator dock at the right edge of the root window,
and the conversation that it runs. It proves that the dock draws only after the
switch turns it on, that the canvas ends at its left edge, that the minimap stays
on the canvas, that each workspace keeps its own draft and its own conversation,
and that a message of the person reaches an agent program with no terminal and
comes back as an answer.

## 2. Applicability

- Candidate: a Horizon build that contains this change.
- Platforms: Linux, with the local device fixture.
- The agent program of this procedure is the stand-in
  `scripts/device-smoke/orchestrator_standin.py`. It answers in the line format
  of Claude Code and it starts no agent program. Section 6.12 uses a program that
  is not there.
- A conversation is not saved. A restart of Horizon starts an empty
  conversation.

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

2. Start the fixture with the switch of the dock on and the stand-in as the
   program of the orchestrator.

   ```sh
   HORIZON_ORCHESTRATOR_DOCK=1 \
   HORIZON_ORCHESTRATOR_PROGRAM=<repo>/scripts/device-smoke/orchestrator_standin.py \
     python3 scripts/device-smoke/serve.py \
       --horizon <candidate>/horizon --native-view --state <evidence>/dock-on
   ```

   The fixture keeps the environment variables of the shell that starts it, and
   it removes only `HORIZON`, `HORIZON_HOME`, `HORIZON_BROWSER_*` and
   `HORIZON_WORK_*` (`scripts/device-smoke/sandbox.py`). It gives the candidate
   the rest of that environment (`scripts/device-smoke/serve.py`). A value of
   `HORIZON_ORCHESTRATOR_PROGRAM` that names the stand-in of the repository is
   replaced by a copy in the state folder of the run, because the sandbox hides a
   checkout under the home of the person.

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
   `Orchestrator · Claude Code · not connected`. There is no cost at the right
   edge of the header.

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
   There is no second line. The field is disabled, and the hover text of the
   control reads `The orchestrator is not connected yet`.

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
   goes to a terminal panel. The second line of the header still reads
   `Orchestrator · Claude Code · not connected`, because no message went out.

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

### 6.8 OD-8 — A message and the answer

1. Click in the field of the dock. Type `hello there`. Press Enter.

   Result: The field is empty. A bubble with `hello there` stands at the right
   half of the list, with a `PANEL_BG_ALT` fill and a soft outline. The second
   line of the header reads `Orchestrator · Claude Code · working`, and then
   `ready`. A text `You said: hello there` stands at the left half, with a bar of
   3 points of the accent colour at its left. `0.01 USD` stands at the right edge
   of the header.

2. Click in the field and press Enter without text.

   Result: No entry appears, and the conversation does not change.

3. Type `   ` and press Enter.

   Result: No entry appears. The white space stays in the field.

4. Type `one`, press Shift+Enter, type `two`, and press Enter.

   Result: The field is empty. One bubble holds two lines, `one` and `two`, and
   the answer to that message follows it.

### 6.9 OD-9 — A tool call

1. Type `use a tool please` and press Enter.

   Result: A row stands under the text of the agent, indented 13 points. It shows
   a green dot, the name `Bash` in the monospace font, and the summary
   `stand-in` in the dim colour.

### 6.10 OD-10 — A long answer and the scroll

1. Type `long answer please` and press Enter. Then type `and again` and press
   Enter.

   Result: The list shows its bottom. The second bubble `and again` and its answer
   stand at the bottom. The last lines of the answer of 40 numbered lines stand
   above them. The field is empty.

2. Turn the wheel of the pointer up over the list, and read the lines of the long
   answer. Send one more message.

   Result: The list keeps its place. A new entry does not pull the list to the
   bottom while the person reads above it. Turn the wheel down to the bottom, and
   send one more message. The list follows the bottom again.

### 6.11 OD-11 — Stop a turn

1. Type `slow answer please` and press Enter.

   Result: The second line of the header reads `Orchestrator · Claude Code ·
   working`. A row with the spinner and `working` stands at the left of the list.
   The send control of the composer shows a filled square.

2. Press the control before the answer comes. The answer of this turn comes after
   2 seconds.

   Result: A notice `Stopped by you` stands in the middle of the list, with a line
   at each side. The second line of the header reads `Orchestrator · Claude Code ·
   stopped`. No answer of that turn appears. The field keeps the keys, and the
   control shows the arrow again.

3. Type `after the stop` and press Enter.

   Result: A new turn starts. The answer arrives, and the second line of the
   header returns to `ready`.

### 6.12 OD-12 — A program that does not start

1. Stop the fixture. Start it again with the same arguments, and give the
   variable a path that is not a program:

   ```sh
   HORIZON_ORCHESTRATOR_PROGRAM=/nonexistent/claude-stand-in
   ```

   Use `--state <evidence>/dock-missing`.

2. Click in the field, type `hello there`, and press Enter.

   Result: A notice in the middle of the list reads
   `Claude Code did not start: No such file or directory (os error 2)`. The second
   line of the header reads `Orchestrator · Claude Code · ended`. The control
   shows the arrow again.

3. Send one more message.

   Result: Horizon tries to start the program again. The notice appears once
   more, and no other entry is added.

### 6.13 OD-13 — Two workspaces

1. Send `a question of the first workspace` in the first workspace.

2. Select the second workspace in the sidebar. Send `a question of the second
   workspace` there. Select the first workspace again.

   Result: The dock shows the conversation of the first workspace, with its
   message and its answer, and no entry of the second workspace. The second line
   of the header reads the state of the first conversation.

3. Select the second workspace.

   Result: The dock shows the conversation of the second workspace only. Both
   conversations keep their entries.

4. Send a message in the second workspace, select the first workspace before the
   answer comes, and wait 5 seconds. Select the second workspace again.

   Result: The answer of the second conversation arrived while the first
   workspace was active. The dock shows it.

### 6.14 OD-14 — The end of Horizon leaves no program

1. Send one message and wait for the answer.

2. In a terminal of this computer, run this command:

   ```sh
   pgrep -af orchestrator_standin
   ```

   Result: One line names the program of the conversation.

3. Close the window of Horizon. The fixture prints the end of its run.

4. Run the command of step 2 again.

   Result: No line names the program. Horizon ended every program of the dock
   before it exited.

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
- Each workspace keeps its own draft and its own conversation.
- A message of the person reaches the program and its answer shows in the list:
  the bubble of the person at the right, the text of the agent at the left and
  the row of a tool at the left, indented.
- The header names the state of the conversation and the cost of its ended
  turns.
- The list follows the bottom until the person scrolls up.
- The control stops a turn, and the next message starts a new one.
- A program that does not start leaves a notice, and the next message tries
  again.
- No program of a conversation outlives Horizon.
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
