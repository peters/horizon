---
procedure: machine-agent-cards
feature: The cards of an agent that runs on a machine
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# The cards of an agent on a machine test procedure

## 1. Purpose

This procedure proves that an agent program in a session of a machine draws a
card in the workspace of that machine, that the person reads and presses it, and
that the card link lives through what a real machine does: the agent program
restarts, a new session joins, and a link ends. No real machine is necessary: the
fixture supplies two sessions with a card, one that speaks the card protocol
directly and one through the relay program that the machines skill ships, and one
stand-in `ssh` that does the work of the `sshd` of a machine.

## 2. Applicability

- Candidate: the build of the branch that holds the card sockets and the child
  that carries them.
- Platforms: Linux with `tmux`, `python3`, `Xvfb` and a VNC viewer, as
  [the fixture](../../../scripts/device-smoke/README.md) states.
- This procedure does not test: a real `sshd` with `AllowStreamLocalForwarding
  no`, a real agent program of a machine, and the setup section of the machines
  skill on a real machine. Those need a rented machine.
- The relay program is the text of
  `assets/plugins/claude-code/skills/horizon-machines/SKILL.md`, so a change of
  that text is a change of what this procedure tests.

## 3. Safety

No step rents compute, deletes data, sends a secret or changes access. The
fixture uses a private home, a private temporary folder and its own `tmux`
server, so it changes no setting of the person.

## 4. Equipment and preconditions

- A Linux computer with `tmux`, `python3`, `Xvfb`, `x11vnc` and `ffmpeg`.
- A built candidate: `cargo build` writes `target/debug/horizon`.
- A free VNC port on the loopback address, for example `5911`.

## 5. Setup

1. Start the fixture, which prepares the machine, its sessions and the isolated
   desktop.

   ```bash
   scripts/device-smoke/serve.py --horizon target/debug/horizon \
     --state /var/tmp/machine-cards-1 --native-view --machine-watch \
     --vnc-port 5911
   ```

   Result: the run prints the VNC address `127.0.0.1:5911`.

2. Show the isolated desktop in a Horizon Device panel of the current workspace,
   with the `device_panel` tool of the `horizon-device` skill and that VNC
   address.

   Result: the panel shows the desktop of the fixture, and its image and its
   frame sequence change while you look.

## 6. Tasks

### 6.1 MC-1 — The card of the session appears without its terminal

1. Read the Machines section of the sidebar of the candidate.

   Result: the machine `build-1` has four rows: **Needs you** for `hz-shop`,
   **Working** for `hz-notes`, and the rows `hz-night` and `hz-relay`. The
   sessions `hz-relay` and `hz-night` run no agent program that reports a state,
   so their rows are the rows of a session with a card.

2. Read the canvas.

   Result: the workspace `build-1` holds two cards: `Night shift` and `Night
   shift by relay`, each with the buttons `Continue` and `Stop`. No terminal
   panel of `hz-night` or of `hz-relay` is open. Each card shows both buttons
   whole and none shows `More below`. The toolbar of that workspace shows
   `Default` selected.

3. Read the footer of that card.

   Result: the footer names `build-1` and `hz-night`.

4. Read the chip of the titlebar of that card.

   Result: the chip names `build-1`.

### 6.2 MC-2 — A press reaches the agent of the machine

1. Press `Continue` on the card of `Night shift`.

   Result: the card says `The person pressed Continue.`

2. Read the pane of the session `hz-night` in the private server:

   ```bash
   tmux -L hz-watch attach -t hz-night
   ```

   Detach with `Ctrl+b`, `d`.

   Result: the pane holds one line `machine-card-agent: the person pressed
   Continue`.

### 6.3 MC-3 — The card of the relay program

1. Read the pane of the session `hz-relay` in the private server:

   ```bash
   tmux -L hz-watch attach -t hz-relay
   ```

   Detach with `Ctrl+b`, `d`.

   Result: the pane holds the line `machine-card-agent: card <id> is drawn`. The
   agent of that session speaks to the relay program, and the relay meets the
   socket of the session.

2. Press `Continue` on the card of `Night shift by relay`.

   Result: the card says `The person pressed Continue.`, and the pane of
   `hz-relay` holds the same line.

3. Stop the relay program of that session, and start it again:

   ```bash
   tmux -L hz-watch send-keys -t hz-relay C-c
   tmux -L hz-watch respawn-pane -k -t hz-relay
   ```

   The second command starts the same command line again, which is the restart
   of an agent program that a person makes.

   Result: the card of `Night shift by relay` stays in the workspace, and the
   sidebar keeps its rows. The child of the cards starts again with the call that
   makes the folder of the sockets on the machine, sets it to mode 700, and removes
   the socket files of the child before it, so the log of the candidate holds no
   `remote port forwarding failed` and no `Address already in use`. After the next
   report of the watch, a press on that card reaches the agent again, with a new
   card of the restarted program.

4. Press the row of `hz-relay` once more, and read the card that it draws.

   Result: the card `Night shift by relay` stands beside the first one, and a
   press reaches the agent of the restarted program.

### 6.4 MC-4 — A new session that needs no restart

1. Make a new session in the private server, with no agent program and no state:

   ```bash
   tmux -L hz-watch new-session -d -s hz-new -x 100 -y 20 'sleep 600'
   ```

