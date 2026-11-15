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
  jump shortcut `Ctrl+Shift+G` and the fixture options `--attention-agent`,
  `--attention-agent-command` and `--attention-agent-kind`.
- Platforms: Linux, because the fixture needs an isolated X11 desktop. The report
  of a panel runs on Linux and on macOS, and the automated tests of the candidate
  prove the macOS lane; a live macOS run needs a real agent and is not part of
  this procedure.
- This procedure does not test: a real coding agent. The fixture runs a stand-in
  program that sends the lifecycle events itself, a stand-in panel whose preset
  has a command of its own, and a stand-in codex panel that writes the state
  titles of the real program.

## 3. Safety

No step rents compute, deletes data, sends a secret or changes access. The
fixture keeps its state in its own folder and touches no real Horizon home.

## 4. Equipment and preconditions

- A Linux computer with the fixture prerequisites of
  [the fixture guide](../../../scripts/device-smoke/README.md).
- A debug build of the candidate: `cargo build`.
- A free TCP port for the loopback VNC server, which the fixture chooses itself.
- Five new evidence folders below `/var/tmp`: `/var/tmp/attention-timer`,
  `/var/tmp/attention-typed`, `/var/tmp/attention-pi`,
  `/var/tmp/attention-command` and `/var/tmp/attention-codex`.

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
   attention-stand-in: type one of start, ask, answer, stop, background, failed, or "after SECONDS WORD", and press Enter
   ```

   Type `quit` and press Enter to end the script without a restart of the
   fixture.

   The menu of the panel row shows **Continue work after restart** as ticked. The
   fixture entry of this panel sets it, and it is the setting that attaches the
   work ledger.

   Result: the stand-in sends the events of a turn every six seconds, and the
   panel row of the sidebar shows the states of section 6.1.

   The canvas also shows the card **Review work before continuing (1)** with the
   line `Automatic continuation is not verified for this session.` and the button
   **Resume work**. The card is expected and it is no defect of the stand-in. A
   panel of the fixture comes from the config of the run, so Horizon restores it,
   and this private home holds no saved work for the conversation of the panel,
   so Horizon asks before it continues. That decision comes from the launch of
   the panel and not from its program, so a real agent panel shows the same card
   in this situation, and the events of the stand-in never change it. One
   keystroke in the panel clears the card, and so does the button **Resume work**,
   which restarts the panel. The typed tasks of section 6 clear it. A task that
   only watches the timer types nothing, so the card stays there until the person
   types into the panel.

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

5. For the tasks of the pi panel, stop the fixture with `Ctrl-C`, then start it
   with the pi agent kind:

   ```bash
   scripts/device-smoke/serve.py --horizon target/debug/horizon \
     --state /var/tmp/attention-pi --native-view --attention-agent 0 \
     --attention-agent-kind pi
   ```

   Result: the board shows one panel, **Pi attention stand-in**. Horizon gives
   its launch the state report extension of this candidate with `-e`, and the
   report runs under `node` when this computer has it. The stand-in prints:

   ```
   pi-attention-stand-in: the report runs under node; harness: 8 events of the report are subscribed; type start, ask, answer, stop, esc, quit
   ```

   A computer without `node` prints the line `no node on PATH, so this stand-in
   sends the documents of the report` instead. The states of section 6.9 appear
   in both cases, and only the line of the stand-in differs. A computer without
   `node` does not prove the extension file itself; the test
   `cargo test -p horizon-ui --test pi_report_extension` proves it where `node`
   exists.

   Result: the row of the panel shows no state until the person types a word,
   because the screen of the stand-in prints no working indicator and the report
   of its launch has sent no event yet. The panel shows no card of a
   continuation, because a pi panel continues no work after a restart.

6. For the tasks that use a command of its own, stop the fixture with `Ctrl-C`,
   then start it with the second shape of the panel entry:

   ```bash
   scripts/device-smoke/serve.py --horizon target/debug/horizon \
     --state /var/tmp/attention-command --native-view --attention-agent 0 \
     --attention-agent-command
   ```

   Result: the board shows two stand-in panels. The second one is **Attention
   stand-in with its own command**: the preset of that panel runs the stand-in
   itself, and **Continue work after restart** is not ticked in its row menu. It
   shows no card of a continuation, because the panel continues nothing.

   Result: its row shows no state until its agent reports its first event, because
   the screen of the stand-in prints no working indicator.

## 6. Tasks

Each task names the command that it uses: **timer** is the command of step 1 of
section 5, **typed** is the command of step 4, **command** is the command of
step 6, and **pi** is the command of step 5.

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

   Result: the panel row says **Background**, and the workspace badge
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

### 6.8 A8 — A panel with a command of its own reports its state (command)

1. Look at the sidebar row of **Attention stand-in with its own command** and
   press Enter in that panel without a word.

   Result: the row shows no state. The panel has a work ledger, and no event of
   its launch has arrived yet, so the screen of the panel decides, and its screen
   shows no working indicator.

2. Type `start` in that panel.

   Result: the row says **Working**, and it comes from the ledger of the panel.
   The card **Review work before continuing** is not there, and no prompt of
   Horizon is in the panel.

3. Type `stop`.

   Result: the row says **Done** when the panel did not have the focus, and the
   workspace row counts it in its second line.

4. Close the panel with the close button of its titlebar.

   Result: the row is gone, and the state of the closed panel does not come back.

### 6.9 A9 — A pi panel reports its turn (pi)

1. Type `start` in the panel and press Enter.

   Result: the panel row says **Working** within one second.

2. Wait three seconds without a word.

   Result: the row still says **Working**. The screen of the stand-in prints no
   spinner, and the report of the launch holds the turn open. The pause is the
   stale timer of a working indicator of a screen, and it does not end a turn
   that the report holds.

3. Type `ask`.

   Result: the row says **Needs you**. It comes from a dialog of an extension,
   which is the only exact wait that a pi panel has.

4. Type `answer`.

   Result: the row says **Working**.

5. Type `after 3 stop`, then click the row of the other panel within three
   seconds.

   Result: at the end of the three seconds the row of the pi panel says
   **Done**, and the workspace row counts it in its second line. The panel has no
   focus at that moment, so the finished turn is unread. A look at the pi panel
   clears the word.

6. Type `start` in the pi panel, then `after 3 esc`, then click the row of the
   other panel within three seconds.

   Result: after `start` the row says **Working** again. At the end of the three
   seconds it says **Done**, and it is the state of an interrupted turn: the
   report names the aborted run of pi.

7. Type `quit`.

   Result: the stand-in ends, the panel shows that its program ended, and its row
   shows no **Working** and no **Needs you**.

### 6.10 A10 — A pi panel of a command of its own reports its state (pi)

1. Stop the fixture with `Ctrl-C`, then start it again with `--attention-agent 0
   --attention-agent-kind pi --attention-agent-command`.

   Result: the board shows two pi panels. The second one is **Pi attention
   stand-in with its own command**, and its row menu shows **Continue work after
   restart** unticked. Both panels show no state until a word.

2. Type `start` in the second panel, then `stop`.

   Result: the row of that panel says **Working** and then **Done**. The state of
   a pi panel that runs a command of its own therefore comes from the report, as
   it does for a Claude panel.

### 6.11 A11 — A codex panel reports its turn through its title (codex)

1. Stop the fixture with `Ctrl-C`, then start it with the codex agent kind:

   ```bash
   scripts/device-smoke/serve.py --horizon target/debug/horizon \
     --state /var/tmp/attention-codex --native-view --attention-agent 0 \
     --attention-agent-kind codex
   ```

   Result: the board shows one panel, **Codex attention stand-in**. The stand-in
   prints one line. The row of the panel shows no state, because no title of the
   launch has claimed one.

2. Watch the row of the panel while the program starts.

   Result: the row shows no state during the launch, and the workspace line
   counts no finished turn. The stand-in writes eight frames of its spinner, a
   tenth of a second apart, and then the plain title of the folder. The frames
   before that title name no turn of yours, so they claim nothing.

3. Type `start` and press Enter.

   Result: the row says **Working** within one second. The launch of this panel
   receives no file and no argument for this state: the stand-in writes a braille
   spinner frame and the name of its folder into the title of the panel.

4. Wait three seconds without a word.

   Result: the row still says **Working**. The screen of the stand-in prints no
   working indicator, so this state comes from the title alone.

5. Type `spin 1000`.

   Result: the row stays **Working**. One thousand frames of the same state
   change nothing, and the window shows no other word.

6. Type `ask`, and watch the name of the row of that panel and the name in the
   titlebar of the panel.

   Result: the row says **Needs you**. The stand-in writes the two frames of
   `Action Required | Create a file | work`, twice a second, and the names do not
   change while they blink: both names end with `work`, the folder of the panel,
   and no name holds `Action Required` or the words of the request.

7. Type `answer`.

   Result: the row says **Working** again, in the same turn, and the names are the
   same names.

8. Type `ask` again, then type `deny`.

   CAUTION: `deny` answers an approval that the person refused. It types no
   permission and changes no file.

   Result: while the approval waits the row says **Needs you**. After `deny` the
   row leaves **Needs you** within one second: it says **Done** while the panel
   does not have the focus, and no word while it has. The turn was stopped and not
   finished, and the request that you refused closed with it. The stand-in writes
   the plain title of a stopped turn for `deny`, which is the title that the real
   program writes after a denial.

9. Type `plain 0.1`.

   Result: the row stays **Working**. The stand-in writes a plain title for
   0.1 seconds in the middle of the turn, and the settle time of 0.4 seconds
   keeps the turn open.

10. Click the row of the other panel, then type `after 3 stop` in the codex panel.

   Result: at the end of the three seconds the row of the codex panel says
   **Done**. The plain title of the stand-in ended the turn, and the panel had no
   focus at that moment. A click on the row of the codex panel clears the word.

11. Type `start`, click the row of the other panel, then type `after 3 esc`.

   Result: after `start` the row says **Working**. At the end of the three
   seconds it says **Done**. An interrupted turn ends as a finished turn, because
   the title of Codex names no interruption.

12. Type `quit`.

    Result: the stand-in ends, the panel shows that its program ended, and its row
    shows no **Working** and no **Needs you**.

13. Press `Ctrl-C` in the terminal of the fixture, then start the same command
    again.

    Result: the codex panel of the new run starts on the rule of its screen. No
    state of a title survives a restart of Horizon.

### 6.12 A12 — A codex panel reports its turn through the program status protocol (codex)

The program status protocol (OSC 7501) is the exact report of the program. A
panel reads it in the reader of its terminal, beside the title. The report
answers before the title does.

1. Stop the fixture with `Ctrl-C`, then start it with the codex agent kind and
   the report of the protocol:

   ```bash
   scripts/device-smoke/serve.py --horizon target/debug/horizon \
     --state /var/tmp/attention-codex-status --native-view --attention-agent 0 \
     --attention-agent-kind codex --attention-agent-protocol
   ```

   Result: the board shows one panel, **Codex attention stand-in**. The stand-in
   writes `ESC ] 7501 ; state=working:app=codex ST` beside the titles of a turn.
   Watch the isolated desktop in a native VNC Device panel of your own Horizon.

2. Watch the row of the panel while the program starts.

   Result: the row shows no state. The launch reports `state=working` while it
   loads, and it reports `state=idle` when it is ready for you. A report before
   that idle names no turn of yours, so the row and the workspace line claim
   nothing.

3. Type `start` and press Enter.

   Result: the row says **Working** within one second. The report of the program
   claims the turn, and no title is needed for it.

4. Type `ask`, and watch the name of the row of that panel and the name in the
   titlebar of the panel.

   Result: the row says **Needs you**. The report is
   `state=blocked:app=codex:kind=permission`, and the two frames of the title blink
   beside it. The names of the row and of the titlebar stay the same while they
   blink, and both end with `work`, the folder of the panel.

5. Type `answer`.

   Result: the row says **Working** again, in the same turn.

6. Type `ask` again, then type `deny`.

   Result: while the approval waits the row says **Needs you**. After `deny` the
   row leaves **Needs you** within one second: it says **Done** while the panel
   does not have the focus, and no word while it has. The program reports
   `state=idle` and no done, and the record of the turn keeps an interrupted turn,
   so Horizon offers no continuation for it.

7. Click the row of the other panel, then type `after 3 stop`.

   Result: at the end of the three seconds the row of the codex panel says
   **Done**. The report `state=done` ended the turn, and the panel had no focus.

8. Click the row of the codex panel, then type `start` and `after 3 esc`.

   Result: the row says **Working**, and after three seconds it says **Done**. An
   interrupted turn reports `state=idle` and no done.

9. Type `start`, then `fail`.

   Result: the row says **Needs you**. A failed turn reports `state=error`, and
   the title of the program cannot report a failure. This is the step that a
   titles-only panel cannot pass.

10. Type `foreign`.

   Result: the row does not change. The report names another program in its `app`
   value, so it claims nothing for this panel.

11. Type `work 1000`.

   Result: the row stays as it is, and the window shows no other word. One
   thousand reports of the state that stands change nothing.

12. Type `quit`.

    Result: the stand-in ends, the panel shows that its program ended, and its row
    shows no **Working** and no **Needs you**.

13. Stop the fixture with `Ctrl-C`, start it again without
    `--attention-agent-protocol`, and type `start` and then `ask`.

    Result: the row says **Working** and then **Needs you**, from the title alone.
    A launch that reports no state of the protocol keeps the path of its titles.

**Pass criteria for A12.** Every step above shows the word that it names. A
repeated report changes no word and no badge. A report of another program changes
nothing.

## 7. Pass criteria

- Each of the four states appears in the panel row and in the workspace badge as
  the tasks say.
- The count at the front of the status line names the agents that need the person.
- A look clears a finished turn and a failed turn. An open question stays.
- A typed word reaches the sidebar while the timer of the fixture runs.
- `Ctrl+Shift+G` goes to an agent that waits, across workspaces, and says one line
  when no agent waits. That line is not drawn as an error.
- **Mark as unread** is in both menus and sets **Done** on an idle agent panel.
- A panel whose preset has a command of its own and whose continuation is off
  shows its state, shows no card of a continuation, and keeps the screen rule
  until its first event.
- A pi panel reaches **Working**, **Needs you** and **Done** from the report of
  its launch, keeps **Working** while its screen is still, shows **Done** after an
  aborted run, and shows no claim after its program ended.
- A codex panel reaches **Working**, **Needs you** and **Done** from the title of
  its own terminal, keeps **Working** while its screen is still, keeps its turn
  through a plain title of a tenth of a second, shows **Done** after an
  interrupted turn, and shows no claim after its program ended.
- A codex panel that reports the program status protocol reaches **Working**,
  **Needs you** and **Done** from the report of the program, shows **Needs you**
  for a failed turn, changes no word for a repeated report or for a report of
  another program, and shows no claim after its program ended.
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
