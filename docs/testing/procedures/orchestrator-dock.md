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
and the conversation that it runs. It proves that the dock draws only after its
setting turns it on, that the canvas ends at its left edge, that the minimap
stays on the canvas, that each workspace keeps its own draft and its own
conversation, and that a message of the person reaches an agent program with no
terminal and comes back as an answer. It also tests the two parts of the sidebar
that the dock adds: the group `Agents` and the button `+ Rent a machine`. It also proves the look of the dock: the field with its
control inside it, the example rows of the empty state, the state chip of the
header with the cost beside it, the request and the result of a tool row, the
gradient bar of an agent text, and the chevron of the three chips.

## 2. Applicability

- Candidate: a Horizon build that contains this change.
- Platforms: Linux, with the local device fixture.
- The agent program of this procedure is the stand-in
  `scripts/device-smoke/orchestrator_standin.py`. It answers in the line format
  of Claude Code and it starts no agent program. Section 6.12 uses a program that
  is not there.
- Tasks OD-17 to OD-19 read the stored conversation. A saved session keeps it in
  the folder of that session, so each of those tasks needs the state folder of
  the fixture and two starts of the candidate. Tasks OD-1 to OD-16 use a start
  that saves nothing, and such a start writes no conversation.

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
   `Orchestrator · Claude Code on this computer`. A chip stands at the right edge
   of the header, on the row of the name, with a hollow dot and the words
   `not connected`. There is no cost left of the chip.

3. Read the three parts of the dock from the top down.

   Result: The header is at the top. The empty state stands in the middle, with
   its title `Say what you want done.`, the two sentences under it and the two
   example rows under them. The composer is at the bottom edge of the dock.

4. Look at the composer.

   Result: The field takes the full width of the composer, which keeps about 16
   points of room at each side of the dock. The field is 48 points high. The hint
   `Write to the orchestrator` starts 14 points from the left edge of the field
   and 12 points under its top edge. The control is a square of 34 points inside
   the field at its right end, 7 points from its right inner edge, and its centre
   is the centre of the field. The control has a grey fill, a soft outline and a
   dim arrow, because the draft is empty. The three chips stand under the field,
   left aligned, 8 points apart, and their bottom edge is 14 points above the
   bottom edge of the dock. A small chevron points down 6 points right of the
   value of each chip.

5. Point at the control, and then at a chip.

   Result: The hover text of the control reads
   `The orchestrator is not connected yet`. The hover text of the chip reads
   `Choice comes later`, and the chip does not change.

6. Look at the canvas beside the dock.

   Result: The canvas ends at the left edge of the dock. No panel and no part of
   the canvas decoration is under the dock.

7. Look at the bottom right corner of the canvas.

   Result: The default window of the fixture is 1480 points wide, so the canvas
   beside the dock is about 870 points wide, which is less than the 996 points
   that the minimap needs. No minimap draws. Section 6.1 shows the minimap of a
   canvas that is wide enough.

8. Close every workspace, or open a new session with no workspace. Look at the
   header of the dock.

   Result: The header shows no tile. The name line reads `No work item is open`.
   There is no second line and no chip. The field is disabled, the control keeps
   the grey fill, the soft outline and the dim arrow, and its hover text reads
   `The orchestrator is not connected yet`.

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
   `Orchestrator · Claude Code on this computer` and the chip still reads
   `not connected`, because no message went out.

2. Select a second workspace in the sidebar. Click in the field of the dock.

   Result: The header shows the tile and the name of the second workspace. The
   field is empty, and it shows the hint `Write to the orchestrator`.

3. Type `a plan for the second workspace`. Select the first workspace again.

   Result: The field shows `a plan for the first workspace` again.

4. Type five more lines in the field, then five lines again.

   Result: The field grows one row for each line, and its top edge moves up. The
   control stays at the bottom right of the field, 7 points above its bottom inner
   edge. The chips keep their place at the bottom of the dock. After six rows the
   field stops growing and it clips the lines under the sixth row.

### 6.7 OD-6b — The example rows of the empty state

1. Start with an empty conversation and no workspace with a message. Read the
   two rows under the sentences of the empty state.

   Result: The first row reads `Fix #1987 on a rented machine`. The second row
   reads `Why is the cold start slow? Measure it first.` Each row takes the width
   of the empty state, 12 points under the sentences, and the rows are 8 points
   apart. Point at a row.

   Result: The outline of the row becomes stronger and the pointer is a pointing
   hand.

