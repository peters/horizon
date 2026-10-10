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
- Platforms: Linux. On macOS and Windows, do the same tasks in an isolated
  desktop. Stop and start the candidate by hand at each restart.
- This procedure does not test these functions:
  - The park state and the status line of a cloud. The unit tests and the
    [cloud park procedure](cloud-park-attach.md) cover them.
  - The cloud list. That list is a later milestone.

## 3. Safety

> **CAUTION:** DO NOT DO THESE TASKS IN YOUR OWN HOME. The tasks damage and
> remove session files. Use only the private home of the fixture.

> **CAUTION:** STOP ONLY THE CANDIDATE OF THIS RUN. Other Horizon processes
> can hold your work.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256. Use
  [S01](cloud-panels/s-test-fixture.md#61-s01--build-and-freeze-the-candidate).
- Python 3 with the `sqlite3` module.
- `xdotool` on the host.

These names apply in the steps:

| Name | Value |
|---|---|
| `<run>` | A new task-owned directory for this run. |
| `<state>` | The state directory of the persistent launcher, `<run>/fixture`. |
| `<home>` | The private home of the fixture, `<state>/data/home`. |
| `<session>` | The session directory, `<home>/.horizon/sessions/<id>`. |
| `<index>` | The runtime index, `<session>/runtime.sqlite`. |

Use this command to show the board that the index holds. The output gives the
number of workspaces, the number of panels and the schema version.

```sh
python3 -c 'import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); print(*(c.execute(q).fetchone()[0] for q in ("select count(*) from workspaces","select count(*) from panels","pragma user_version")))' <index>
```

## 5. Setup

1. Make the persistent launcher with S02 steps 1, 2 and 5 of the
   [test fixture area](cloud-panels/s-test-fixture.md#62-s02--start-the-persistent-launcher).

   Result: `<run>/launcher` has a copy of `serve.py` without `--ephemeral`, with
   the restart branch.

2. In the restart branch of the copy, add these two lines before `marker.unlink()`.

   ```python
                   while (args.state / 'restart-hold').exists():
                       time.sleep(0.2)
   ```

   Result: While the restart hold exists, the launcher does not start the candidate again.

3. In the copy, replace the `terminals` list of the fixture workspace with six editor panels.

   ```python
                   'terminals': [
                       {'name': f'Editor {n}', 'kind': 'editor',
                        'position': [40 + 560 * (n % 2), 60 + 300 * (n // 2)], 'size': [520, 260]}
                       for n in range(6)]}]}
   ```

   Result: The board of the fixture has one workspace with six editor panels.

4. Start the persistent launcher with a new state directory.

   ```sh
   python3 <run>/launcher/serve.py --horizon <run>/bin/horizon --native-view --state <state>
   ```

   Result: The output shows a `vnc_address`.

5. Do [S03](cloud-panels/s-test-fixture.md#63-s03--show-the-fixture-in-a-device-panel)
   and [S04](cloud-panels/s-test-fixture.md#64-s04--make-sure-that-the-frozen-candidate-runs).

   Result: A Device panel shows a live view. The `horizon` child is the frozen candidate.

## 6. Tasks

### 6.1 R01: The index holds the board

1. Find the session directory.

   ```sh
   ls -d <home>/.horizon/sessions/*/
   ```

   Result: One directory shows. It contains `runtime.yaml` and `runtime.sqlite`.

2. Show the board that the index holds.

   Result: The output is `1 6 1`.

3. Move the panel Editor 0 to a new position with its title bar.

   Result: The panel shows at the new position.

4. Wait 2 seconds. Show the position of Editor 0 in the index.

   ```sh
   python3 -c 'import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute("select data from panels where name=?",("Editor 0",)).fetchone()[0])' <index>
   ```

   Result: The `position` value is the new position.

### 6.2 R02: A restart keeps the board

1. Record a screenshot of the board.

   Result: The screenshot shows Editor 0 at its new position.

2. Make the restart marker.

   ```sh
   touch <state>/restart-request
   ```

   Result: The launcher starts the candidate again after the next close.

3. Close the window of the candidate through the window manager.

   Result: The candidate stops. The launcher starts it again.

4. Do S04 again.

   Result: The new `horizon` child is the frozen candidate.

5. Compare the board with the screenshot of step 1.

   Result: The workspace and the six panels are the same. Editor 0 is at its new position.

### 6.3 R03: A damaged index falls back to runtime.yaml

1. Make the restart hold and the restart marker.

   ```sh
   touch <state>/restart-hold <state>/restart-request
   ```

   Result: The launcher waits after the next close.

2. Close the window of the candidate through the window manager.

   Result: The candidate stops. The launcher does not start it.

3. Replace the index with text. Remove the journal files of the index.

   ```sh
   printf 'synthetic damage' > <index>
   rm -f <index>-wal <index>-shm
   ```

   Result: The index is not a database.

   > **NOTE:** Horizon can stop with the recent changes of the index in
   > `<index>-wal`. SQLite then reads the board from that file and does not
   > see the damage. Thus this step removes the journal files too.

4. Remove the restart hold.

   ```sh
   rm <state>/restart-hold
   ```

   Result: The launcher starts the candidate.

5. Compare the board with the screenshot of R02.

   Result: The board is the same. `<state>/horizon.log` contains `loading runtime.yaml of session`.

6. Move the panel Editor 1 with its title bar. Wait 2 seconds.

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

1. Make the restart hold and the restart marker. Close the window through the window manager.

   Result: The candidate stops. The launcher does not start it.

2. Remove the files of the runtime index. Keep the damaged copy.

   ```sh
   rm -f <session>/runtime.sqlite <session>/runtime.sqlite-wal <session>/runtime.sqlite-shm
   ```

   Result: Only `runtime.yaml`, `meta.yaml`, the `transcripts` directory and the
   damaged copy stay.

3. Record the SHA-256 of `runtime.yaml`. Remove the restart hold.

   Result: The launcher starts the candidate.

4. Compare the board with the board of R03.

   Result: The board is the same. The SHA-256 of `runtime.yaml` did not change.

5. Move the panel Editor 2 with its title bar. Wait 2 seconds.

   Result: `runtime.sqlite` exists again.

6. Show the board that the index holds.

   Result: The output is `1 6 1`.

### 6.5 R05: runtime.yaml from an earlier release wins

1. Make the restart hold and the restart marker. Close the window through the window manager.

   Result: The candidate stops. The launcher does not start it.

2. Change the name of the workspace in `runtime.yaml`, as an earlier release does.

   ```sh
   sed -i 's/^  name: Disposable VNC debug$/  name: Changed by an earlier release/' <session>/runtime.yaml
   ```

   Result: `runtime.yaml` has the new name. The index has the old name.

3. Remove the restart hold.

   Result: The launcher starts the candidate.

4. Look at the workspace name on the board.

   Result: The board shows `Changed by an earlier release`.

### 6.6 R06: An ephemeral session writes no session file

1. Start the baseline fixture `scripts/device-smoke/serve.py` with a new state directory.

   Result: The candidate starts with `--ephemeral`.

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

1. Remove the restart marker and the restart hold. Close the window of the candidate.

   Result: The candidate stops and the launcher stops.

2. Stop the baseline fixture with Ctrl-C.

   Result: The fixture stops its child processes.

3. Close the Device panels of this run with `device_panel` operation `close`.

   Result: No Device panel of this run stays open.

4. Remove `<run>`.

   Result: No file of the run stays on the computer.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
