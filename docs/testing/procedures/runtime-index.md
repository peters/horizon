---
procedure: runtime-index
feature: Runtime index of a saved session in SQLite
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Runtime index test procedure

## 1. Purpose

This procedure makes sure that a restart keeps the board of a saved session.
It also makes sure that the candidate loads `runtime.yaml` when the runtime
index is damaged, missing or older than `runtime.yaml`. It also makes sure
that an ephemeral session writes no session file.

## 2. Applicability

- Candidate: a build that writes `runtime.sqlite` next to `runtime.yaml`.
- Platforms: Linux. On macOS, do the same tasks in an isolated desktop. Stop
  and start the candidate by hand at each restart.
- This procedure does not test the park state of a workspace. The unit tests
  cover it.

## 3. Safety

> **CAUTION:** DO NOT DO THESE TASKS IN YOUR OWN HOME. The tasks damage and
> remove session files. Use only the private home of the fixture.

> **CAUTION:** STOP ONLY THE CANDIDATE OF THIS RUN. Other Horizon processes
> can hold your work.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256. Build it with `cargo build --release`
  and copy the binary to `<run>/bin/horizon`.
- Python 3 with the `sqlite3` module.
- `xdotool` on the host.

These names apply in the steps:

| Name | Value |
|---|---|
| `<run>` | A new task-owned directory for this run. |
| `<state>` | The state directory of the fixture, `<run>/fixture`. |
| `<home>` | The private home of the fixture, `<state>/data/home`. |
| `<session>` | The session directory, `<home>/.horizon/sessions/<id>`. |
| `<index>` | The runtime index, `<session>/runtime.sqlite`. |
| `<new-state>` | The state directory of the baseline fixture in R06, `<run>/ephemeral`. This directory must not exist before R06. |

Use this command to show the board that the index holds. The output gives the
number of workspaces, the number of panels and the schema version.

```sh
python3 -c 'import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); print(*(c.execute(q).fetchone()[0] for q in ("select count(*) from workspaces","select count(*) from panels","pragma user_version")))' <index>
```

## 5. Setup

