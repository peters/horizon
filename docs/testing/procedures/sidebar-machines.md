---
procedure: sidebar-machines
feature: The Machines section of the sidebar, and a click that connects without an agent
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Machines in the sidebar procedure

## 1. Purpose

This procedure tests the Machines section of the sidebar and the machine
records that agents write. It proves that an agent registers a machine, that the
person sees one row for it, that one click connects without an agent, that the
row says the state of that terminal, that the list survives a restart, and that
a forgotten record leaves its key.

## 2. Applicability

- Candidate: a current `horizon` build.
- Platforms: Linux.
- The procedure uses a loopback SSH server on this computer, so it needs no
  rented machine and costs nothing.
- This procedure does not test a real cloud provider, provider prices, a
  workspace that lives on a machine, or an agent that runs on a machine.

## 3. Safety

> **CAUTION: THE TEST WRITES TO A PRIVATE HORIZON HOME ONLY.** Start the
> candidate through the fixture, which sets its own `HORIZON_HOME` and its own
> desktop. Do not start it with your own home.

## 4. Equipment and preconditions

- A frozen candidate `horizon` binary and its SHA-256.
- An isolated test desktop, viewed through a Horizon native VNC Device panel, as
  [the device smoke fixture](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel)
  says.
- A loopback SSH server on port 2222 that accepts the machine key, as
  [the block-panel procedure](block-panel.md) sets it up.
- One agent panel in the first workspace with the `block_panel` tool and a
  shell.
- The scripted card agent, `scripts/device-smoke/card_agent.py`, with the
  scenario `machines` and `CARD_AGENT_SSH_HOST=127.0.0.1`.

## 5. Setup

1. Start the candidate in the isolated desktop through the fixture with the
   native viewer. Watch the window in the Device panel.

   Result: The window shows one workspace and the sidebar. The sidebar shows
   `Workspaces` and no `Machines` section.

2. Start the loopback SSH server and the agent panel of the first workspace.

   Result: The agent panel answers `block_panel`.

## 6. Tasks

### 6.1 M0 — No machine means no section

1. Look at the sidebar below the workspace rows.

   Result: There is no `Machines` header, no row and no empty box.

2. Call `block_panel` with `{"operation":"machines"}`.

   Result: The answer is `status: machines` with `machines: []`.

### 6.2 M1 — An agent registers two machines

1. Run the card agent with `scenario=machines`,
   `CARD_AGENT_SSH_HOST=127.0.0.1` and the second host `127.0.0.1`.

   Result: The agent calls `machine_set` two times, with the names `build-1` and
   `build-2`, the keys `build-1` and `build-2`, the sessions `build-1` and
   `build-2`, and the provider line `Loopback, test`. Both answers have
   `created: true`.

2. Look at the sidebar.

   Result: A `Machines` header appears below the workspace rows, with one row
   `build-1` and one row `build-2`, in that order. Each row shows a hollow dot,
   the name, and the dimmed text `Loopback, test`.

3. Move the pointer over a row.

   Result: The row shows a pointer cursor and a hover text that names the
   machine and says `no terminal open`.

### 6.3 M2 — One click connects without an agent

1. Click the row `build-1`.

   Result: A new SSH panel opens in the active workspace with the title
   `build-1`, the titlebar mark `opened by Machines`, and a prompt from the
   loopback server. The row dot turns green and the hover text says
   `connected`. The last row of the panel holds the output of the session, and
   no green tmux status line stands at the bottom of the panel.

2. Read the command line of the `ssh` child of the fixture process.

   ```sh
   ps -o pid=,args= -C ssh
   ```

   Result: The line holds `-i <state>/data/home/.horizon/machine-keys/build-1`,
   `-o IdentitiesOnly=yes`, `-o IdentityAgent=none`,
   `-o UserKnownHostsFile=<state>/data/home/.horizon/machine-keys/build-1.known_hosts`,
   `-o StrictHostKeyChecking=accept-new`, `-t`, `root@127.0.0.1` or the user of
   the record, the port `-p 2222` when the record has it, and
   `tmux new-session -A -s build-1 \; set-option status off` last. That is the
   same command that a terminal of a workspace on the machine runs.

