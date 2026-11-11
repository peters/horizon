---
procedure: workspace-place
feature: A workspace that lives on a machine: its new terminals run there and survive a restart
platforms: [linux]
cost: none
destructive: yes
secrets: none
owner: peters
---

# A workspace on a machine test procedure

## 1. Purpose

This procedure tests the place of a workspace. It proves that the person puts a
workspace on a machine, that each new terminal of that workspace runs on the
machine in its own tmux session, that the work in it survives a restart of
Horizon, that a close ends the session, that two workspaces on one machine do
not collide, and that a forgotten machine is reported.

## 2. Applicability

- Candidate: a current `horizon` build.
- Platforms: Linux.
- The procedure uses a loopback SSH server on this computer, so it needs no
  rented machine and costs nothing. It does not rent compute.
- This procedure does not test agents on a machine, a browser or a Device panel
  of a machine, or a remote working directory. Horizon keeps those panels on this
  computer.

## 3. Safety

> **CAUTION: THE TEST WRITES TO A PRIVATE HORIZON HOME AND ENDS TMUX SESSIONS OF A
> LOOPBACK MACHINE ONLY.** Start the candidate through the fixture, which sets
> its own `HORIZON_HOME` and its own desktop. Do not start it with your own home.

> **CAUTION: DO NOT RUN THIS PROCEDURE AGAINST A REAL MACHINE.** A close in task
> P5 ends a tmux session with `tmux kill-session`, and the work in that session
> stops.

## 4. Equipment and preconditions

- A frozen candidate `horizon` binary and its SHA-256.
- An isolated test desktop, viewed through a Horizon native VNC Device panel, as
  [the device smoke fixture](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel)
  says.
- A loopback SSH server on port 2222 that accepts a machine key, as
  [the block-panel procedure](block-panel.md) sets it up.
- `tmux` on this computer, so the loopback SSH server has a session to make.
- One agent panel in the first workspace, with the `block_panel` tool.
- A second terminal on this computer, outside Horizon and outside the fixture
  desktop, that runs `ssh` to the loopback server, so the operator can run
  `tmux ls` on the machine.

## 5. Setup

1. Start the loopback SSH server, then start the candidate in the isolated
   desktop through the fixture with the native viewer. Watch the window in the
   Device panel.

   Result: The window shows one workspace and the sidebar with `Workspaces`.

2. Register the machine with the agent panel. Call `block_panel` with
   `{"operation":"machine_set"}` and the record `{"name":"build-1","ssh":{"host":"127.0.0.1","port":2222},"key":"build-1","provider":"Loopback, test"}`.

   Result: The sidebar shows the `Machines` section with the row `build-1`.

3. In the second terminal, run `ssh -p 2222 -i <the machine key> 127.0.0.1 tmux ls`.

   Result: The output says that there is no server running, or it lists no
   session that starts with `hz-`.

## 6. Tasks

### 6.1 P0 — A workspace on this computer is the default

1. Right-click the row of the workspace and look at the `Place` section of the
   menu.

   Result: The menu shows `Place`, then `This computer` with a check mark in
   front of it, then `build-1` with no check mark.

2. Close the menu and press the shortcut for a new terminal.

   Result: A terminal panel shows a local shell prompt, and its titlebar shows no
   chip.

### 6.2 P1 — Put the workspace on the machine

1. Right-click the row of the workspace and click `build-1` in the `Place`
   section.

   Result: The row shows `build-1` after the workspace name, in a dim style. The
   `Place` section now shows the check mark in front of `build-1` and no check
   mark in front of `This computer`.

2. Move the pointer over the name in the row.

   Result: A hover text shows `build-1`.

3. Press the shortcut for a new terminal.

   Result: A terminal panel appears, and it connects to `127.0.0.1`. Its titlebar
   shows the chip `on build-1`. The panel is not a local shell prompt.

### 6.3 P2 — Type in the terminal of the machine, and resize it

1. In the new terminal, run `echo $SSH_CONNECTION`.

   Result: The output holds the address of the loopback server, not an empty
   line. The command ran on the machine.

2. Run `touch /var/tmp/hz-place-proof` on the machine, then run `ls -l
   /var/tmp/hz-place-proof`.

   Result: The file exists on the machine.

3. Drag the lower-right corner of the panel to make it about half as wide and
   half as tall.

   Result: The text of the terminal is redrawn at the new size, and the shell
   prompt keeps its position at the bottom of the visible area. The panel keeps
   one row at its bottom for tmux, which this procedure turns off in known
   behaviour KB1; if the row is there, record it.

4. Run `tput cols` and `tput lines` in the terminal.

   Result: The two numbers follow the new size of the panel, with a difference of
   at most one. A second client on the same session could shrink the view; this
   procedure uses one client, so the numbers follow the panel.

### 6.4 P3 — Quit Horizon and start it again

1. Run `printf 'place-%s\n' proof` in the terminal of the machine, so the last
   line of the screen is known.

   Result: The line `place-proof` is the last output.

2. Quit Horizon, then start it again through the same fixture command.

   Result: The window shows the workspace, and the terminal of the machine
   re-attaches by itself. The last line of the panel holds `place-proof`, and
   the title of the panel is the title that it had before the restart.

3. In the terminal, run `echo again`.

   Result: The command runs on the machine. The panel attached to the same
   session and did not start a new one.

4. In the second terminal, run `ssh -p 2222 -i <the machine key> 127.0.0.1 tmux
   ls`.

   Result: Exactly one session that starts with `hz-` is in the list.

### 6.5 P4 — Two workspaces on one machine do not collide

