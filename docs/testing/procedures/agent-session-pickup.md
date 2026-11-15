---
procedure: agent-session-pickup
feature: The pickup of the agent sessions on the first start page, and the start in a folder
platforms: [linux, macOS]
cost: none
destructive: no
secrets: none
owner: peters
---

# Agent session pickup and start in a folder test procedure

This procedure proves that Horizon offers the conversations that the agent
programs of this computer keep, that a press of `Open` continues one of them, and
that `horizon <folder>` shows the board of one folder.

## 1. Purpose

Prove these results:

- The section **Pick up where you left off** shows the conversations of Claude
  Code and Codex, and a press continues one of them in its folder.
- The window **Recent agent sessions** of the Menu shows the full list.
- `horizon <folder>` shows the workspace of that folder.
- A path that is not a folder ends the start with one sentence.
- `horizon <folder>` beside a running Horizon shows the folder in that window.

## 2. Applicability

- Candidate: any build of `main` after this procedure was added.
- Platforms: Linux and macOS. The stores of the two agent programs are the same
  on both systems.
- This procedure does not test: the deletion of saved conversations, the
  conversations of other agent programs, and two live windows of one home at the
  same time.

## 3. Safety

> **CAUTION:** DO NOT USE YOUR OWN HOME FOLDER FOR THIS PROCEDURE. The steps make
> session stores in a private home. A mistake can delete or change the
> conversations of your own agent programs.

> **CAUTION:** DO NOT DELETE A FOLDER THAT THE STEPS DO NOT NAME. Delete only the
> private home of step 5.

## 4. Equipment and preconditions

- A Linux or macOS computer with a build of the candidate.
- The programs `claude` and `codex` on `PATH`. A step also runs without one of
  them.
- A terminal.

## 5. Setup

1. Make a private home and two folders with conversations in them:

   ```bash
   export PRIVATE_HOME=$(mktemp -d /tmp/horizon-pickup-XXXXXX)
   mkdir -p "$PRIVATE_HOME/.claude/projects/-repo-one" "$PRIVATE_HOME/.claude/sessions"
   mkdir -p "$PRIVATE_HOME/.codex" "$PRIVATE_HOME/work/repo-one" "$PRIVATE_HOME/work/repo-two"
   python3 - "$PRIVATE_HOME" <<'EOF'
   import json, sys
   home = sys.argv[1]
   def claude(folder, session, prompt):
       record = {"type": "user", "sessionId": session, "cwd": folder, "slug": "a-slug"}
       line = {"type": "last-prompt", "lastPrompt": prompt}
       path = f"{home}/.claude/projects/-repo-one/{session}.jsonl"
       open(path, "w").write(json.dumps(record) + "\n" + json.dumps(line) + "\n")
   claude(f"{home}/work/repo-one", "00000000-0000-0000-0000-000000000101", "Fix the build")
   claude(f"{home}/work/repo-one", "00000000-0000-0000-0000-000000000102", "Write the test")
   claude(f"{home}/work/repo-two", "00000000-0000-0000-0000-000000000103", "Read the log")
   EOF
   ```

   Result: The private home holds three conversations in two folders.

2. Start Horizon with that home and the default board:

   ```bash
   HOME=$PRIVATE_HOME ./target/debug/horizon
   ```

   Result: The first start page shows, with four tiles.

## 6. Tasks

### 6.1 P1 — The section shows the conversations of the two folders

1. Examine the page under the four tiles.

   Result: The section **Pick up where you left off** shows. It has two rows, and
   each row names a folder, a title, the program and a time.

2. Push Tab until a row has the focus ring, then push Enter.

   Result: Horizon shows the workspace of that folder and opens one panel of the
   program of the row. The panel continues that conversation.

### 6.2 P2 — The full list and the second confirmation

1. Open the Menu of the titlebar and press **Recent agent sessions**.

   Result: A window lists the conversations, grouped by folder. Each folder with
   more than one conversation has **Open all**.

2. Press `Open all` on the folder of the two conversations.

   Result: One panel opens for each conversation of that folder.

3. Make a live registry entry for one conversation that is still in the list:

   ```bash
   pid=$$
   printf '{"pid": %s, "sessionId": "00000000-0000-0000-0000-000000000103"}' "$pid" \
     > "$PRIVATE_HOME/.claude/sessions/$pid.json"
   ```

   Result: After the next read of the list, that row says `open in another
   program`.

4. Press `Open` on that row.

   Result: No panel opens. The window says that the program has the conversation
   open now. The press on `Open anyway` opens the panel.

### 6.3 P3 — A folder that is gone