3. Click the row `build-1` again.

   Result: No second panel opens. The existing panel takes the keyboard focus
   and the canvas moves to it.

4. Switch to another workspace, then click the row `build-1` again.

   Result: Horizon switches to the workspace of the panel and focuses it.

5. Click the row `build-2`.

   Result: A second SSH panel opens, in the active workspace, with the mark
   `opened by Machines` and the session `build-2`.

### 6.4 M3 — The list survives a restart

1. Close the window normally, so the session is saved. Start the candidate
   again through the fixture with the same home.

   Result: The `Machines` section shows the same two rows. The panel `build-1`
   starts its own `ssh` again, without an agent, and attaches to the same tmux
   session.

2. Call `block_panel` with `{"operation":"machines"}` from a new agent panel.

   Result: The answer holds both records, with `terminal_open: true` for each
   machine whose panel is open.

### 6.5 M4 — Forget removes the row and keeps the key

1. Right-click the row `build-1`.

   Result: A menu shows `Focus` and `Forget this machine`.

2. Click `Forget this machine`.

   Result: The menu shows the line `Forget build-1? Its key stays.` with
   `Forget` and `Cancel`.

3. Click `Cancel`.

   Result: The row stays and nothing is written.

4. Right-click the row again, click `Forget this machine`, then `Forget`.

   Result: The row `build-1` is gone and `build-2` stays. The panel `build-1`
   is still open and still connected.

5. Examine the key directory.

   ```sh
   find <state>/data/home/.horizon/machine-keys -maxdepth 1 -name 'build-1*'
   ```

   Result: `build-1` and `build-1.pub` are there. A forget never deletes a key.

6. Close the panel `build-1` and restart the candidate.

   Result: The `Machines` section shows only `build-2`. The panel `build-1`
   restores from its own saved connection, which still names the key file on
   this computer.

### 6.6 M5 — A refused record opens nothing

1. Call `block_panel` with `{"operation":"machine_set","name":"Build","ssh":{"host":"127.0.0.1"}}`.

   Result: The answer is `code: invalid_machine_name`, and no row appears.

2. Call `block_panel` with
   `{"operation":"machine_set","name":"build-3","ssh":{"host":"127.0.0.1"},"session":"build 3"}`.

   Result: The answer is `code: invalid_session_name`, and no row appears.

3. Call `block_panel` with
   `{"operation":"machine_set","name":"build-3","ssh":{"host":"-oProxyCommand=id"}}`.

   Result: The answer is `code: invalid_ssh_route`, and no row appears.

4. Stop the candidate. In its private home, change the `host` of the record
   `build-2` to `-oProxyCommand=id` in `machines/registry.json`. Start the
   candidate again and click the row `build-2`.

   Result: No panel opens. The section shows the line
   `build-2: ... (invalid_ssh_route)` for ten seconds. Change the record back.

> **CAUTION: RESTORE THE RECORD.** The edited record is in the private home of
> the test only. Never edit the registry of your own Horizon.

### 6.7 M6 — An agent connects to a registered machine

1. From the agent panel, call `block_panel` with
   `{"operation":"open_terminal","machine":"build-2"}`.

   Result: The answer is `status: opened`. No second panel of `build-2` opens:
   the panel of the click becomes the panel of the agent.

2. Call it again with `{"operation":"open_terminal","machine":"build-2","key":"build-2"}`.

   Result: The answer is `code: invalid_request`, and the message says that
   `open_terminal` takes `key` from the record.

3. Call it with `{"operation":"open_terminal","machine":"build-9"}`.

   Result: The answer is `code: machine_unavailable`.