2. Wait two seconds, and read the state of the child of the machine:

   ```bash
   ps -A -o pid=,args= | grep -c 'horizon-cards'
   ```

   Result: the count is one more than before the command of step 1, because the
   child of that machine now carries the socket of `hz-new` too. No agent
   program restarted, and no session lost its card.

3. Read the folder of the card sockets inside the fixture namespace:

   ```bash
   ls /var/tmp/machine-cards-1/private/tmp/horizon-cards/build-1/
   ```

   Result: the folder holds `hz-new.sock` beside the sockets of the other
   sessions.

4. Read the mode of the two folders of the sockets:

   ```bash
   ls -ld /var/tmp/machine-cards-1/private/tmp/horizon-cards \
     /var/tmp/machine-cards-1/private/tmp/horizon-cards/build-1
   ```

   Result: both lines start with `drwx------`. Horizon makes both folders, with
   mode 700, before each start of the child of the machine: `sshd` binds no socket
   in a folder that is not there.

5. Remove that session again:

   ```bash
   tmux -L hz-watch kill-session -t hz-new
   ```

   Result: `hz-new.sock` leaves the folder, and the sockets and cards of the
   other sessions stay.

### 6.5 MC-5 — A lost link answers the call of the agent

CAUTION: this step stops the card link of the machine. A card that an agent draws
while the link is down reaches nothing, and the agent reads an error.

1. Stop the fixture with `Ctrl+C` in the terminal of step 5.1 of the setup, which
   ends the child of the cards of the machine.

   Result: the card of `Night shift` stays in the workspace, and the row of
   `hz-night` leaves the Machines section.

2. Read the pane of the session `hz-relay` in the private server:

   ```bash
   tmux -L hz-watch attach -t hz-relay
   ```

   Detach with `Ctrl+b`, `d`.

   Result: the pane holds one line `machine-card-agent: the card socket ended:
   The link to Horizon was lost. Call again.` That call of the agent waited when
   the link ended: a call during a cut is answered with that error, and the agent
   calls again.

3. Start the fixture again with the same state folder and the same command of
   step 5.1 of the setup. Wait until the Machines section reports the sessions
   again, and press `Continue` on the card of `Night shift`.

   Result: the card of `Night shift` is in the workspace, and the press reaches
   the agent of the session: the card says `The person pressed Continue.`

4. Read the lane of a stream that resets, in the check of the fixture:

   ```bash
   python3 -B -m unittest discover -s scripts/device-smoke/tests -v \
     -k test_a_stream_that_resets_is_a_lost_link
   ```

   Result: the test passes. Its server leaves one call of the relay unread and
   resets the stream of the socket, so `recv` of the relay raises
   `ConnectionResetError`: the relay stays alive, it answers the call in flight with
   the error, and a call after the reconnect works. The fixture above ends the link
   with a close, and this test is the lane of the harder cut.

### 6.6 MC-6 — A machine without the watch reports no session

1. Turn the setting **Show the agents of each machine** off.

   Result: the row of `hz-night` leaves the Machines section, and the agent of the
   session reads a transport error when it calls the tool.

2. Turn the setting on again.

   Result: the row comes back and a press reaches the agent again.

### 6.7 MC-7 — A terminal of the session joins the workspace of the cards

1. Press the row of `hz-night` in the Machines section.

   Result: the terminal of that session opens in the workspace `build-1` beside
   the card, at the size of a new terminal.

2. Read the toolbar of the workspace `build-1`.

   Result: `Default` is selected, and the card keeps the size of its content.

3. Close the terminal.

   Result: the card alone remains in the workspace.

## 7. Pass criteria

- The cards of `hz-night` and of `hz-relay` appear in the workspace `build-1`
  while no panel of those sessions is open.
- A press on a card reaches the agent of the session, and the card changes to the
  label of that press.
- The footer names the machine and the session, and the chip names the machine.
- The rows follow the sessions that the watch reports, and a session with no agent
  program keeps its socket.
- A restart of the agent program keeps the card link, and the child of the machine
  binds the socket files of the sessions again.
- Horizon makes the folder of the card sockets of a machine, with mode 700, before
  the child of that machine starts.
- A new session of the machine gets its socket without a restart of the child and
  without a lost card of another session.
- A lost link answers the call of the agent with an error that the agent program
  ends, the relay program of the skill outlives a stream that resets, and the relay
  is the text that the card of `hz-relay` came through.
- The card shows its buttons whole and says no `More below`, also after a
  terminal of that session joins the workspace.

## 8. Cleanup

1. Close the Device panel that shows the fixture.

   Result: the panel leaves the workspace.

2. Stop the fixture with `Ctrl+C` in the terminal of step 5.1.

   Result: the run stops its private `tmux` server, its VNC server and its
   candidate, and it removes no file of the person.

3. Remove the state folder of the run and its cards folder:

   ```bash
   rm -rf /var/tmp/machine-cards-1
   ```

   Result: no file of the run stays, and no socket file of a session is left in
   the folder that the fixture used for `/tmp`.

## 9. Record of results

Put the results in the pull request, with the GIF of the run and the hash of the
binary. Keep private evidence out of the repository.
