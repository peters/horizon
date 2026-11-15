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
that a lost link ends the call of the agent. No real machine is necessary: the
fixture supplies one session that speaks the card protocol, and one stand-in
`ssh` that does the work of the `sshd` of a machine.

## 2. Applicability

- Candidate: the build of the branch that holds the card sockets and the child
  that carries them.
- Platforms: Linux with `tmux`, `python3`, `Xvfb` and a VNC viewer, as
  [the fixture](../../../scripts/device-smoke/README.md) states.
- This procedure does not test: a real `sshd` with `AllowStreamLocalForwarding
  no`, a real agent program of a machine, and the setup section of the machines
  skill on a real machine. Those need a rented machine.

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

   Result: the machine `build-1` has three rows: **Needs you** for `hz-shop`,
   **Working** for `hz-notes`, and `hz-night`.

2. Read the canvas.

   Result: the workspace `build-1` holds a card with the heading `Night shift`
   and the buttons `Continue` and `Stop`. No terminal panel of `hz-night` is
   open.

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

### 6.3 MC-3 — A lost link ends the call of the agent

1. Stop the fixture, which ends the child of the cards of the machine.

   Result: the card stays in the workspace, and the row of `hz-night` leaves the
   Machines section.

2. Start the fixture again with the same state folder.

   Result: the card of `Night shift` is in the workspace again after the watch
   reports the session, and a press on it reaches the agent again.

### 6.4 MC-4 — A machine without the watch reports no session

1. Turn the setting **Show the agents of each machine** off.

   Result: the row of `hz-night` leaves the Machines section, and the agent of the
   session reads a transport error when it calls the tool.

2. Turn the setting on again.

   Result: the row comes back and a press reaches the agent again.

## 7. Pass criteria

- The card of `hz-night` appears in the workspace `build-1` while no panel of that
  session is open.
- A press on the card reaches the agent of the session, and the card changes to
  the label of that press.
- The footer names the machine and the session, and the chip names the machine.
- The card and the row follow the sessions that the watch reports.

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

   Result: no file of the run stays.

## 9. Record of results

Put the results in the pull request, with the GIF of the run and the hash of the
binary. Keep private evidence out of the repository.
