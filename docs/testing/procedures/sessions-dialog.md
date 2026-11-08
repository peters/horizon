---
procedure: sessions-dialog
feature: The Sessions dialog of the toolbar Menu
platforms: [linux]
cost: none
destructive: yes
secrets: none
owner: peters
---

# Sessions dialog test procedure

## 1. Purpose

This procedure makes sure that a person can open the Sessions dialog again
after a dismissal. The dialog stays above its backdrop. The dialog removes a
saved session. For a session whose board the candidate cannot read, the dialog
says so in its question before it removes that session.

## 2. Applicability

- Candidate: each candidate that changes the Sessions dialog, its backdrop or
  `SessionStore::delete_session`.
- Platforms: Linux, with the fixture.
- This procedure does not test: the switch between sessions, the browser
  profile cleanup of a board that the candidate can read, and the cloud cards
  of a session.

## 3. Safety

> **CAUTION:** REMOVE A SESSION ONLY IN THE PRIVATE HOME. A session holds the
> board of a person. If you remove the session of a person, that person loses
> that board.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The [fixture](../../../scripts/device-smoke/README.md).
- A Device panel that shows a live view of the isolated desktop.
- A state path that this task owns.
- These fixture arguments:
  - `--native-view`
  - `--saved-session`
  - `--reuse-state`

## 5. Setup

1. Start the fixture with the frozen candidate, the three arguments and the
   state path of this task.

   Result: The fixture prints a manifest with a `vnc_address`.

2. Open that `vnc_address` in a Device panel.

   Result: The Device panel shows the candidate window.

3. Close the candidate window with the close button of the window manager.

   Result: The candidate stops. The fixture keeps its private home.

4. Copy the session folder of the first run to a new identifier:

   ```sh
   sessions=<state>/data/home/.horizon/sessions
   cp -r "$sessions/<first>" "$sessions/0950d159-852d-484c-a9b7-49fb4dc481d0"
   ```

   Result: The private home holds two session folders.

5. Write `meta.yaml` of the copy with these four values:

   - the new identifier;
   - the same `profile_id`;
   - the label `Workspace 1`;
   - a `last_active_at` of some weeks before today.

   Result: The copy has the new identifier and its own time.

6. Write `runtime.yaml` of the copy with one workspace and one panel of the
   kind `moonbase`, which the candidate does not have:

   ```yaml
   version: 2
   workspaces:
   - local_id: 76c88150-e2c3-4cca-b116-38a66df2067a
     name: Workspace 1
     panels:
     - local_id: 282fc53e-0ed2-459d-bb84-c859be647981
       name: Moonbase CLI
       kind: moonbase
   ```

   Result: The candidate cannot read the board of the copy.

7. Add the new identifier under `recent_session_ids` of that `profile_id` in
   `<state>/data/home/.horizon/sessions/index.yaml`.

   Result: The index holds two sessions with the same `profile_id`.

8. Start the fixture again with the same state path and the same three
   arguments.

   Result: The fixture prints a manifest with a new `vnc_address`.

9. Open that `vnc_address` in a Device panel.

   Result: The Device panel shows the candidate window.

## 6. Tasks

### 6.1 OPEN — The dialog opens above its backdrop

1. Open **Menu** › **Sessions**.

   Result: The dialog shows two session cards. The dialog is bright and the
   board around it is dim.

2. Look at the heading **Sessions** and at the text of the two session cards.

   Result: That text has the same contrast as the text of a panel.

### 6.2 AGAIN — The dialog opens a second time

1. Click the board outside the dialog.

   Result: The dialog closes.

2. Wait one second.

   Result: The dialog stays closed.

3. Open **Menu** › **Sessions** again.

   Result: The dialog is bright, as in task 6.1.

4. Click the heading of the dialog.

   Result: The dialog stays open.

5. Click the session card **Workspace 1**.

   Result: The dialog stays open. That session card becomes the selected card.

### 6.3 REMOVE — The dialog removes an unreadable session

> **CAUTION:** REMOVE ONLY THE SESSION THAT SETUP MADE. The dialog deletes the
> session folder from the disk. If you remove another session, that board is
> gone.

1. On the session card **Workspace 1**, click **Delete**.

   Result: The card asks `Horizon cannot read this board. Delete the session and
   its board?` in red, with **Yes** and **No**.

2. Click **No**.

   Result: The card shows **Delete** again. The list does not change.

3. Click **Delete**.

   Result: The card asks the same question again.

4. Click **Yes**.

   Result: The card leaves the list. The dialog shows no error text.

5. Close the candidate window with the close button of the window manager.

   Result: The candidate stops.

6. Look in `<state>/data/home/.horizon/sessions`.

   Result: The session folder of the removed session is not there. The session
   folder of the current session is there.

### 6.4 CLOSE — The buttons of the dialog work

1. Start the fixture again.

   Result: The fixture prints a manifest with a `vnc_address`.

2. Open that `vnc_address` in a Device panel.

   Result: The Device panel shows the candidate window.

3. Open **Menu** › **Sessions**.

   Result: The dialog shows the current session.

4. Click **Close**.

   Result: The dialog closes.

5. Open **Menu** › **Sessions** again.

   Result: The dialog opens.

6. Press **Escape**.

   Result: The dialog closes.

## 7. Pass criteria

- In tasks 6.1 and 6.2, the dialog is bright above a dim board each time that
  it opens.
- Each click inside the dialog goes to the control under the pointer.
- In task 6.3, the question names the board that the candidate cannot read.
- In task 6.3, the dialog removes the session and its session folder, and it
  shows no error text.
- In task 6.4, **Close** and **Escape** close the dialog.

## 8. Cleanup

1. Close the candidate window with the close button of the window manager.

   Result: The candidate and the fixture stop.

2. Close the task-owned Device panel.

   Result: The Device panel releases its connection.

> **CAUTION:** REMOVE ONLY THE STATE PATH AND THE FROZEN CANDIDATE OF THIS
> TASK. The state path holds the private home and the saved sessions of this
> run. If you remove another path, another run loses its state.

3. Remove the state path of this task.

   Result: The state path is gone.

4. Remove the frozen candidate of this run.

   Result: The frozen candidate is gone.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