1. Make a second workspace with the sidebar `New` button, then right-click its
   row and click `build-1` in the `Place` section.

   Result: The second row shows `build-1` after its name.

2. Press the shortcut for a new terminal, in the second workspace.

   Result: A second panel with the chip `on build-1` appears.

3. Run `echo second` in the second panel, and look at the first panel.

   Result: The first panel does not show the line of the second panel. The two
   panels hold different sessions.

4. In the second terminal, run `ssh -p 2222 -i <the machine key> 127.0.0.1 tmux
   ls`.

   Result: Two sessions that start with `hz-` are in the list, and the part
   after `hz-` differs.

### 6.6 P5 — A close ends the session

1. In the second terminal, note the name of the session of the second panel from
   `tmux ls`.

   Result: One name of the form `hz-<eight hexadecimal characters>-<n>`.

2. Close the second panel with its titlebar close button.

   Result: The panel goes away, and Horizon shows no error dialog.

3. In the second terminal, run `ssh -p 2222 -i <the machine key> 127.0.0.1 tmux
   ls` again.

   Result: The session of the second panel is not in the list. The session of
   the first panel is still there.

4. Right-click the row of the second workspace, click `Close All Panels`, and
   read the confirm line.

   Result: The workspace holds no panel of the machine any more, so the menu
   closes its panels at once. Record what it does.

5. Make one terminal in the second workspace again, then right-click the row of
   the second workspace and click `Close All Panels`.

   Result: The menu shows the line `End 1 tmux session(s) of this workspace's
   machine?`, then `Close All Panels` and `Cancel`.

6. Click `Cancel`.

   Result: The panel stays, and the session stays in `tmux ls`.

7. Click `Close All Panels` again and then `Close All Panels` in the confirm
   line.

   Result: The panel goes away, and its session is not in `tmux ls`.

### 6.7 P6 — A forgotten machine

1. Right-click the row of the machine `build-1` in the `Machines` section, click
   `Forget this machine`, and confirm.

   Result: The row of the machine goes away.

2. Right-click the row of the first workspace and look at the `Place` section.

   Result: The `Place` section still shows the place of the workspace. Record
   what it shows for the forgotten machine: this procedure expects the dimmed
   row `build-1 (gone)` with the check mark, and the pointer must not select it.

3. Press the shortcut for a new terminal in the first workspace.

   Result: No panel appears. The sidebar shows a line that names `build-1` and
   the reason `machine_unavailable`.

4. Click `This computer` in the `Place` section of the workspace.

   Result: The row shows no machine after the name.

5. Press the shortcut for a new terminal.

   Result: A terminal panel with a local shell prompt appears, and it shows no
   chip.

### 6.8 P7 — The panels that are open stay where they are

1. Put the first workspace on the machine again, with the record that task P6
   needs. Register the machine again with `machine_set` as in setup step 2, then
   click `build-1` in the `Place` section.

   Result: The row shows `build-1` again.

2. Make one terminal in the workspace, then click `This computer` in the `Place`
   section.

   Result: The panel with the chip `on build-1` stays open. The row shows no
   machine.

3. Press the shortcut for a new terminal.

   Result: The new panel is a local shell with no chip. The first panel still
   shows the chip `on build-1`.

4. In the second terminal, run `ssh -p 2222 -i <the machine key> 127.0.0.1 tmux
   ls`.

   Result: The session of the panel that is still open is in the list.

## 7. Known behaviour

### KB1 — The status line of tmux is off

Horizon runs `tmux new-session -A -s <session> \; set-option status off`, so the
panel has no tmux status line at its bottom. The last row of the panel is the
last row of the shell. A person who starts a session of their own on the machine
keeps the default status line.

### KB2 — The scrollback belongs to tmux

tmux draws a full screen, so the scrollback of the panel in Horizon stays empty.
Press the tmux copy-mode key, `Ctrl-b` and then `[`, to look at older output.
`q` leaves copy-mode. The mouse wheel scrolls the copy-mode buffer only while
copy-mode is open, because Horizon does not turn tmux mouse mode on.

### KB3 — Resize follows the policy of tmux

The size of the panel becomes the size of the remote terminal. tmux uses its
`window-size` policy, which is `latest` by default: the window takes the size of
the client that attached last. A second client on the same session therefore
changes the size of the first. This procedure uses one client per session.

### KB4 — No local directory is asked for

A workspace on a machine never asks where its new terminal runs, because the
remote shell starts in the home directory of the machine. A shell of a workspace
on this computer asks the question only when the directory of the workspace is
gone.

### KB5 — A known list of panels follows the place

Only a shell or a command panel moves to the machine. An agent, browser, Device,
card or editor panel of the same workspace starts on this computer, as it did
before the workspace had a place.

## 8. Pass criteria

- A placed workspace shows the machine in its row, and its menu marks the place.
- Each new terminal of a placed workspace runs on the machine, in its own
  session, with the chip `on <machine>`.
- A restart of Horizon attaches the panel to the same session, and the output of
  the session is still there.
- Two workspaces on one machine hold two sessions.
- A close of a panel ends its session, and a quit of Horizon ends none.
- A workspace that holds a panel of a machine asks before it closes its panels.
- A forgotten machine gives `machine_unavailable` and makes no panel, and a
  person can put the workspace on this computer again.

## 9. Cleanup

1. Close the test window normally, then stop the loopback SSH server.

   Result: The fixture processes stop.

2. In the second terminal, remove the file of task P2 if the loopback server
   shares the file system of this computer: `rm -f /var/tmp/hz-place-proof`.

   Result: The file is gone.

3. Remove the temporary directory of the fixture.

   Result: `git -C <checkout> status --short` shows no change that the procedure
   made.

## 10. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
