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
- This procedure does not test: the migration of a home that no release made, the
  secrets of the credential store of the operating system, and the panels
  themselves. Use the other procedures for those.

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

3. Read the store of the home.

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

## 8. Cleanup

1. Close the Device panel of this run through the `device_panel` tool.

   Result: the viewer is gone, and the fixture keeps its state.

2. Stop the fixture with the normal window close of the candidate, or with
   **Ctrl+C** in the terminal of the fixture.

   Result: the display, the window manager and the VNC server stop.

3. Remove the state folder of the run.

   Result: no file of the run stays in the evidence folder.

## 9. Record of results

Put the results in the pull request with the hashes of the two binaries and of
the settings file. Keep the log, the screenshots and the recording in the private
evidence folder of the task. If the run must be kept, write a report in
`docs/testing/reports/` with [the report template](../reports/TEMPLATE.md).