1. Remove the folder of one row:

   ```bash
   rm -rf "$PRIVATE_HOME/work/repo-two"
   ```

2. Press `Show all` and examine the row of that folder.

   Result: The row says `folder is gone`, and its press does nothing.

### 6.4 P4 — Start in a folder

1. Close Horizon. Start it in one folder of the private home:

   ```bash
   HOME=$PRIVATE_HOME ./target/debug/horizon "$PRIVATE_HOME/work/repo-one"
   ```

   Result: Horizon shows a workspace with the name `repo-one` and one panel in it.
   The panel runs in `repo-one`. The keyboard is in that panel.

2. Start Horizon in the same folder again.

   Result: The workspace `repo-one` shows. Horizon makes no second one.

3. Stop Horizon. Type `horizon "$PRIVATE_HOME/work/repo-one/a-file"` after
   `touch "$PRIVATE_HOME/work/repo-one/a-file"`.

   Result: One sentence on the standard error says that the path is not a folder,
   the status is 2, and no window opens.

4. Type `horizon "$PRIVATE_HOME/work/not-there"`.

   Result: One sentence on the standard error says that the path is not there, the
   status is 2, and no window opens.

### 6.5 P5 — A folder start beside a running Horizon

1. Start Horizon with `HOME=$PRIVATE_HOME ./target/debug/horizon` and wait until
   its window shows.

2. In another terminal, type `HOME=$PRIVATE_HOME ./target/debug/horizon
   "$PRIVATE_HOME/work/repo-two"`.

   Result: The running window comes to the front within five seconds. It shows a
   workspace with the name `repo-two` and one panel in it, in that folder. The
   second start writes one line on the standard output, `Horizon made a workspace
   for the folder <path>.`, and ends with status 0. No second window opens.

3. Type the same command again.

   Result: The running window shows the workspace `repo-two` again and adds no
   panel. The second start writes the line `Horizon shows the folder <path>.`, and
   ends with status 0.

4. In the running window, close every panel of every workspace. Type
   `HOME=$PRIVATE_HOME ./target/debug/horizon "$PRIVATE_HOME/work/repo-one"`.

   Result: The window shows a workspace with the name `repo-one`, also while no
   panel of it is open. The second start writes one line and ends with status 0.

5. Pause the running window, so it answers nothing. The shell of step 1 gives its
   process id:

   ```bash
   kill -STOP "$WINDOW_PID"
   HOME=$PRIVATE_HOME ./target/debug/horizon "$PRIVATE_HOME/work/repo-two"
   kill -CONT "$WINDOW_PID"
   ```

   Result: After about five seconds the second start writes one line on the
   standard output, `Horizon runs for this home, but it did not answer, so the
   folder <path> was not opened there.`. The status is 0, and it opens no window
   of its own. The window shows the folder `repo-two` when the step resumes it,
   because the request is younger than ten seconds.

6. Name a folder that is gone when the window takes the request. Pause the window
   again, start the second start in the background, and remove the folder before
   the window runs:

   ```bash
   mkdir -p "$PRIVATE_HOME/work/repo-three"
   kill -STOP "$WINDOW_PID"
   HOME=$PRIVATE_HOME ./target/debug/horizon "$PRIVATE_HOME/work/repo-three" &
   sleep 0.3
   rmdir "$PRIVATE_HOME/work/repo-three"
   kill -CONT "$WINDOW_PID"
   wait
   ```

   Result: One line on the standard error says that Horizon did not show the folder
   and names `folder_gone`. The status is 2, the window makes no workspace, and no
   second window opens.

7. In the home of the running window, examine the requests after each step:

   ```bash
   ls -l "$PRIVATE_HOME/.horizon/runtime/folder-requests"
   ```

   Result: The folder holds no file after a start that a window answered or that
   ended its wait.

## 7. Pass criteria

- P1: the section shows two folders, and each press continues the conversation of
  its row.
- P2: the window shows every conversation, `Open all` opens the rows of one
  folder, and a conversation of another program asks once more before it opens.
- P3: a row of a folder that is gone says so and opens nothing.
- P4: the workspace of the folder shows with one panel, and a second start makes
  no second workspace. A path that is not a folder ends with one sentence and
  status 2.
- P5: a folder start beside a running window shows the folder in that window and
  writes the one line of its answer, also when the window has no panel open. A
  folder that is gone is answered `folder_gone` with status 2, and a window
  without a session takes no request.
- The page without a conversation in the private home shows no section.

## 8. Cleanup

1. Close Horizon.
2. Remove the private home:

   ```bash
   rm -rf "$PRIVATE_HOME"
   ```

   Result: The conversations of the procedure are gone. Your own home folder was
   not touched.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request.