2. Click the first row.

   Result: The text of the row stands in the field and the outline of the field
   becomes the accent colour, because the field holds the keyboard. The control
   becomes a filled accent square with a dark arrow. No bubble and no answer
   appear, because nothing was sent, and the empty state stays.

3. Type one word in the field, then click the second row.

   Result: The row does nothing. The field keeps the word that the person typed.

### 6.8 OD-7 — Light and dark theme

1. Open **Menu › Settings**, and choose the light theme under Appearance.

   Result: The dock is a light surface. The tile, the name and the chips stay
   readable. The field, the send control and the chips show their outlines.

2. Choose the dark theme.

   Result: The dock is a dark surface, above the canvas surface. The tile, the
   name, the chip and the chevrons of the chips stay readable.

### 6.9 OD-8 — A message and the answer

1. Click in the field of the dock. Read the outline of the field, then type
   `hello there`. Press Enter.

   Result: The outline of the field is the accent colour while it holds the
   keyboard. The field is empty after the Enter. A bubble with `hello there`
   stands at the right half of the list, with a `PANEL_BG_ALT` fill and a soft
   outline. The chip of the header reads `working`, and then `ready` with a green
   dot. The second line of the header keeps
   `Orchestrator · Claude Code on this computer`. A text `You said: hello there`
   stands at the left half, with a bar of 3 points at its left. The top of the bar
   is the accent colour, and its bottom is the mauve of the workspace accents,
   because the bar is a vertical gradient. `0.01 USD` stands 10 points left of the
   chip, on its centre line.

2. Click in the field and press Enter without text.

   Result: No entry appears, and the conversation does not change.

3. Type `   ` and press Enter.

   Result: No entry appears. The white space stays in the field.

4. Type `one`, press Shift+Enter, type `two`, and press Enter.

   Result: The field is empty. One bubble holds two lines, `one` and `two`, and
   the answer to that message follows it.

### 6.10 OD-9 — A tool call

1. Type `use a tool please` and press Enter.

   Result: A row stands under the text of the agent, indented 13 points. It shows
   a green dot, the name `Bash` in the monospace font, and the command
   `echo stand-in` in the dim colour. The row shows no second line, because the
   call is done.

2. Type `fail please` and press Enter.

   Result: A row shows a red dot, the name `Bash` and the command `exit 3`. Under
   the command, in the same column, one red line reads `the command failed`. The
   second line of the result is not drawn.

### 6.11 OD-10 — A long answer and the scroll

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

### 6.12 OD-11 — Stop a turn

1. Type `slow answer please` and press Enter.

   Result: The chip of the header reads `working`. A row with the spinner and
   `working` stands at the left of the list. The control of the composer shows a
   filled square, with a grey fill, a strong outline and the square in the
   foreground colour.

2. Press the control before the answer comes. The answer of this turn comes after
   2 seconds.

   Result: A notice `Stopped by you` stands in the middle of the list, with a line
   at each side. The chip of the header reads `stopped`, with a hollow dot. No
   answer of that turn appears. The field keeps the keys, and the control shows
   the arrow again. While the stop waits, the pointer of the person still moves
   the dock and a click on an example row still fills an empty draft: the stop
   does not hold the frame, and it does not hold a browser job.

3. Type `after the stop` and press Enter.

   Result: A new turn starts. The answer arrives, and the chip of the header
   returns to `ready`.

### 6.13 OD-12 — A program that does not start

1. Stop the fixture. Start it again with the same arguments, and give the
   variable a path that is not a program:

   ```sh
   HORIZON_ORCHESTRATOR_PROGRAM=/nonexistent/claude-stand-in
   ```

   Use `--state <evidence>/dock-missing`.

2. Click in the field, type `hello there`, and press Enter.

   Result: A notice in the middle of the list reads
   `Claude Code did not start: No such file or directory (os error 2)`. The chip
   of the header reads `ended`, with a red dot. The control shows the arrow
   again.

3. Send one more message.

   Result: Horizon tries to start the program again. The notice appears once
   more, and no other entry is added.

### 6.14 OD-13 — Two workspaces

1. Send `a question of the first workspace` in the first workspace.

2. Select the second workspace in the sidebar. Send `a question of the second
   workspace` there. Select the first workspace again.

   Result: The dock shows the conversation of the first workspace, with its
   message and its answer, and no entry of the second workspace. The chip of the
   header reads the state of the first conversation.

