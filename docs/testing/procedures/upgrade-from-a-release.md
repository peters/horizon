---
procedure: upgrade-from-a-release
feature: Upgrade from an older Horizon
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Upgrade from a release test procedure

## 1. Purpose

This procedure proves that a person who upgrades from the last release keeps
every setting and every session. It also proves that the panels and the features
that this build removed leave a notice and no error.

## 2. Applicability

- Candidate: any build of Horizon that reads the settings of an older build. This
  procedure uses the release binary of v0.2.7 and the candidate.
- Platforms: Linux. The isolated fixture of
  [`scripts/device-smoke`](../../../scripts/device-smoke/README.md) runs on Linux.
- This procedure does not test: the migration of a home that no release made, and
  the panels themselves. Use the other procedures for those.
- Step 6.8 tests the keyring import with a stand-in program that is first on the
  `PATH` of the fixture. This procedure never opens the keyring of a person.

## 3. Safety

> **CAUTION:** NEVER USE THE REAL HOME OF A PERSON IN THIS PROCEDURE. THE FIXTURE
> WRITES A SETTINGS FILE, A SESSIONS FOLDER AND A SECRETS FILE. ENTER ONLY THE
> PRIVATE HOME OF A FIXTURE STATE FOLDER, AND KEEP THAT STATE IN THE EVIDENCE
> FOLDER OF THE TASK.

> **CAUTION:** DO NOT RUN THE RELEASE BINARY ON A DISPLAY OF THE PERSON. START IT
> ONLY IN THE ISOLATED DESKTOP OF THE FIXTURE, VIEWED THROUGH A NATIVE DEVICE
> PANEL. THE RELEASE BINARY STARTS A SHELL AND WRITES A HOME.

## 4. Equipment and preconditions

- The release binary of v0.2.7. Keep a copy and record its SHA-256.
- The candidate binary. Freeze it and record its SHA-256.
- `scripts/device-smoke/serve.py` from the candidate checkout, with `Xvfb`,
  `openbox`, `x11vnc` and `bwrap` on the host.
- One new state folder for this run. Do not use a folder of another run.
- A Horizon host that can show a native Device panel in the workspace of the
  person who does the procedure.

## 5. Setup

1. Make the state folder of the run.

   ```sh
   state=/tmp/hz-upgrade-<task>
   mkdir -m 700 "$state"
   ```

   Result: the folder exists and no file is in it.

2. Start the fixture with the release binary and a plain start.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <release>/horizon \
     --native-view --plain-start --reuse-state --state "$state"
   ```

   Result: the command prints the manifest, and the private home is
   `$state/data/home/.horizon`.

3. Show the isolated desktop in the current workspace.

   ```json
   {"operation":"list"}
   {"operation":"create","endpoint":"127.0.0.1:<vnc port of the manifest>"}
   ```

   Result: the panel connects, an image arrives, and the frame sequence advances.

## 6. Tasks

### 6.1 U01 — Make the settings of the release

1. In the window of the release, press **Ctrl+Shift+,** and set the theme to
   **Dark**.

   Result: the window is dark.

2. Close the settings window and place the shell panel of the first workspace at
   another position on the canvas.

   Result: the panel moves, and the board of the release keeps the new position.

3. Quit the release with the normal window close.

   Result: the fixture stops the display, and the state folder keeps its data.

### 6.2 U02 — Check the home of the release

1. Read the settings file of the release.

   ```sh
   grep -E 'version:|theme:' "$state/data/home/.horizon/config.yaml"
   ```

   Result: the file names `version: 8` and `theme: dark`.

2. Record the SHA-256 of the settings file and of the session.

   ```sh
   sha256sum "$state/data/home/.horizon/config.yaml" \
     "$state/data/home/.horizon/sessions/"*/runtime.yaml
   ```

   Result: the command prints three or more hashes. Keep them for step 6.5.

### 6.3 U03 — Start the candidate on the same home

1. Start the fixture again with the candidate and the same state folder.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <candidate>/horizon \
     --native-view --plain-start --reuse-state --state "$state"
   ```

   Result: the window of the candidate comes up in the dark theme.

