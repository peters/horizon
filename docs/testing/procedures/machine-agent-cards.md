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
- The checkout of this repository, which holds
  `scripts/device-smoke/machine_card_probe.py`, the probe of the card link.
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

   Result: the footer names `build-1` and `hz-night`. The time `updated HH:MM`
   stands at the right edge of the card, and no word of the footer is painted
   under it.

4. Read the chip of the titlebar of that card.

   Result: the chip names `build-1`. The card says no banner that its agent is not
   here, and both of its buttons are on: the session of the card is there, also
   when its program is no agent program and reports no state.

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
   `hz-night` leaves the Machines section. The run also stops its private `tmux`
   server, so the panes of the sessions end with the run.

2. Start the fixture again with the same state folder and the same command of
   step 5.1 of the setup. Wait until the Machines section reports the sessions
   again, and press `Continue` on the card of `Night shift`.

   Result: the card of `Night shift` is in the workspace, and the press reaches
   the agent of the session: the card says `The person pressed Continue.`

3. Read the lanes of a cut link in the check of the fixture:

   ```bash
   python3 -B -m unittest discover -s scripts/device-smoke/tests -v \
     -k test_a_stream_that_resets_is_a_lost_link \
     -k test_an_error_answer_of_the_relay_ends_no_call \
     -k test_a_cut_stream_ends_no_call_and_the_press_after_it_is_an_update
   ```

   Result: the three tests pass. The first server resets the stream of the socket,
   so `recv` of the relay raises `ConnectionResetError`: the relay stays alive, it
   answers the call in flight with the error, and a call after the reconnect works.
   The second relay answers one call with that error: the stand-in agent calls again,
   and the next press becomes an update of the card that it drew. The third server
   ends the stream of a direct card socket, which is the cut of the link of a card:
   the agent meets the socket again, says `initialize` again, draws no second card,
   and the press after the cut becomes an update of the same card.

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

### 6.8 MC-8 — A card whose session is gone says which session

1. Stop the session `hz-relay` of the private server:

   ```bash
   tmux -L hz-watch kill-session -t hz-relay
   ```

2. Wait two seconds, and read the cards of the workspace `build-1`.

   Result: the card `Night shift by relay` says `Its session hz-relay on build-1 is
   not there.` and `Start the session on that machine again, or close the card.`
   Its chip says `No agent` and its buttons are off. The card of `hz-night` keeps
   its buttons, because its session is there.

### 6.9 MC-9 — The cards over a real `sshd`, with no machine

This lane uses no rented machine. The machine of the fixture is one real `sshd` of
this computer, inside a namespace of its own, so `sshd` binds the socket of each
session and Horizon reaches it with the real `ssh` of this computer.

1. Unpack an `sshd` into a folder of your own. Neither command needs root:

   ```bash
   apt-get download openssh-server
   dpkg-deb -x openssh-server_*.deb /var/tmp/openssh-server
   ```

   Result: `/var/tmp/openssh-server/usr/sbin/sshd` and the two session programs in
   `/var/tmp/openssh-server/usr/lib/openssh/` are files.

2. Start the fixture with the real machine, and show its desktop in a Device panel
   as steps 5.1 and 5.2 show it:

   ```bash
   scripts/device-smoke/serve.py --horizon target/debug/horizon \
     --state /var/tmp/machine-cards-sshd-1 --native-view --machine-watch \
     --machine-sshd /var/tmp/openssh-server/usr/sbin/sshd --vnc-port 5912
   ```

   Result: the workspace `build-1` holds the cards of `hz-night` and of `hz-relay`,
   as MC-1 and MC-3 show them.

3. Read the folder of the card sockets of the machine. Horizon must have made it:

   ```bash
   ls -ld /var/tmp/machine-cards-sshd-1/machine-sshd/tmp/horizon-cards \
     /var/tmp/machine-cards-sshd-1/machine-sshd/tmp/horizon-cards/build-1
   ls /var/tmp/machine-cards-sshd-1/machine-sshd/tmp/horizon-cards/build-1
   ```

   Result: both folders have the mode `drwx------`, and the second one holds one
   socket file for each session that the watch reports. `sshd` bound each of them.

4. Start one more session on the machine, and wait two seconds:

   ```bash
   ssh -F /var/tmp/machine-cards-sshd-1/data/home/.ssh/config \
     -i /var/tmp/machine-cards-sshd-1/machine-sshd/fixture_key build-1 \
     'tmux new-session -d -s hz-late'
   ```

   Result: the folder of step 3 holds `hz-late.sock` without a restart of the child,
   and the cards of the other sessions keep their buttons.

5. Cut the link of the cards of the machine. The child that carries the sockets is
   an `ssh -N` child of the candidate, and its pid is the one whose arguments name
   `hz-night.sock`:

   ```bash
   ps -o pid,args -C ssh | grep -F hz-night.sock
   kill <pid>
   ```

   > **CAUTION:** THIS STEP CUTS THE LINK OF THE CARDS OF THE MACHINE. Read the pid,
   > and give that number to `kill`. Do not stop another process.

   Result: `sshd` leaves the socket file of each session in place. Both cards stay in
   the workspace with their buttons on, and the agents of the two sessions do not
   end: each one names the cut in its pane and calls again.