3. Select the second workspace.

   Result: The dock shows the conversation of the second workspace only. Both
   conversations keep their entries.

4. Send a message in the second workspace, select the first workspace before the
   answer comes, and wait 5 seconds. Select the second workspace again.

   Result: The answer of the second conversation arrived while the first
   workspace was active. The dock shows it.

### 6.15 OD-14 — The end of Horizon leaves no program

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

### 6.15 OD-15 — The orchestrator draws a card on the canvas

This task needs a screen that is wide enough for the card and the dock beside the
panels of the fixture.

1. Stop the fixture and start it again with the stand-in, the switch of the dock
   and a screen of 2400 by 1100 points, and use `--state <evidence>/dock-card`.

   ```sh
   HORIZON_ORCHESTRATOR_DOCK=1 \
   HORIZON_ORCHESTRATOR_PROGRAM=<repo>/scripts/device-smoke/orchestrator_standin.py \
     python3 scripts/device-smoke/serve.py \
       --horizon <candidate>/horizon --native-view --screen 2400x1100 \
       --state <evidence>/dock-card
   ```

2. Open a Device panel on the address of the fixture in the current workspace.

3. Click in the field of the dock, type `draw a card`, and press Enter.

   Result: The list of the dock shows the bubble `draw a card`, the text
   `You said: draw a card`, and one text that names the id of a new card, such as
   `I drew the card 124cef72-8cb1-483b-9665-ec8cc155c737 in this workspace. Press
   Go or Cancel and I read it.`. The header reads
   `Orchestrator · Claude Code · ready`. The sidebar of the workspace lists a row
   with the mark `CARD` and the name `Plan`.

4. Click the row `Plan` in the sidebar.

   Result: The canvas moves to the card. The card holds the title `Plan`, heading
   `Plan`, the subtitle `drawn by the stand-in orchestrator`, one line of text,
   the button `Go` in the primary tone, the button `Cancel`, and the foot line
   `Card drawn by an agent` with the time. The state chip of the title bar reads
   `Claude listens`, with a green dot. The card covers no terminal panel of the
   workspace.

5. Take a picture of the whole desktop and keep it as the evidence of this task.

   Result: The picture shows the card on the canvas of the workspace of the
   conversation, the chip `Claude listens`, and the two texts of the dock.

### 6.16 OD-16 — A press reaches the orchestrator

The program of the dock reads a press only through the `next_click` that it keeps
open, because Horizon types no wake line for an owner without a terminal.

1. Do the steps 1 to 4 of OD-15.

2. Click the button `Go` of the card.

   Result: The list of the dock shows one more text of the agent,
   `You pressed go`, and the cost of the header grows by one turn. The state chip
   of the card reads `Claude`, without the dot of a wait, because the one
   `next_click` of the program ended with that press.

3. Take a picture of the whole desktop and keep it as the evidence of this task.

   Result: The picture shows the card, the text `You pressed go` in the dock, and
   the chip `Claude`.

4. Click the button `Cancel`, and wait for the wait of the program to end.

   Result: The dock adds no text, because the program holds no `next_click` any
   more. The press waits in the queue of the card for five minutes.

### 6.17 OD-17 — The setting of Settings turns the dock on

1. Stop the fixture. Start it again with the same arguments, but without
   `HORIZON_ORCHESTRATOR_DOCK=1`, and use `--state <evidence>/dock-setting`.

   Result: The Horizon window shows no column at its right edge, and the sidebar
   shows no group `Agents` and no button `+ Rent a machine`.

2. Open **Menu › Settings** and read the tab General, section Features.

   Result: The section holds the check box **Orchestrator dock** with the line
   `A conversation with one agent at the right edge of the window. It does the
   work of the workspace and shows it as cards. Experimental.` under it. The check
   box is clear.

3. Press the check box **Orchestrator dock**.

   Result: The dock stands at the right edge of the window without a restart, and
   the sidebar shows the group `Agents` and the button `+ Rent a machine`. The
   canvas ends at the left edge of the dock.

4. Press the check box again.

   Result: The dock goes, and the group and the button go with it. The canvas
   reaches the right edge of the window.

5. Press the check box once more, and restart Horizon with the same arguments.

   Result: The dock stands at the right edge of the window after the restart,
   because the setting is saved.

6. Press the chord of the command palette, type `orchestrator dock` and press
   Enter. Then close the Settings window, open it again and read the check box.

   Result: The dock goes, and the check box is still clear or still checked as
   step 5 left it: the palette turns the dock of this run over and writes no
   setting.

