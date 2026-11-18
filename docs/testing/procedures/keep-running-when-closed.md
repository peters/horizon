---
procedure: keep-running-when-closed
feature: The close of the main window of Horizon, and the ask that counts the programs
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# The close of the main window test procedure

## 1. Purpose

This procedure proves what the close of the main window does with the three
values of the setting. It proves the ask of a close, the ask of "Quit Horizon",
the programs that the ask counts, and the lines that the ask shows.

## 2. Applicability

- Candidate: any build that has the setting `features.when_window_closes`.
- Platforms: Linux on X11, Linux on Wayland and macOS. The window of a Wayland
  session cannot hide, so the close of the window ends Horizon there and asks
  nothing.
- This procedure does not test: the desktop of a second computer, the shared
  terminals of a server, and the end of Horizon by a signal.

## 3. Safety

> **CAUTION:** THE STEPS THAT SELECT "END THEM AND QUIT", "QUIT" AND "QUIT
> HORIZON" END EVERY TERMINAL AND EVERY AGENT PROGRAM IN HORIZON. Do those steps
> only when the terminals hold no work that you need.

## 4. Equipment and preconditions

- A computer with a desktop, and a build of Horizon from this candidate.
- A second terminal window on the same desktop, to start Horizon again.
- A terminal panel in Horizon that runs a program, for example `sleep 600`.
- An agent panel that is in the working state, for task KR-2.
- A workspace that a machine runs, with one terminal panel in it, for task KR-2.

## 5. Setup

1. Start Horizon with a home that holds no setting of this feature.

   Result: The main window of Horizon shows.

2. Open Settings, and find the row "When the window is closed" under Features.

   Result: The row shows three values, and "Ask" has the mark.

3. Close the Settings window.

   Result: The board of Horizon shows.

## 6. Tasks

### 6.1 KR-1 — Baseline: "Ask" with no program

1. Close every panel of Horizon, and close the main window of Horizon.

   Result: Horizon ends, and no dialog shows.

2. Start Horizon again.

   Result: The main window of Horizon shows, and no panel is in it.

### 6.2 KR-2 — The ask of a close

1. Open the terminal panel with the program, the agent panel and the workspace of
   the machine.

   Result: The program runs, the agent works and the machine panel shows its
   terminal.

2. Close the main window of Horizon.

   Result: The window stays, and one dialog shows the heading "Close
   Horizon?".

3. Read the lines of the dialog.

   Result: One line counts the programs of this computer, for example "2
   programs are running. 1 agent is working." One line says how many panels run
   on a machine, for example "1 panel runs on a machine and keeps running
   there."

4. Examine the actions of the dialog.

   Result: The dialog shows "Keep running in the background" first, with the
   help line under it. It shows "End them and quit" in red, the action "Cancel",
   and the box "Remember this choice".

5. Close the main window of Horizon a second time.

   Result: The dialog changes nothing, and the counts stay as they were.

### 6.3 KR-3 — Enter keeps Horizon running

1. Press Enter.

   Result: The window goes away, and Horizon keeps running.

2. Read the desktop notification of Horizon.

   Result: One notification says that Horizon keeps running, that a start of
   Horizon shows the window, and that the Menu of Horizon ends it.

3. Start Horizon again from the second terminal window.

   Result: The window of Horizon shows and comes to the front. The program of
   the terminal panel runs.

4. Close the main window of Horizon a second time.

   Result: The window goes away, and Horizon sends no second notification.

5. Start Horizon again.

   Result: The window shows again.

### 6.4 KR-4 — "Cancel" and the escape key keep everything

1. Close the main window of Horizon, and select "Cancel".

   Result: The dialog goes away, and the window stays. The program of the
   terminal panel runs.

2. Close the main window of Horizon, and press the escape key.

   Result: The dialog goes away, and the window stays.

3. Close the main window of Horizon, and press the pointer outside the dialog.

   Result: The dialog goes away, and the window stays.

### 6.5 KR-5 — "Remember this choice"

1. Open Settings, and select "Ask".

   Result: The row "When the window is closed" shows the mark at "Ask".

