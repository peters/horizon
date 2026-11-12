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
- Platforms: Linux. The candidate attaches the work ledger of an agent panel on
  Linux only, so this procedure runs on Linux. macOS uses the same sidebar code,
  and the shortcut has the same logical binding there.
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
- Two new evidence folders below `/var/tmp`: `/var/tmp/attention-timer` and
  `/var/tmp/attention-typed`.

## 5. Setup

The fixture starts the stand-in as the agent program of a Claude panel, so
Horizon gives the panel its own launch arguments and attaches the work ledger of
the panel. The stand-in sends its events through the hook entry of Horizon, and
the sidebar reads them.

1. Start the fixture for the tasks that use a timer:

   ```bash
   scripts/device-smoke/serve.py --horizon target/debug/horizon \
     --state /var/tmp/attention-timer --native-view --attention-agent
   ```

   Result: the fixture prints one JSON line with `vnc_address`. Horizon shows one
   workspace with three panels. The panel **Attention stand-in** is in front of
   the other two, and it prints:

   ```
   attention-stand-in: type one of start, ask, answer, stop, background, failed and press Enter
   ```

   Type `quit` and press Enter to end the script without a restart of the
   fixture.

   The menu of the panel row shows **Continue work after restart** as ticked. The
   fixture entry of this panel sets it, and it is the setting that attaches the
   work ledger.

   Result: the stand-in sends the events of a turn every six seconds, and the
   panel row of the sidebar shows the states of section 6.1.

2. Make a Device panel in your own Horizon workspace for that `vnc_address`, as
   [the fixture guide](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel)
   shows.

   Result: the Device panel shows the isolated desktop.

3. Type in the panel at any time, also while the timer runs: a typed word is sent
   at once and the timer goes on.

4. For the tasks that type the words, stop the fixture with `Ctrl-C`, then start
   it without a timer:

   ```bash
   scripts/device-smoke/serve.py --horizon target/debug/horizon \
     --state /var/tmp/attention-typed --native-view --attention-agent 0
   ```

   Result: the stand-in prints its line and sends nothing. Its row shows no state
   until the person types a word.

## 6. Tasks

Each task names the command that it uses: **timer** is the command of step 1 of
section 5, and **typed** is the command of step 4.

### 6.1 A1 — The panel works, finishes and is done (timer)

1. Watch the stand-in panel and the sidebar row of its workspace for twenty
   seconds.

   Result: the panel row says **Working** while a turn runs. When a turn ends the
   panel row says **Done**, its workspace row shows the accent check mark, and the
   second line of the workspace row starts with `1 done`.

### 6.2 A2 — The person clicks the row and the state clears (timer)

1. Wait for **Done**, then click the panel row of the stand-in in the sidebar.

   Result: the panel gets the keyboard focus, and the word **Done** and the check
   mark are gone.

2. Click the row of the other workspace, then wait for the next turn that ends.

   Result: the state comes back as **Done** while the person looks at the other
   workspace.

### 6.3 A3 — An agent that asks needs the person (typed)

1. Type `start`, then `ask`, in the stand-in panel, and press Enter after each
   word.

   Result: the panel row says **Needs you**. The workspace row shows the filled
   warning badge with the `!` mark and its second line starts with `1 needs you`.

2. Type `answer`.

   Result: the word changes to **Working**. The badge of the workspace is the
   green dot again.

3. Press `Ctrl+Shift+G` after you clicked another workspace.

   Result: Horizon switches to the workspace of the stand-in and the panel has the
   keyboard focus.

### 6.4 A4 — Background work and a failed turn (typed)

1. Type `start`, then `background`.

   Result: the panel row says **Works in the background**, and the workspace badge
   stays the green working dot.

2. Type `start`, then `failed`.

   Result: the panel row says **Needs you** until the person clicks the row. A
   click clears a failed turn, also when its question was not answered.

### 6.5 A5 — The jump reaches every agent that waits (typed)

1. Type `start`, then `stop`, in the stand-in panel. Then look at a different
   workspace, so the panel is unseen.

2. Press `Ctrl+Shift+G` twice.

   Result: the first press goes to the stand-in panel. The second press goes on to
   the next panel that waits, or comes back to the stand-in when it is the only
   one.

3. Add a second workspace with a shell panel only, then press `Ctrl+Shift+G` until
   no agent waits.

   Result: the sidebar shows one line, **No agent needs you.**, in the muted text
   of the sidebar and not in the red of an error. The focus does not change.

### 6.6 A6 — Mark as unread (typed)

1. Right-click the row of an idle agent panel and choose **Mark as unread**.

   Result: the row says **Done**, and its workspace row starts its second line
   with `1 done`.

2. Right-click the titlebar of the same panel and choose **Mark as unread**. Then
   click the row.

   Result: the menu entry is there, and the click clears the state again.

### 6.7 A7 — A restart starts the same board again (timer or typed)

1. Type `stop` and look at another workspace, so the panel is **Done**.

2. Stop the fixture with `Ctrl-C`, then start the same command again with
   `--reuse-state` added.

   Result: the fixture keeps its private home, and Horizon shows the same two
   workspaces and panels again, because the fixture starts the candidate as a
   session that saves nothing. The stand-in starts again and prints its line, and
   its row shows a state again within a second.

   The finish that was unread before the restart is gone: the unread mark lives in
   the window and is not saved. The panel of this run is a new panel with its own
   work ledger, so the row starts empty.

## 7. Pass criteria

- Each of the four states appears in the panel row and in the workspace badge as
  the tasks say.
- The count at the front of the status line names the agents that need the person.
- A look clears a finished turn and a failed turn. An open question stays.
- A typed word reaches the sidebar while the timer of the fixture runs.
- `Ctrl+Shift+G` goes to an agent that waits, across workspaces, and says one line
  when no agent waits. That line is not drawn as an error.
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
