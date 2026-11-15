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
workspace on a machine, that the row of that workspace says where it lives, that
the canvas picker says where its agents and its terminals start, that each new
terminal of that workspace runs on the machine in its own tmux session, that the
work in it survives a restart of Horizon, that a terminal reconnects by itself
after a lost link and keeps its session, that a close ends the session, that two
workspaces on one machine do not collide, and that a forgotten machine is
reported.

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

> **CAUTION: A FIREWALL RULE CHANGES THE NETWORK OF THIS COMPUTER.** Task P9 cuts
> a link. Stop the loopback SSH server instead of adding a rule when this
> computer has no test-only rule set.

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

   Result: The second line of the row starts with `on build-1`, in the dim style
   of the status text, and a middle dot and the status text follow it when the
   workspace has a status text. The name line is unchanged. The `Place` section
   now shows the check mark in front of `build-1` and no check mark in front of
   `This computer`.

2. Rename the workspace to a long name, then look at its row.

   Result: The name is cut with an ellipsis at the end of the name line, and the
   second line still starts with `on build-1`: the machine is never cut away.

3. Press the shortcut for a new terminal.

   Result: A terminal panel appears, and it connects to `127.0.0.1`. Its titlebar
   shows the chip `on build-1`. The panel is not a local shell prompt.

### 6.3 P2 — The canvas picker says where the agents of a placed workspace start

1. Double-click the empty canvas inside the workspace on the machine.

   Result: The preset picker opens with the heading `New Terminal`. Above the
   `Agents` header one dim line says `Agents start on this computer. Terminals
   start on build-1.`

2. Close the picker. Hold Ctrl and double-click empty canvas that fits a new
   panel, and choose a shell.

   Result: The new panel stands exactly at the point that was clicked.

3. Close that panel. Place the pointer on the lower-left corner of an existing
   panel of the workspace, so that a new panel at that point would cover it, hold
   Ctrl, and double-click there. Choose a shell.

   Result: The new panel covers no panel: Horizon gives it the nearest free place
   instead of the point of the click.

4. Hold Ctrl and double-click the canvas inside the workspace on this computer,
   whose place is `This computer`.

   Result: No line about a machine stands above the `Agents` header of that
   picker.

### 6.4 P3 — Type in the terminal of the machine, and resize it

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

### 6.5 P4 — Quit Horizon and start it again

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

### 6.6 P5 — Two workspaces on one machine do not collide

1. Make a second workspace with the sidebar `New` button, then right-click its
   row and click `build-1` in the `Place` section.

   Result: The second line of the second row starts with `on build-1`.

2. Press the shortcut for a new terminal, in the second workspace.

   Result: A second panel with the chip `on build-1` appears.

3. Run `echo second` in the second panel, and look at the first panel.

   Result: The first panel does not show the line of the second panel. The two
   panels hold different sessions.

4. In the second terminal, run `ssh -p 2222 -i <the machine key> 127.0.0.1 tmux
   ls`.

   Result: Two sessions that start with `hz-` are in the list, and the part
   after `hz-` differs.

### 6.7 P6 — A close ends the session

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

### 6.8 P7 — A forgotten machine

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

   Result: The second line of the row no longer starts with `on build-1`.

5. Press the shortcut for a new terminal.

   Result: A terminal panel with a local shell prompt appears, and it shows no
   chip.

### 6.9 P8 — The panels that are open stay where they are

1. Put the first workspace on the machine again, with the record that task P7
   needs. Register the machine again with `machine_set` as in setup step 2, then
   click `build-1` in the `Place` section.

   Result: The second line of the row starts with `on build-1` again.

2. Make one terminal in the workspace, then click `This computer` in the `Place`
   section.

   Result: The panel with the chip `on build-1` stays open. The second line of the
   row holds the status text alone, with no `on` prefix.

3. Press the shortcut for a new terminal.

   Result: The new panel is a local shell with no chip. The first panel still
   shows the chip `on build-1`.

4. In the second terminal, run `ssh -p 2222 -i <the machine key> 127.0.0.1 tmux
   ls`.

   Result: The session of the panel that is still open is in the list.