> **CAUTION:** THE NEXT STEP CHANGES SETTINGS. TURN THE SETTING OFF AFTER IT.

7. Clear the check box **Orchestrator dock** and close the Settings window.

   Result: The window holds no dock.

### 6.18 OD-18 — The group Agents

1. Start the run of OD-17 step 3, so that the dock is on, and look at the sidebar
   under the section Machines.

   Result: The header `Agents` stands 18 points from the left edge of the sidebar,
   and the count `4` follows it in the monospace font. Four rows stand under the
   header, 34 points each, in this order: Claude, Codex, Pi, Gemini.

2. Read each row.

   Result: A state dot of 8 points stands at the left of the row. The dot is
   filled green when the program of that row is on the `PATH` of the fixture, and
   the line under the name reads `on this computer`. The dot is hollow when the
   program is not there, the name is dim, and the line reads
   `not found on this computer`. On a computer without Claude Code, the row
   `Claude` shows the hollow dot.

3. Point at a row, and press it.

   Result: The row shows no card and no hover text, the pointer stays an arrow,
   and the press does nothing.

4. Start a computer where none of the four programs is on the `PATH`. Start the
   fixture with the setting on.

   Result: Every row says `looking` with a hollow dot for the first frame of the
   window, and then the four rows say `not found on this computer`.

### 6.19 OD-19 — The button + Rent a machine

1. Read the end of the section Machines of the run of OD-18.

   Result: The button `+ Rent a machine` stands under the rows of the machines,
   or under the line `No machines yet. Set up rents one, or adds the Horizon
   server.` when the list holds no machine. Its fill is the panel surface, it has
   a 1 px outline and radius 10, it is 30 points high, and it keeps 12 points of
   room at each side of the sidebar. The label is centred.

2. Point at the button.

   Result: The outline of the button becomes stronger and the pointer is a
   pointing hand. The fill does not change.

3. Press the button.

   Result: The dock of the active workspace shows the bubble
   `Rent a machine for this workspace. Show me the card with the type, the place and the price first.`
   The chip of the header of the dock reads `working`, and the answer of the
   stand-in follows it. The board opens no panel.

4. Press the button again while the chip of the header reads `working`.

   Result: The pointer shows a pointing hand, the hover text reads
   `The orchestrator works`, and the press adds no message to the dock.

5. Close every workspace of the window, and read the button.

   Result: The label is dim, and the hover text reads `Open a workspace first`. A
   press adds no message.

6. Clear the check box **Orchestrator dock** in Settings, and read the end of the
   section Machines.

   Result: The button is gone.
### 6.17 OD-17 — A restart shows the conversation and continues its session

This task needs a record of the arguments of the program, because the dock
passes the stored session id to the program as one argument.

1. Write the wrapper of the stand-in into the evidence folder. It writes its
   arguments to a log file and then runs the stand-in.

   ```sh
   cat > <evidence>/orchestrator-argv.sh <<'EOF'
   #!/bin/sh
   printf '%s\n' "$@" >> "<evidence>/orchestrator-argv.log"
   exec python3 <repo>/scripts/device-smoke/orchestrator_standin.py "$@"
   EOF
   chmod +x <evidence>/orchestrator-argv.sh
   ```

   Result: The file is executable and the log file does not exist yet.

2. Start the fixture in the state folder `<evidence>/dock-stored`, with
   `HORIZON_ORCHESTRATOR_PROGRAM=<evidence>/orchestrator-argv.sh`. Give
   `serve.py` the two options `--reuse-state` and `--saved-session`, so the
   candidate saves its session in the private home.

   Result: The window opens, with the dock at the right edge.

3. Write `alpha` in the field of the dock and press Enter.

   Result: The list shows the bubble `You said: alpha` and the answer of the
   stand-in. The log file holds one line that reads `--session-id`, followed by
   one line that holds a UUID.

4. Close the window of the candidate in the usual way, and stop the fixture with
   Ctrl-C.

   Result: Horizon writes the board and ends. The state folder holds a session
   folder with a `conversations` folder below it, at
   `<state>/data/home/.horizon/sessions/<session id>/conversations/`.

5. Start the fixture again on the same state folder, with `--reuse-state`,
   `--resume-session` and the same `HORIZON_ORCHESTRATOR_PROGRAM`. Do not give
   `--saved-session` here.

   Result: The window opens. The dock shows the bubble `You said: alpha` and the
   answer of the stand-in above the empty space, and the state chip of the header
   reads `not connected`.