2. Show the new run in the same Device panel.

   Result: the panel shows the candidate, and the frame sequence advances.

3. Read the log of the run.

   ```sh
   grep -E 'importing|no settings found|cannot read' "$state"/horizon*.log
   ```

   Result: one `importing` line names
   `$state/data/home/.horizon/config.yaml`, and there is no `cannot read` line.

### 6.4 U04 — Check the settings and the session

1. Open **Settings**.

   Result: the theme is **Dark**, and the shortcuts and the presets are the ones
   that the release had.

2. Close the settings window and read the board.

   Result: the session of the release opens with its panels at their places. A
   panel of a kind that this build removed is not there, and one line in the
   sidebar says how many panels were left out and of which kind.

3. Read the line of the panels that were left out again after ten seconds.

   Result: the line is still in the sidebar. It stays until a person closes it.

4. Press the close control of that line, and press **Space** on it in a second run.

   Result: the line goes, and it does not come back until a session with such a
   panel opens again.

5. Read the store of the home.

   ```sh
   python3 - "$state/data/home/.horizon/settings.sqlite" <<'PY'
   import sqlite3, sys
   rows = sqlite3.connect(sys.argv[1]).execute(
       "SELECT section, data FROM settings ORDER BY section").fetchall()
   print([name for name, _ in rows])
   print(dict(rows)["appearance"])
   PY
   ```

   Result: the store holds the sections of this build, and the `appearance`
   section holds `{"theme":"dark"}`.

### 6.5 U05 — Check that the release home is unchanged

1. Compare the hashes of the settings file and of the session with the hashes of
   step 6.2.

   ```sh
   sha256sum "$state/data/home/.horizon/config.yaml"
   ```

   Result: the hash is the same. The settings file keeps every byte.

2. Read the copy of the board of the release.

   ```sh
   grep -c 'kind: usage\|kind: git_changes' \
     "$state/data/home/.horizon/sessions/"*/runtime.yaml.before-3
   ```

   Result: the copy `runtime.yaml.before-3` holds the board of the release,
   including one line for each panel of a removed kind.

3. Quit the candidate with the normal window close, and start it again with
   `--plain-start --reuse-state` as in step 6.3.

   Result: the window comes up with the same settings and the same board, and the
   log names the store instead of the YAML file.

### 6.6 U06 — Check the secrets sentence

1. Open **Settings → Secrets** and add a name that this computer does not hold.

   ```text
   HETZNER_TOKEN
   ```

   Result: the row says `no value on this computer`, and the tab says that a
   value of an older Horizon must be typed once more.

2. Add the value of the name and save.

   Result: the row says `Stored on this computer`.

### 6.7 U07 — Find the account of a reference

1. Read the endpoint origin and the slot of a reference of the settings.

   ```sh
   python3 - "$state/data/home/.horizon/settings.sqlite" <<'PY'
   import json, sqlite3, sys
   rows = dict(sqlite3.connect(sys.argv[1]).execute(
       "SELECT section, data FROM settings").fetchall())
   print(json.dumps(json.loads(rows["browser"])["remote"], indent=2))
   PY
   ```

   Result: the output names the `endpoint` of the provider and the `slot` of each
   credential binding. An older version put the value of a reference into the
   keyring under its origin, one `|`, and its slot.

2. Write that account down, and the reference of the same binding.

   Result: the account is, for example,
   `https://grid.example.net|remote-browser/grid/key`, and the reference is `key`.

### 6.8 U08 — Check the import of the keyring with a stand-in

> **CAUTION:** THIS STEP NEVER OPENS THE REAL KEYRING OF A PERSON. IT RUNS A
> STAND-IN PROGRAM THAT PRINTS A SYNTHETIC VALUE, AND IT KEEPS THAT PROGRAM FIRST
> ON THE `PATH` OF THE FIXTURE ONLY. DO NOT RUN `secret-tool`, `security`,
> `gdbus`, `busctl` OR `gnome-keyring-daemon` BY HAND FOR THIS STEP, AND DO NOT
> PRESS THE BUTTON OF A RUN THAT READS THE REAL KEYRING.

