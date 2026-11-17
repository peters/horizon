---
procedure: agent-kept-through-restart
feature: The program of an agent panel that outlives a restart of Horizon
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Agent program kept through a restart test procedure

This procedure proves that the setting `Keep agent programs running when Horizon
restarts` runs the program of an agent panel in a holder, that the program keeps
running when Horizon ends, and that a new Horizon attaches to the same program and
shows its last output.

## 1. Purpose

Prove these results:

1. A new agent panel runs its program in a holder, and the panel shows the output
   of that program.
2. The program keeps its pid when Horizon ends and starts again in the same
   session.
3. A closed panel ends its program.

## 2. Applicability

- Candidate: a build that holds the setting `features.keep_agents_through_restart`.
- Platforms: Linux. The holder is not proven on macOS.
- This procedure does not test a panel of a machine and an ssh panel. No panel
  of these kinds uses a holder.
- Task K5 needs a real agent program and a Horizon that installs its agent
  integrations. The stand-in program has no hooks and no tools.

## 3. Safety

> **CAUTION:** A HOLDER KEEPS AN AGENT PROGRAM RUNNING AFTER HORIZON ENDS. AN
> AGENT THAT RUNS WITH NOBODY LOOKING AT IT CAN COST MONEY AND CAN CHANGE FILES.
> CLOSE THE PANEL BEFORE YOU END THE TASK. THE PROCEDURE STOPS EVERY HOLDER IN
> ITS CLEANUP.

## 4. Equipment and preconditions

- The device fixture of this repository, `scripts/device-smoke/serve.py`.
- A state path of this task.
- The stand-in program `scripts/device-smoke/agent_attention_standin.py`, which
  the fixture copies into its state folder.
- A native VNC Device panel in the workspace of the agent, which shows the
  isolated desktop of the fixture.

## 5. Setup

1. Start the fixture with the setting on and one agent panel of the stand-in
   program, for example:

   ```bash
   python3 scripts/device-smoke/serve.py --state /var/tmp/hz-kept-1 \
     --saved-session --reuse-state --keep-agents \
     --attention-agent --native-view
   ```

   `--attention-agent` takes an optional number of seconds, and the default is
   `6`. A value that is not a number stops the fixture.

   Result: The fixture starts its own desktop, and Horizon opens with an agent
   panel.

2. Read the pid of the program of the panel:

   ```bash
   ls "$HOME/.horizon/panel-holders" 2>/dev/null
   ```

   The private home of the fixture holds the folder `panel-holders`, and its
   socket is named by 32 hexadecimal characters of the SHA-256 of the local id of
   the panel.

   Result: The folder exists and holds one socket.

## 6. Tasks

### 6.1 K1 — The program of a new agent panel runs in a holder

1. Read the process tree of the candidate:

   ```bash
   ps -o pid,ppid,pgid,cmd -g "$(ps -o sid= -p <candidate pid>)"
   ```

   Result: One process of the candidate is the holder of the panel, and the
   program of the panel is a child of that process.

2. Read the pid of the program of the panel, and keep it.

   Result: The pid is the value of this task.

### 6.2 K2 — Horizon ends and the program keeps running

The first run of the fixture ends when its candidate ends. The fixture keeps the
private home and the board of the state path. Step 6.2.2 starts the fixture again
with `--reuse-state --resume-session` in that state path, so the second run shows
the board of the first run.

1. Quit Horizon with the close request of its window, as the close button of the
   window manager sends it:

   ```bash
   DISPLAY=<display> xdotool search --pid <candidate pid> --name '^Horizon$' windowquit
   ```

   Do not use `xdotool windowclose`: it destroys the window, and the candidate
   ends with an error of its window library instead of the path of this test.

   Result: Horizon ends. The holder process and the program of the panel still
   run, and the socket of the holder is still in the folder `panel-holders` of
   the private home.

2. Start the fixture again in the same state path, without `--keep-agents`:

   ```bash
   python3 scripts/device-smoke/serve.py --state /var/tmp/hz-kept-1 \
     --reuse-state --resume-session --native-view
   ```

   Result: Horizon opens the saved board, and the agent panel shows the text of
   the program from before the quit.

3. Compare the pid of the program of the panel with the pid of step 6.1.

   Result: The pid is the same. The panel attached to the program of the earlier
   run and did not start a new one.

4. Type a command in the panel and read its answer.

   Result: The program answers, and the panel shows the answer.

### 6.3 K3 — A closed panel ends its program

1. Close the agent panel with its menu item **Close**.
2. Read the folder `panel-holders` of the private home of the fixture.

   Result: The socket of the panel is gone.

3. Read the process tree of the candidate, as in step 6.1.

   Result: The holder of the panel and the program of the panel are gone.

### 6.4 K4 — A restart of the panel starts a new program

1. Open a new agent panel of the stand-in program.
2. Read the pid of its program.
3. Use **Restart** in the menu of the panel.
4. Read the pid of its program again.

   Result: The pid changed, and the panel shows the output of the new program.

> **CAUTION:** A HOLDER OF A PANEL THAT THIS STEP LEAVES OPEN KEEPS RUNNING AFTER
> HORIZON ENDS. DO STEP 8 BEFORE YOU END THE TASK.

### 6.5 K5 — The hooks and the tools of a real agent after a restart

Do this task with a real agent program, a Horizon that installs its agent
integrations, and a private Horizon home of this task.

1. Open a Claude panel and send it one message. Read the folder
   `runtime/agent-plugins` of the Horizon home.

   Result: The folder holds one host folder, its `.lock` file and its `.held`
   file.

2. Quit Horizon as in step 6.2.1, and read the folder again.

   Result: The host folder and its files are there, because the holder of the
   panel runs.

3. Start Horizon again in the same session. Read the folder and the log.

   Result: The folder holds the same host folder and no second one. The log has
   the line `agent programs of an earlier Horizon run on in holders` with
   `taken=true`.

4. Send the agent one message.

   Result: The panel shows no `hook error` line.

5. Tell the agent to call `block_panel` with the operation `list`.

   Result: The call returns in less than 10 seconds.

6. Close the panel and quit Horizon. Read the folder again.

   Result: The folder holds no host folder, because no holder runs.

## 7. Pass criteria

- K1 proves that the program of a new agent panel is a child of a holder.
- K2 proves the same pid and the same text after the quit.
- K3 proves that a closed panel leaves no holder and no program.
- K4 proves that a restart starts a new program.
- K5 proves that the hooks and the tools of a real agent work after a restart.
- No holder of the task is left after the cleanup.

## 8. Cleanup

1. Close every agent panel of the fixture, or quit the fixture and start it again
   in the same state path without `--keep-agents`. The start ends the programs of
   the holders that no panel shows, and one line of the sidebar names the count.

   Result: The folder `panel-holders` holds no socket.

2. Stop the fixture.
3. Delete the state path.

   Result: The private home and the evidence of the task are gone.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