1. Start the fixture with a new state directory and a saved session.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <run>/bin/horizon --native-view --saved-session --reuse-state --state <state>
   ```

   Result: The output shows a `vnc_address`. `<state>/lab.json` shows `"session":
   "saved"`, and the manifest names the SHA-256 of the frozen candidate.

2. Show the fixture in a Device panel. Do the
   [automatic viewer health check](../../../scripts/device-smoke/README.md#automatic-viewer-health-check)
   and keep its observations.

   Result: A Device panel shows a live view with frames that advance.

3. Make six editor panels. Hold Ctrl and double-click an empty part of the
   board six times, and select **Editor** each time.

   Result: The board has one workspace with six editor panels.

4. Arrange the six panels in two columns and three rows, and note the exact
   title of each panel.

   Result: Each panel has a title that this procedure uses in a query.

This procedure restarts the candidate several times. To restart it, close its
window through the window manager and run the command of step 1 again with the
same `<state>`. The fixture keeps the private home of that state path.

## 6. Tasks

### 6.1 R01: The index holds the board

1. Find the session directory.

   ```sh
   ls -d <home>/.horizon/sessions/*/
   ```

   Result: One directory shows. It contains `runtime.yaml` and `runtime.sqlite`.

2. Show the board that the index holds.

   Result: The output is `1 6 1`.

3. Move the first editor panel to a new position with its title bar. Write its
   exact title as `<panel>`.

   Result: The panel shows at the new position.

4. Wait 2 seconds. Show its position in the index.

   ```sh
   python3 -c 'import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute("select data from panels where name=?",(sys.argv[2],)).fetchone()[0])' <index> <panel>
   ```

   Result: The `position` value is the new position.

### 6.2 R02: A restart keeps the board

1. Record a screenshot of the board.

   Result: The screenshot shows the panel of R01 at its new position.

2. Close the window of the candidate through the window manager.

   Result: The candidate stops, and the fixture stops.

3. Start the candidate again with the command of setup step 1 and the same
   `<state>`.

   Result: The manifest names the frozen candidate and its SHA-256.

4. Compare the board with the screenshot of step 1.

   Result: The workspace and the six panels are the same. The panel of R01
   is at its new position.

### 6.3 R03: A damaged index falls back to runtime.yaml

1. Close the window of the candidate through the window manager.

   Result: The candidate stops, and the fixture stops.

2. Replace the index with text. Remove the journal files of the index.

   ```sh
   printf 'synthetic damage' > <index>
   rm -f <index>-wal <index>-shm
   ```

   Result: The index is not a database.

   > **NOTE:** Horizon can stop with the recent changes of the index in
   > `<index>-wal`. SQLite then reads the board from that file and does not
   > see the damage. Thus this step removes the journal files too.

3. Start the candidate again with the command of setup step 1 and the same
   `<state>`.

   Result: The candidate shows a board.

4. Compare the board with the board of R02.

   Result: The board is the same. `<state>/horizon.log` contains `loading runtime.yaml of session`.

6. Move a second editor panel with its title bar. Wait 2 seconds.

   Result: The panel shows at the new position.

7. List the files of the runtime index.

   ```sh
   ls <session> | grep runtime.sqlite
   ```

   Result: The list shows `runtime.sqlite` and `runtime.sqlite.damaged-<time>`.
   The damaged file contains `synthetic damage`.

8. Show the board that the index holds.

   Result: The output is `1 6 1`.

### 6.4 R04: A missing index is imported one time

1. Close the window through the window manager.

   Result: The candidate stops, and the fixture stops.

2. Remove the files of the runtime index. Keep the damaged copy.

   ```sh
   rm -f <session>/runtime.sqlite <session>/runtime.sqlite-wal <session>/runtime.sqlite-shm
   ```

   Result: Only `runtime.yaml`, `meta.yaml`, the `transcripts` directory and the
   damaged copy stay.

3. Record the SHA-256 of `runtime.yaml`. Start the candidate again with the
   command of setup step 1 and the same `<state>`.

   Result: The candidate shows a board.

4. Compare the board with the board of R03.

   Result: The board is the same. The SHA-256 of `runtime.yaml` did not change.

5. Move a third editor panel with its title bar. Wait 2 seconds.

   Result: `runtime.sqlite` exists again.

6. Show the board that the index holds.

   Result: The output is `1 6 1`.

### 6.5 R05: runtime.yaml from an earlier release wins

1. Close the window through the window manager.

   Result: The candidate stops, and the fixture stops.

2. Change the name of the workspace in `runtime.yaml`, as an earlier release does.

   ```sh
   sed -i 's/^  name: Disposable VNC debug$/  name: Changed by an earlier release/' <session>/runtime.yaml
   ```

   Result: `runtime.yaml` has the new name. The index has the old name.

3. Start the candidate again with the command of setup step 1 and the same
   `<state>`.

4. Look at the workspace name on the board.

   Result: The board shows `Changed by an earlier release`.

### 6.6 R06: An ephemeral session writes no session file

1. Start the baseline fixture `scripts/device-smoke/serve.py` with the state
   directory `<new-state>`.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <run>/bin/horizon --native-view --state <new-state>
   ```

   Result: The candidate starts with `--ephemeral`. The output shows a `vnc_address`.

2. Show the fixture in a Device panel. Move a panel with its title bar.

   Result: The panel shows at the new position.

3. Wait 2 seconds. List the Horizon directory of the private home.

   ```sh
   ls <new-state>/data/home/.horizon
   ```

   Result: There is no `sessions` directory.

## 7. Pass criteria

- R01 to R06 give the results in their steps.
- No restart shows an empty board or a board with fewer panels.
- `<state>/horizon.log` shows no panic.

## 8. Cleanup

1. Close the window of the candidate.

   Result: The candidate stops and the fixture stops.

2. Stop the baseline fixture of R06 with Ctrl-C.

   Result: The fixture stops its child processes.

3. Close the Device panels of this run with `device_panel` operation `close`.

   Result: No Device panel of this run stays open.

4. Remove `<run>`.

   Result: No file of the run stays on the computer.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