2. Close the main window of Horizon, select the box "Remember this choice", and
   select "Keep running in the background".

   Result: The window goes away, and Horizon keeps running.

3. Start Horizon again, open Settings, and read the row "When the window is
   closed".

   Result: The mark is at "Keep Horizon running".

4. Close the main window of Horizon.

   Result: The window goes away at once, and no dialog shows.

### 6.6 KR-6 — "Keep Horizon running" never asks

1. Select "Keep Horizon running" in the row "When the window is closed".

   Result: The mark is at "Keep Horizon running".

2. Close the main window of Horizon with a program in a panel.

   Result: The window goes away at once, and no dialog shows.

3. Start Horizon again.

   Result: The window shows again.

### 6.7 KR-7 — "Quit Horizon" never asks

> **CAUTION:** THIS STEP ENDS EVERY TERMINAL AND AGENT PROGRAM THAT RUNS. Do it
> only when the terminals hold no work that you need.

1. Select "Quit Horizon" in the row "When the window is closed".

   Result: The mark is at "Quit Horizon".

2. Close the main window of Horizon with a program in a panel.

   Result: Horizon ends at once, and no dialog shows. The program of the panel
   ends.

3. Start Horizon again.

   Result: The window shows again.

### 6.8 KR-8 — The ask of "Quit Horizon"

> **CAUTION:** THE STEP THAT SELECTS "QUIT" ENDS EVERY TERMINAL AND AGENT
> PROGRAM. Do it only when the terminals hold no work that you need.

1. Open the terminal panel with the program, the agent panel and the workspace of
   the machine.

   Result: The program runs and the machine panel shows its terminal.

2. Open the Menu, and select "Quit Horizon".

   Result: One dialog shows the question "End 2 programs and quit?".

3. Read the lines of the dialog.

   Result: One line says that the agent programs that a restart keeps come back
   at the next start, and one line says how many panels run on a machine. This
   procedure needs the setting "Keep agent programs running when Horizon
   restarts" for the first line.

4. Select "Cancel".

   Result: The dialog goes away, and Horizon keeps running.

5. Open the Menu, select "Quit Horizon", and select "Quit".

   Result: Horizon ends, and the programs of the panels end.

### 6.9 KR-9 — A hidden window after a restart of the computer

1. Select "Keep Horizon running" in the row "When the window is closed".

   Result: The mark is at "Keep Horizon running".

2. Add a workspace with a name that you can read, for example `Kept`.

   Result: The board shows the workspace.

3. Close the main window of Horizon.

   Result: The window goes away, and Horizon keeps running.

4. Stop the computer, and start it again.

   Result: The computer starts with no window of Horizon.

5. Start Horizon.

   Result: The main window shows the workspace `Kept` as it was at the close.

### 6.10 KR-10 — The platforms

Run this task on each platform of section 2.

1. On Linux on X11: close the main window of Horizon with the mark at "Ask" and
   a program in a panel.

   Result: The dialog of task KR-2 shows.

2. On macOS: close the main window of Horizon with the mark at "Ask" and a
   program in a panel.

   Result: The dialog of task KR-2 shows.

3. On Linux on Wayland: close the main window with the mark at "Ask" and a
   program in a panel.

   Result: Horizon ends, and no dialog shows. This platform cannot hide a
   window.

## 7. Pass criteria

- Each step shows the result that the step names.
- The ask shows only when a program of this computer runs, or a panel runs on a
  machine, and the value of the setting is "Ask".
- The counts of the ask agree with the panels that are open, and a card, an
  editor and a browser panel are not in them.
- "Keep running in the background" hides the window and ends no program.
- "End them and quit" and "Quit" end the programs and Horizon.
- "Cancel", the escape key and a press outside the dialog end no program.
- The box "Remember this choice" writes "Keep Horizon running" or "Quit
  Horizon", and the next close does not ask.
- The workspace of the board is on the disk after the hide, and a restart of the
  computer writes no work off.

## 8. Cleanup

1. Select the value of the row "When the window is closed" that you want.

   Result: The setting keeps the value that you want.

2. Remove the workspace of task KR-9.

   Result: The board shows the workspaces that you need.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