1. Make a stand-in `secret-tool` inside the state folder, which the sandbox
   shows to the candidate.

   ```sh
   standin="$state/standin"
   mkdir -p "$standin"
   cat >"$standin/secret-tool" <<'SH'
   #!/bin/sh
   printf '%s\n' '[/org/freedesktop/secrets/collection/login/7]'
   printf '%s\n' 'label = keyring@horizon-remote-browser'
   printf '%s\n' 'secret = synthetic-token'
   printf '%s\n' 'created = '
   printf '%s\n' 'modified = '
   printf '%s\n' 'attribute.service = horizon-remote-browser' >&2
   printf '%s\n' 'attribute.username = https://grid.example.net|remote-browser/grid/key' >&2
   SH
   chmod 755 "$standin/secret-tool"
   ```

   Use the account of step 6.7 in the `attribute.username` line, and read the
   format of the stand-in from
   [the keyring import](../../architecture/settings-store.md#the-values-that-an-older-build-kept-in-the-system-keyring).

   Result: `$state/standin/secret-tool` is one executable file.

2. Start the candidate again with that folder first on its `PATH`.

   ```sh
   PATH="$standin:$PATH" python3 scripts/device-smoke/serve.py \
     --horizon <candidate>/horizon \
     --native-view --plain-start --reuse-state --state "$state"
   ```

   Result: the window comes up with the settings and the board of step 6.5. The
   candidate still finds every other program of the host, because only one folder
   comes in front of `PATH`.

3. Open **Settings → Remote**, and read the rows of the references.

   Result: the row of the reference of step 6.7 says `missing`, and one line
   above the providers says **A value that an older version stored in the system
   keyring can be imported.**

4. Press **Import from the keyring**.

   Result: the tab says `Imported 1 value.`, and the row of the reference says
   `present`.

5. Read the row that the import wrote.

   ```sh
   python3 - "$state/data/home/.horizon/secrets.sqlite" <<'PY'
   import sqlite3, sys
   print(dict(sqlite3.connect(sys.argv[1]).execute("SELECT id, body FROM secrets").fetchall()))
   PY
   ```

   Result: the output holds `credential/<reference>` with `synthetic-token` as
   its body.

6. Type another value into the row of that reference, press **Save to the secrets
   file**, and read the file again.

   Result: the row holds the value that the person typed.

7. Remove `$standin/secret-tool`, start the candidate again with the same `PATH`,
   and press the button of a row that says `missing`.

   Result: the tab says **Install the package `libsecret-tools` ...**, it writes
   no value, and the row of step 6 still holds the value that the person typed.

8. Close the run, and remove the stand-in folder.

   Result: no file of the stand-in stays in the state folder.

## 7. Pass criteria

- The candidate starts with the theme, the shortcuts, the presets and the
  workspaces of the release.
- The session of the release opens with every panel of a kind that this build
  still has, and the sidebar says which removed kinds left it.
- The settings file of the release keeps every byte, and the session keeps a copy
  of the board of the release.
- The log names the imported YAML file, and it names no error.
- The second start reads the store, and it reads no YAML file.
- The Settings panel keeps one width on every tab, and the Secrets tab says that
  an old value is typed once more.
- The line about the panels that were left out stays after ten seconds, and it
  goes on its close control.
- The import of the keyring writes one row from a stand-in program, writes no row
  over a value that is there, and names the package of the program when that
  program is missing.

## 8. Cleanup

1. Close the Device panel of this run through the `device_panel` tool.

   Result: the viewer is gone, and the fixture keeps its state.

2. Stop the fixture with the normal window close of the candidate, or with
   **Ctrl+C** in the terminal of the fixture.

   Result: the display, the window manager and the VNC server stop.

3. Remove the state folder of the run, which holds the stand-in folder of step 6.8.

   Result: no file of the run stays in the evidence folder.

## 9. Record of results

Put the results in the pull request with the hashes of the two binaries and of
the settings file. Keep the log, the screenshots and the recording in the private
evidence folder of the task. If the run must be kept, write a report in
`docs/testing/reports/` with [the report template](../reports/TEMPLATE.md).
