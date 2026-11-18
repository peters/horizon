---
procedure: keep-running-when-closed
feature: The main window of Horizon when the setting keeps Horizon running
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Keep Horizon running when its window is closed test procedure

## 1. Purpose

This procedure proves that Horizon keeps its terminals, its agent programs and
its links when the person closes the main window, that a second start shows the
window again, and that a person can end Horizon.

## 2. Applicability

- Candidate: any build that has the setting `features.keep_running_when_closed`.
- Platforms: Linux on X11, Linux on Wayland and macOS. The window of a Wayland
  session cannot hide, so the close of the window ends Horizon there.
- This procedure does not test: the desktop of a second computer, the shared
  terminals of a server, and the end of Horizon by a signal.

## 3. Safety

> **CAUTION:** THE STEP THAT ENDS HORIZON ENDS EVERY TERMINAL AND EVERY AGENT
> PROGRAM IN IT. Do that step only when the terminals hold no work that you need.

## 4. Equipment and preconditions

- A computer with a desktop, and a build of Horizon from this candidate.
- A second terminal window on the same desktop, to start Horizon again.
- A terminal panel in Horizon that runs a program, for example `sleep 600`.

## 5. Setup

1. Start Horizon with a home that holds no setting of this feature.

   Result: The main window of Horizon shows.

2. Open Settings, and find the row "Keep Horizon running when its window is
   closed" under Features.

   Result: The row is not selected.

3. Select the row.

   Result: The row shows a mark, and the sentence under it says how to show the
   window again and how to end Horizon.

4. Close the Settings window.

   Result: The board of Horizon shows.

## 6. Tasks

### 6.1 KR-1 — Baseline: the setting off

1. Open Settings, and clear the row "Keep Horizon running when its window is
   closed".

   Result: The row shows no mark.

2. Close the Settings window, and close the main window of Horizon.

   Result: Horizon ends. The terminal panel of the program ends.

3. Start Horizon again.

   Result: The main window of Horizon shows, and the program of the terminal
   panel is gone.

### 6.2 KR-2 — Hide the window

1. Open Settings, select the row "Keep Horizon running when its window is
   closed", and close the Settings window.

   Result: The board of Horizon shows.

2. Note the text in the terminal panel of the program.

3. Close the main window of Horizon.

   Result: The window of Horizon goes away, and Horizon keeps running.

4. Read the desktop notification of Horizon.

   Result: One notification says that Horizon keeps running, that a start of
   Horizon shows the window, and that the Menu of Horizon ends it.

5. Start Horizon again from the second terminal window.

   Result: The window of Horizon shows and comes to the front. The second
   terminal window prints one line, "Horizon shows its window.", and no second
   Horizon opens.

6. Look at the terminal panel of the program.

   Result: The text of the panel is as it was in step 2, and the program runs.

### 6.3 KR-3 — One notification for each start

1. Close the main window of Horizon a second time.

   Result: The window goes away, and Horizon sends no second notification.

2. Start Horizon again.

   Result: The window shows again.

### 6.4 KR-4 — End Horizon from the Menu

> **CAUTION:** THIS STEP ENDS EVERY TERMINAL AND AGENT PROGRAM IN HORIZON. Do it
> only when the terminals hold no work that you need.

1. Open the Menu, and select "Quit Horizon".

   Result: Horizon ends, and the program of the terminal panel ends.

2. Start Horizon again.

   Result: The main window shows, and the program of the terminal panel is gone.

### 6.5 KR-5 — A hidden window after a restart of the computer

1. Select the row "Keep Horizon running when its window is closed".

   Result: The row shows a mark.

2. Add a workspace with a name that you can read, for example `Kept`.

3. Close the main window of Horizon.

   Result: The window goes away, and Horizon keeps running.

4. Stop the computer, and start it again.

   Result: The computer starts with no window of Horizon.

5. Start Horizon.

   Result: The main window shows the workspace `Kept` as it was at the close.

### 6.6 KR-6 — The platforms

Run this task on each platform of section 2.

1. On Linux on X11: close the main window of Horizon with the setting on.

   Result: The window goes away, and Horizon keeps running.

2. On macOS: close the main window of Horizon with the setting on.

   Result: The window goes away, and Horizon keeps running.

3. On Linux on Wayland: close the main window of Horizon with the setting on.

   Result: Horizon ends, as it does without the setting. This platform cannot
   hide a window.

## 7. Pass criteria

- Each step shows the result that the step names.
- A close of the window with the setting on ends no terminal and no agent
  program.
- A second start of Horizon shows the window of the run that keeps running, and
  prints one line.
- The workspace of the board is on the disk after the hide, and a restart of the
  computer writes no work off.
- "Quit Horizon" ends Horizon with the setting on and with the setting off.

## 8. Cleanup

1. Select the row "Keep Horizon running when its window is closed" only when you
   want the behaviour of this candidate.

   Result: The setting keeps the value that you want.

2. Remove the workspace of step 6.5.

   Result: The board shows the workspaces that you need.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