6. Write `bravo` in the field of the dock and press Enter.

   Result: The list shows the bubble `You said: bravo` below the entries of the
   earlier run, and the answer of the stand-in. The log file holds one line that
   reads `--resume`, followed by the UUID that the first start named in step 3.

7. Take a picture of the whole desktop and keep it as the evidence of this task.

   Result: The picture shows the entries of both runs in one conversation.

### 6.18 OD-18 — A start that saves nothing stores nothing

1. Start the fixture in a new state folder, without `--saved-session`, and with
   the stand-in as `HORIZON_ORCHESTRATOR_PROGRAM`. Such a start is ephemeral.

   Result: The window opens with one workspace, and the dock shows the empty
   state.

2. Write `alpha` in the field of the dock and press Enter.

   Result: The list shows the bubble and the answer of the stand-in.

3. Stop the fixture with Ctrl-C, and read the state folder.

   ```sh
   find <state> -name '*.jsonl' -o -name conversations -type d
   ```

   Result: The command prints no path. An unsaved start writes no conversation
   file and no record of a session.

### 6.19 OD-19 — A removed workspace removes its conversation

1. Do the steps 1 to 3 of OD-17, so that the workspace keeps a conversation.

2. Read the folder of the conversations of the session.

   ```sh
   ls <state>/data/home/.horizon/sessions/*/conversations
   ```

   Result: The folder holds one file that ends with `.jsonl`.

3. Remove the workspace through its row in the sidebar, and confirm the removal.

   Result: The workspace and its dock go, and the folder of the conversations of
   the session shows no file of that workspace after a moment.

4. Stop the fixture with Ctrl-C, and read the folder again.

   Result: The folder still holds no file of that workspace. The folder of the
   session goes whole when the person deletes that saved session, so this file
   leaves nothing behind.

## 7. Pass criteria

- The dock draws only after the switch turns it on.
- The header is at the top of the dock, the empty state is in the middle, and the
  composer keeps the bottom edge.
- The field takes the full width of the composer and stands at its bottom edge.
  The control stands inside the field at its right end, and the three chips stand
  under the field.
- The control shows the accent form only while a draft can go out. It shows the
  grey form while the draft is empty or there is no workspace, and the stop form
  while a turn runs.
- The empty state draws the title, the two sentences and the two example rows. A
  click on a row fills an empty draft and sends nothing.
- The header draws the state in a chip at its right edge, and the cost left of the
  chip on the same centre line. The second line names the program and no state.
- A tool row draws the request of the tool. A failed call draws the first line of
  its result under the request, and a call that is done draws no such line.
- The bar of an agent text is a vertical gradient from the accent to the mauve of
  the workspace accents.
- Each of the three chips shows a chevron and the hover text `Choice comes later`.
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
- A message that holds `card` draws one card of the stand-in in the workspace of
  the conversation, and the card holds the heading, the subtitle, the text and the
  two buttons of the stand-in.
- A card of an orchestrator stands on a free tile of the canvas and covers no
  panel. Its state chip reads `Claude listens` while the program holds a
  `next_click` open, and `Claude` after the wait ends.
- A press on a button of the card reaches the program of the dock, and the dock
  shows the text that names the button.
- The setting **Orchestrator dock** turns the dock on and off without a restart,
  and it is saved. The palette turns the dock of one run over and writes no
  setting.
- The group `Agents` stands under the machine section only while the dock is on.
  It draws four rows in the order Claude, Codex, Pi, Gemini, and each row says
  whether this computer has that program.
- The button `+ Rent a machine` stands at the end of the machine section only
  while the dock is on. One press sends one message to the orchestrator of the
  active workspace, and the button sends nothing without an active workspace and
  while that orchestrator works.
- A restart of Horizon on a saved session shows the entries of the earlier run,
  and the state chip reads `not connected` above them.
- The first message after that restart starts the program with `--resume` and the
  session id of the earlier run.
- A program that does not continue the stored session leaves the notice `The
  earlier session could not continue. A new one starts with your next message.`,
  and the message after it starts a program with `--session-id`.
- A start that saves nothing writes no conversation file and no record of a
  session.
- A workspace that is removed takes its conversation file with it, and the delete
  of a saved session takes the folder of its conversations with the rest of the
  session folder.

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