### 6.8 M7 — The row says that the terminal reconnects by itself

1. Click the row `build-1`, then run `printf 'row-%s\n' state` in its terminal.

   Result: `row-state` is the last output, and the dot of the row is filled in
   green.

2. Cut the link of the machine for 60 seconds: stop the loopback SSH server, or
   add a rule to the firewall of this computer that drops TCP port 2222.

   > **CAUTION: A FIREWALL RULE CHANGES THE NETWORK OF THIS COMPUTER.** Stop the
   > loopback SSH server instead when this computer has no test-only rule set.

   Result: The dot turns red within 30 seconds, and the hover text of the row
   says `disconnected`.

3. Watch the row for 10 seconds.

   Result: The dot is filled in the warning colour, and the hover text says
   `reconnecting`. The mark of the panel reads `Reconnecting` too.

4. Restore the link: start the loopback SSH server again, or remove the rule.

   Result: Within 30 seconds the dot is filled in green again, the hover text
   says `connected`, and the last output of the panel is still `row-state`.

### 6.8a M7a — A machine whose key was deleted

> **CAUTION: THIS TASK DELETES A KEY OF THE TEST HOME.** The key belongs to the
> private home of the fixture. A key of your own Horizon is not touched.

1. Call `block_panel` with `{"operation":"delete_machine_key","name":"build-2"}`,
   for the machine that the earlier tasks made.

   Result: The answer is `status: deleted_machine_key`. The three files of that
   key go from the key folder.

2. Click the row `build-2`.

   Result: No panel opens, and the section shows
   `The key of build-2 was deleted. Ask an agent to make a new key and install it.`
   for ten seconds. No `ssh` child starts for that machine.

3. Call `block_panel` with `{"operation":"machines"}`.

   Result: The record of `build-2` says `key_missing: true` and keeps its key
   name.

4. Call `block_panel` with `{"operation":"machine_key","name":"build-2"}`, and
   click the row again.

   Result: A new key pair exists, the answer says `created: true`, and the click
   opens a panel whose error is the one of ssh for a key that the machine does not
   accept yet. `machines` says `key_missing: false`.

### 6.9 M8 — Visual regression

1. Look at the section at the default sidebar width, then drag the window narrow
   and wide.

   Result: The rows keep one line each. A long name and a long provider line
   end with an ellipsis, and the hover text shows the whole name. The section
   stays below the last workspace row and inside the single scroll bar.

2. Click the empty canvas, then a row, three times.

   Result: The dot, the name and the provider text do not move or jump. No row
   flickers between two states.

## 7. Pass criteria

- The section shows no header and no row while this Horizon has no machine.
- A machine that an agent sets appears in name order, and its row names the
  machine, its provider line and whether a terminal of it is open.
- One click connects without an agent, and a second click focuses the same panel.
- A record that no longer passes its rule opens nothing and says the typed reason.
- A machine whose key was deleted opens nothing, says that the key was deleted,
  and reports `key_missing: true` to an agent.
- A terminal of a machine reconnects by itself after a lost link, and a key press
  in a panel whose attempt is in flight starts the new attempt at once.

## 8. Clean-up

1. Close the exact test window normally. Close the Device panel of the test
   through `device_panel`.

2. Stop the loopback SSH server and remove its temporary directory.

   Result: No process of the test is left, and the fixture-owned home and
   desktop are gone.

## 9. Limits of this procedure

- The loopback server answers in milliseconds, so the procedure does not test a
  slow first contact or a host that disappears.
- The procedure cut the link by stopping the loopback server, so the first
  attempt of a panel that waits meets a refused connection instead of a host
  that does not answer.
- The procedure does not test a machine without `tmux`.
- The procedure does not test more than two machines in the section.
- The task of the deleted key proves the notice and `key_missing` of one
  computer. The other computer that follows the same server is the task of
  [the server procedure](horizon-server.md).