### 6.10 P9 — A lost link reconnects by itself

1. Run `printf 'lost-%s\n' link` in the terminal of the machine.

   Result: `lost-link` is the last output of the panel.

2. Cut the link for 60 seconds: stop the loopback SSH server, or add a rule to
   the firewall of this computer that drops TCP port 2222.

   Result: The panel shows `Disconnected` in red within 30 seconds, and its last
   lines hold the reason that ssh printed.

3. Watch the panel and the row `build-1` of the `Machines` section.

   Result: The mark of the panel reads `Reconnecting` in the warning colour, the
   body keeps the last screen, and the dot of the machine row is filled in the
   same colour. The mark changes to `Connecting...` while an attempt runs, and
   back to `Reconnecting` when that attempt fails. The body holds `lost-link`
   while each of those attempts runs, until the bytes of a connection draw over
   it. From the third attempt on, the mark names the attempt, for example
   `Reconnecting (3)`, in the same colour.

4. Keep the link cut for four attempts, and read the mark between them.

   Result: The mark counts the attempts from the third one on, as in
   `Reconnecting (3)`, in the same warning colour. The body still holds
   `lost-link` after each failed attempt.

5. Wait 30 seconds without a key press.

   Result: The panel tries again by itself, and Horizon shows no dialog and no
   error line.

6. Press one key in the panel while the mark reads `Reconnecting`.

   Result: The panel shows `Connecting...` at once, before the delay that was
   due, and the next loss of the link waits 3 seconds again.

7. Read the command line of the `ssh` child of the new attempt.

   ```sh
   ps -o pid=,args= -C ssh | grep 2222
   ```

   Result: The line holds `-o ConnectTimeout=10`, `-o ServerAliveInterval=5` and
   `-o ServerAliveCountMax=3`.

8. Cut the link with the rule of step 2. Type `echo ahead` while the mark reads
   `Connecting...`.

   Result: No new attempt starts. The mark stays `Connecting...`, the list of
   step 7 holds one `ssh` child of this attempt, and the text went into that
   child.

9. Remove the rule of step 2.

   Result: Within 30 seconds the mark reads `Connected` in green. The line
   `echo ahead` is at the prompt of the session, and `Enter` runs it now: the
   attempt kept the text that the person typed while it ran.

10. Cut the link with the rule of step 2 again, and press `Esc` while the mark
    reads `Connecting...`.

    Result: The attempt ends and the next one starts at once, so the mark reads
    `Connecting...` for the new attempt. No `ssh` child of the ended attempt
    stays in the list of step 7.

11. Remove the rule of step 2.

    Result: Within 30 seconds the mark reads `Connected` in green, and the last
    output is still `lost-link`. No output of the session was lost, and the body
    never went blank while the attempts ran.

12. Run `echo again`, then run `ssh -p 2222 -i <the machine key> 127.0.0.1 tmux
    ls` in the second terminal.

    Result: The command runs on the machine, and the session of the panel is the
    one before the cut. No second session exists.

13. Type `exit` in the shell of the panel.

    Result: The tmux session ends, the mark reads `Disconnected` in red, and it
    stays `Disconnected` for at least 30 seconds. Horizon does not start the
    command again, because a new `tmux new-session -A` would make an empty
    session without the person.

14. Press the shortcut for a new terminal in the workspace, run one command in
    it, and cut the link again as in step 2. When the mark reads
    `Reconnecting`, close the panel with its titlebar close button.

    Result: The panel goes away, Horizon shows no error dialog, and Horizon
    starts no further attempt.

15. Remove the rule of step 2.

    Result: The machine answers again, and the `tmux ls` command of step 12 lists
    no session of the two panels that ended.

## 7. Known behaviour

### KB1 — The status line of tmux is off

Horizon runs `tmux -u new-session -A -s <session> \; set-option status off`, and a
terminal that a click on a machine row opens runs the same command, so the
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

### KB5 — A lost link of a machine terminal reconnects by itself

