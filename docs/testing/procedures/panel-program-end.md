---
procedure: panel-program-end
feature: A terminal panel whose program ended
platforms: [linux, darwin]
cost: none
destructive: no
secrets: none
owner: peters
---

# A terminal panel whose program ended test procedure

## 1. Purpose

This procedure tests a terminal panel whose program ended. It tests the line
that the panel shows, the program that starts again in the same panel, the text
that no program takes, the control that closes the panel, and the rule that
Horizon starts nothing again and closes nothing by itself.

## 2. Applicability

- Use a current Horizon candidate on Linux or macOS.
- Use shell panels of this computer only.
- This procedure does not test an SSH panel, which reports a lost link, and it
  does not test a browser panel, a Device panel or a card panel.

## 3. Safety

> **CAUTION: TYPE ONLY THE COMMANDS OF THIS PROCEDURE.** A shell panel of this
> run runs on this computer with your rights.

Use a private state folder of this run and synthetic output only.

## 4. Equipment and preconditions

- A frozen candidate `horizon` binary and its SHA-256.
- One state folder of this run, for example `$HOME/horizon-program-end`.
- No Horizon process of another person uses that state folder.

## 5. Setup

1. Make the state folder of this run.

   ```sh
   export STATE="$HOME/horizon-program-end"
   mkdir -p "$STATE/home"
   chmod 700 "$STATE/home"
   ```

   Result: The folder has mode `700`.

2. Start the candidate with the private state of this run, and watch the window
   in a Device panel. On Linux, start the candidate through
   [the device smoke fixture](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel).

   Result: The window shows one workspace.

3. Make one shell panel with **Menu**, then **New Panel**.

   Result: The panel reaches a shell prompt, and its title bar holds the close
   control.

## 6. Tasks

### 6.1 PROG-END-01 — The panel of an ended program says so

1. Type `exit` and press Enter.

   Result: A strip at the bottom of the grid says
   `The program of this panel ended. Text typed here is not sent.` The grid
   keeps the last screen, the prompt included.

2. Read the right end of the strip.

   Result: It holds the buttons `Start again` and `Close`.

3. Type `echo one` and press Enter.

   Result: The line stays, and the grid does not print `one`. No program takes
   the text.

4. Read the sidebar row of the panel.

   Result: The row keeps its place in its workspace, and it shows no state word
   next to the name of the panel.

### 6.2 PROG-END-02 — Start again starts the same program

1. Press the button `Start again`.

   Result: The strip goes, and the panel reaches a shell prompt again. The grid
   shows the text of the first program above the new prompt.

2. Type `echo two` and press Enter.

   Result: The panel prints `two`.

3. Type `exit 1` and press Enter.

   Result: The strip says
   `The program of this panel ended with status 1. Text typed here is not sent.`

4. Wait 30 seconds, and read the panel again.

   Result: The strip still says the same words. Horizon starts no program again
   by itself.

### 6.3 PROG-END-03 — Close closes the panel

1. Press the button `Close`.

   Result: The panel goes, and its sidebar row goes with it.

2. Make one shell panel, type `exit 0`, and press Enter.

   Result: The strip says `The program of this panel ended. Text typed here is
   not sent.` It does not name a status, because the status is zero.

3. Press the close control of the title bar.

   Result: The panel goes, as it does after the button `Close`.

### 6.4 PROG-END-04 — A restart of Horizon starts the program again

1. Make one shell panel, and type `exit` in it.

   Result: The strip says that the program ended.

2. Close Horizon normally, and start it again with the same state folder.

   Result: The panel is there, and its program runs: the panel reaches a shell
   prompt, and it shows no strip. Horizon starts the program of a saved panel
   again, as it did before this procedure existed.

### 6.5 PROG-END-05 — An SSH panel keeps its own line

1. Open one SSH panel of a host of the test, and stop the host.

   Result: The strip says `The link to <host> is lost. Text typed here is not
   sent.` It does not say that the program of the panel ended.

## 7. Pass criteria

- The strip of an ended program says that the program ended and that typed text
  is not sent, and it names the exit status when the status is not zero.
- The strip holds the buttons `Start again` and `Close`.
- No key reaches the panel after its program ended.
- `Start again` starts one program in the same panel, and the old text stays
  above the new program.
- `Close` closes the panel.
- Horizon starts nothing again and closes nothing by itself.
- A restart of Horizon starts the program of a saved panel again.
- An SSH panel keeps the line of a lost link.

## 8. Cleanup

1. Close the panels of this run.

   Result: The workspace holds no panel of the test.

2. Stop the candidate, and remove the state folder of this run.

   ```sh
   rm -rf "$HOME/horizon-program-end"
   ```

   Result: The folder is gone.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
