---
procedure: agent-attention
feature: The attention state of the agent panels in the sidebar
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Agent attention test procedure

## 1. Purpose

This procedure proves that the sidebar shows which agent panel needs the person,
which one finished while the person looked elsewhere, and which one works. It
also proves that the person can go to the next agent that needs them, that a look
clears a finished turn, and that the states come back after a restart.

## 2. Applicability

- Candidate: a Linux build that has the attention state of the agent panels, the
  jump shortcut `Ctrl+Shift+G` and the fixture option `--attention-agent`.
- Platforms: Linux. macOS uses the same code, and the shortcut has the same
  logical binding there.
- This procedure does not test: a real coding agent. The fixture runs a stand-in
  program that sends the lifecycle events itself.

## 3. Safety

No step rents compute, deletes data, sends a secret or changes access. The
fixture keeps its state in its own folder and touches no real Horizon home.

## 4. Equipment and preconditions

- A Linux computer with the fixture prerequisites of
  [the fixture guide](../../../scripts/device-smoke/README.md).
- A debug build of the candidate: `cargo build`.
- A free TCP port for the loopback VNC server, which the fixture chooses itself.
- A new evidence folder below `/var/tmp`, such as `/var/tmp/attention-1`.

## 5. Setup

1. Start the fixture, and show the attention stand-in:

   ```bash
   scripts/device-smoke/serve.py --horizon target/debug/horizon \
     --state /var/tmp/attention-1 --native-view --attention-agent
   ```

   Result: the fixture prints one JSON line with `vnc_address`. Horizon shows one
   workspace with one panel, **Attention stand-in**, and the panel prints
   `attention-stand-in: type one of stop, ask, ... and press Enter`.

2. Make a Device panel in your own Horizon workspace for that `vnc_address`, as
   [the fixture guide](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel)
   shows.

   Result: the Device panel shows the isolated desktop. The fixture starts the
   stand-in with `--timer 6`, so the events of the sequence arrive by themselves.

## 6. Tasks

### 6.1 A1 — The panel works, finishes and is done

1. Watch the stand-in panel and the sidebar row of its workspace for ten seconds.

   Result: the panel row says **Working** while the turn runs. When the turn ends
   the panel row says **Done**, its workspace row shows the accent check mark, and
   the second line of the workspace row starts with `1 done`.

### 6.2 A2 — The person clicks the row and the state clears

1. Click the panel row of the stand-in in the sidebar.

   Result: the panel gets the keyboard focus, and the word **Done** and the check
   mark are gone.

2. Type `stop` in the stand-in panel and press Enter. Then click the row of
   another workspace and back.

   Result: the state comes back as **Done** while the person looks at the other
   workspace.

### 6.3 A3 — An agent that asks needs the person

1. Type `start`, then `ask`, in the stand-in panel, and press Enter after each
   word.

   Result: the panel row says **Needs you**. The workspace row shows the filled
   warning badge with the `!` mark and its second line starts with `1 need you`.

2. Type `answer`.

   Result: the word changes to **Working**. The badge of the workspace is the
   green dot again.

3. Press `Ctrl+Shift+G` after you clicked another workspace.

   Result: Horizon switches to the workspace of the stand-in and the panel has the
   keyboard focus.

### 6.4 A4 — Background work and a failed turn

1. Type `background`.

   Result: the panel row says **Works in the background**, and the workspace badge
   stays the green working dot.

2. Type `start`, then `failed`.

   Result: the panel row says **Needs you** until the person clicks the row. A
   click clears a failed turn, also when its question was not answered.

### 6.5 A5 — The jump reaches every agent that waits

1. Type `stop` in the stand-in panel, then look at a different workspace so the
   panel is unseen.

2. Press `Ctrl+Shift+G` twice.

   Result: the first press goes to the stand-in panel. The second press goes on to
   the next panel that waits, or comes back to the stand-in when it is the only
   one.

3. Open a second workspace with a shell panel only, then press `Ctrl+Shift+G`
   until no agent waits.

   Result: the sidebar shows one line, **No agent needs you.**, and the focus does
   not change.

### 6.6 A6 — Mark as unread

1. Right-click the row of an idle agent panel and choose **Mark as unread**.

   Result: the row says **Done**, and its workspace row starts its second line
   with `1 done`.

2. Right-click the titlebar of the same panel and choose **Mark as unread**. Then
   click the row.

   Result: the menu entry is there, and the click clears the state again.

### 6.7 A7 — A restart keeps the state

1. Type `stop` and look at another workspace, so the panel is **Done**.

2. Stop the fixture with `Ctrl-C`, then start the same command again with
   `--reuse-state` added.

   Result: Horizon comes back with the same board. The stand-in panel sends its
   events again, and its row shows a state again within a second.

## 7. Pass criteria

- Each of the four states appears in the panel row and in the workspace badge as
  the tasks say.
- The count at the front of the status line names the agents that need the person.
- A look clears a finished turn and a failed turn. An open question stays.
- `Ctrl+Shift+G` goes to an agent that waits, across workspaces, and says one line
  when no agent waits.
- **Mark as unread** is in both menus and sets **Done** on an idle agent panel.
- No step needs a login of an agent.

## 8. Cleanup

1. Press `Ctrl-C` in the terminal of the fixture.

   Result: the fixture ends its processes and removes the private state of the
   run.

2. Close the Device panel of this run in your own Horizon.

   Result: the panel is gone and your own Horizon kept its terminals.

## 9. Record of results

Put the results in the pull request. Keep the screenshots and the video of the
isolated desktop out of the repository.