A panel whose saved command attaches a tmux session runs its command again when
ssh ends with its own connection-failure status 255: after 3, 6, 12 and 24
seconds, and then every 30 seconds, with no limit. The body keeps the screen of
the session while the attempts run, because the session still holds that work,
and the mark names the attempt from the third one on. The delays start again after
a connection that lasted 10 seconds, and a key press in the panel or the
`Reconnect` action connects at once and starts the delays again. Any other exit
status, a plain SSH panel of the remote hosts chooser, an agent panel and a
local shell never reconnect. A panel that the person closed tries nothing, and
the attempts of a panel that a machine record opened stop when that record is
forgotten. Quitting Horizon starts the delays again in the next run.

Every ssh that Horizon starts for a machine terminal, which is a click on a
machine row, a terminal of a placed workspace and the attempt after a lost link,
holds `-o ConnectTimeout=10`, `-o ServerAliveInterval=5` and
`-o ServerAliveCountMax=3`. A connect that a network drops therefore ends in ten
seconds, and an open connection whose machine stops answering ends after fifteen,
so the delays above run on a dead network. ssh reads the first value of an option
that it meets, and the value of a command line comes before the value of a
configuration file, so these three options decide for the connection that Horizon
starts, also when the ssh config of the person names other values. A plain SSH
panel of the remote hosts chooser holds `-o ConnectTimeout=10`,
`-o ServerAliveInterval=15` and `-o ServerAliveCountMax=1` now, and so do an
upload and the `ssh -W` forward of a Device panel: no child of Horizon waits for
the connect timeout of the system.

A key press in a machine terminal whose attempt is in flight, which is the state
`Connecting`, does not end that attempt: the text goes into the running ssh child
and reaches the shell when the link is up. Only two keys end the attempt, `Esc`
and `Enter`, and the next one starts at once. A letter never ends an attempt,
because a person who types ahead on a link of a round trip of 300 ms would
otherwise keep cancelling a connect that is only slow. A key press in a panel
that waits for its attempt, which is the state `Reconnecting`, still connects at
once. Horizon kills and reaps the child of an attempt that it ends, so no process
of the dead network stays.

### KB6 — A known list of panels follows the place

Only a shell or a command panel moves to the machine. An agent, browser, Device,
card or editor panel of the same workspace starts on this computer, as it did
before the workspace had a place.

## 8. Pass criteria

- A placed workspace starts the second line of its row with `on <machine>`, and a
  long name cannot cut the machine away. Its menu marks the place.
- The canvas picker says, above its agent entries, where the agents and the
  terminals of a placed workspace start.
- A panel that a person creates at a click on empty canvas keeps that point, and
  a click that would cover a panel gives the new panel the nearest free place.
- Each new terminal of a placed workspace runs on the machine, in its own
  session, with the chip `on <machine>`.
- A restart of Horizon attaches the panel to the same session, and the output of
  the session is still there.
- A lost link of a terminal of the machine reconnects by itself, without a key
  press, and the panel attaches to the same session with its output.
- A machine terminal holds `ConnectTimeout=10`, `ServerAliveInterval=5` and
  `ServerAliveCountMax=3`. A letter in a panel whose attempt is in flight does
  not end that attempt and reaches the shell after the connect, and `Esc` and
  `Enter` end it and start a new one at once.
- A session that ended does not reconnect, and a panel that a person closed
  starts no attempt.
- Two workspaces on one machine hold two sessions.
- A close of a panel ends its session, and a quit of Horizon ends none.
- A workspace that holds a panel of a machine asks before it closes its panels.
- A forgotten machine gives `machine_unavailable` and makes no panel, and a
  person can put the workspace on this computer again.

## 9. Cleanup

1. Close the test window normally, then stop the loopback SSH server.

   Result: The fixture processes stop.

2. In the second terminal, remove the file of task P3 if the loopback server
   shares the file system of this computer: `rm -f /var/tmp/hz-place-proof`.

   Result: The file is gone.

3. Remove the temporary directory of the fixture.

   Result: `git -C <checkout> status --short` shows no change that the procedure
   made.

## 10. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