6. Wait five seconds. Cut the link twice more with the two commands of step 5.

   > **CAUTION:** THIS STEP CUTS THE LINK OF THE CARDS TWO MORE TIMES. Give `kill`
   > the pid that the command prints, and stop no other process.

   Result: each cut ends one child, and Horizon starts the next child of the machine.
   The two cards keep their buttons after the three cuts, and no card names a
   session that is not there.

7. Press `Continue` on the card of `Night shift`, and then on the card of `Night
   shift by relay`.

   Result: each card says `The person pressed Continue.` A press after the three cuts
   therefore reaches the agent of the session, through the direct socket and through
   the relay.

8. Read the folder of step 3 again, and read the log of the machine for a bind
   error:

   ```bash
   ls /var/tmp/machine-cards-sshd-1/machine-sshd/tmp/horizon-cards/build-1
   grep -c 'cannot bind' /var/tmp/machine-cards-sshd-1/machine-sshd/sshd.log
   ```

   Result: every socket file of the folder is bound again, because Horizon removed
   the old files before it started each child. The second command prints `0`: `sshd`
   bound every socket, and it reported no `cannot bind` and no
   `remote port forwarding failed`.

9. Prove that Horizon answers every call of an agent through a cut. Copy the probe
   of the fixture into the state folder, which the namespace of the machine binds:

   ```bash
   cp scripts/device-smoke/machine_card_probe.py \
     /var/tmp/machine-cards-sshd-1/machine_card_probe.py
   ```

   Start a session of the machine that runs the probe. The relay learns the name of
   its session from `tmux`, so the probe runs in a session of the machine:

   ```bash
   ssh -F /var/tmp/machine-cards-sshd-1/data/home/.ssh/config \
     -i /var/tmp/machine-cards-sshd-1/machine-sshd/fixture_key build-1 \
     'tmux new-session -d -s hz-probe -x 120 -y 30 \
       "python3 /var/tmp/machine-cards-sshd-1/machine_card_probe.py \
         --machine build-1 --relay ~/.horizon-cards/relay.py --calls 10 --pause 4"'
   ```

   Wait until the probe has made its first calls:

   ```bash
   ssh -F /var/tmp/machine-cards-sshd-1/data/home/.ssh/config \
     -i /var/tmp/machine-cards-sshd-1/machine-sshd/fixture_key build-1 \
     'tmux capture-pane -p -t hz-probe | tail -3'
   ```

   Result: the pane holds `machine-card-probe: call 2 is answered` and more lines of
   that shape, so the probe speaks to the socket of its session.

   Cut the link once more with the two commands of step 5.

   > **CAUTION:** THIS CUT ENDS THE CHILD OF THE CARDS OF THE MACHINE AGAIN. Give
   > `kill` the pid that the command of step 5 prints, and stop no other process.

   Wait 30 seconds, and read the pane of the probe again with the command above.

   Result: the pane holds one `call <n> is answered` line for each call, and its last
   line reads `machine-card-probe: 10 of 10 calls are answered; status 0`. A call that
   the cut reached reads `was answered with an error` and names the error of the lost
   link, and the last line then reads `status 1`: run the probe again through the next
   cut. The probe is the tool that shows that every call is answered through a cut.

10. Read the log of the machine:

    ```bash
    grep -E 'Server listening|Starting session' \
      /var/tmp/machine-cards-sshd-1/machine-sshd/sshd.log | tail -5
    ```

    Result: the log names the port that `machine_sshd.port` of `lab.json` names, and
    one `Starting session: command for <user>` line for each call of Horizon: the
    removal before a child and the child of the card sockets.

## 7. Pass criteria

- The cards of `hz-night` and of `hz-relay` appear in the workspace `build-1`
  while no panel of those sessions is open.
- A press on a card reaches the agent of the session, and the card changes to the
  label of that press.
- The footer names the machine and the session, and the chip names the machine.
- The time of a card of a machine keeps the right edge of the card, and the foot
  line ends with an ellipsis in the room that the time leaves.
- A card of a live session names no missing agent and keeps its buttons on, also
  when the program of its session is no agent program and reports no state.
- A card whose session the watch no longer names says which session and which
  machine are gone, and what the person can do.
- The rows follow the sessions that the watch reports, and a session with no agent
  program keeps its socket.
- A restart of the agent program keeps the card link, and the child of the machine
  binds the socket files of the sessions again.
- Horizon makes the folder of the card sockets of a machine, with mode 700, before
  the child of that machine starts.
- A new session of the machine gets its socket without a restart of the child and
  without a lost card of another session.
- A lost link answers the call of the agent with an error, and the stand-in agents
  of a machine call again: the card of each session outlives the cut, the relay
  program of the skill outlives a stream that resets, and the relay is the text that
  the card of `hz-relay` came through.
- The card shows its buttons whole and says no `More below`, also after a
  terminal of that session joins the workspace.
- The lane of section 6.9 shows the results that it names: Horizon makes the folder
  of the sockets of the machine, a new session gets its socket, three cuts of the
  link leave both cards with their buttons, a press after the cuts changes each card,
  `sshd` reports no `cannot bind`, and the probe reads `0` for every call.

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

4. Remove the state folder of MC-9 and the sshd that you unpacked:

   ```bash
   rm -rf /var/tmp/machine-cards-sshd-1 /var/tmp/openssh-server
   ```

   Result: both folders are gone, and no socket file and no `tmux` server of the
   machine stays on this computer.

## 9. Record of results

Put the results in the pull request, with the GIF of the run and the hash of the
binary. Keep private evidence out of the repository.
