---
procedure: settings-store
feature: settings in SQLite, one JSON settings file, no YAML
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Settings store test procedure

## 1. Purpose

This procedure proves that Horizon keeps its settings in one SQLite file, that it
reads a settings file only when it has no store, and that it leaves that file
exactly as it is. It also proves that a file of an older Horizon, which is YAML,
is not read, and that a damaged store does not replace the settings of a person
with defaults.

## 2. Applicability

- Candidate: a build that keeps its settings in `settings.sqlite`, reads
  `config.json`, and has no YAML settings file.
- Platforms: Linux.
- Each task runs on a copy of a real Horizon home, so the run shows the settings
  that a person has today.
- This procedure does not test these functions:
  - The changes in the Settings window that a person makes at a normal start.
    Use [the Horizon home procedure](horizon-home.md) for the window.
  - The migration of an old configuration version. The unit tests of
    `config_migration` examine each version.

## 3. Safety

> **CAUTION:** USE A TEST HOME DIRECTORY IN EACH TASK. A step damages a
> settings store. Do not use your own home directory.

## 4. Equipment and preconditions

- A frozen candidate `<bin>/horizon` and its SHA-256. Use the
  [device smoke fixture](../../../scripts/device-smoke/README.md) notes.
- A running Device panel for the fixture display. The person watches the
  window there.

These names apply in the steps:

| Name | Value |
|---|---|
| `<run>` | A new task-owned directory for this run. |
| `<home>` | The test home directory, `<run>/home`. |
| `<state>` | The state directory of the fixture, `<run>/fixture`. |
| `<display>` | The `display` value in the output of the fixture. |

## 5. Setup

1. Make the run directory and the test home.

   ```sh
   mkdir -p <run>/home/.horizon && chmod 700 <run>
   ```

   Result: `<run>/home/.horizon` exists.

2. Copy the sessions of the real Horizon home of this computer into the test
   home, and write a settings file there. This is the instance that the
   procedure imports.

   ```sh
   cp -a "$HOME/.horizon/sessions" <home>/.horizon/
   cat > <home>/.horizon/config.json <<'JSON'
   {
     "appearance": {"theme": "light"},
     "features": {"sidebar_accordion": true}
   }
   JSON
   sha256sum <home>/.horizon/config.json > <run>/copy.sha256
   ```

   Result: The test home holds the real sessions and a settings file with the
   light theme and the accordion sidebar.

3. Start the fixture with the private Horizon home in `<home>` and note the
   display.

   ```sh
   HOME=<home> python3 scripts/device-smoke/serve.py --horizon <bin>/horizon \
     --native-view --state <state>
   ```

   Result: The output shows `vnc_address` and `display`. The Device panel shows
   the Horizon window with the same board as your own window: the same
   workspaces, the same panels and the same settings.

## 6. Tasks

### 6.1 S01 — The first start reads the settings file one time

1. Look at the files of the Horizon home.

   ```sh
   ls -A <home>/.horizon
   ```

   Result: The output has `settings.sqlite`, `config.json`, and `sessions`. The
   settings file stays where you wrote it, because a start does not move it.

2. Compare the settings file with the copy that you made.

   ```sh
   sha256sum -c <run>/copy.sha256
   ```

   Result: The check passes. The import did not change one byte of the file.

3. Read the sections of the store.

   ```sh
   sqlite3 <home>/.horizon/settings.sqlite 'SELECT section FROM settings ORDER BY section;'
   ```

   Result: The output names `appearance`, `browser`, `features`, `overlays`,
   `presets`, `remote_hosts`, `shortcuts`, `version`, `window`, and
   `workspaces`.

### 6.2 S02 — A settings file next to the store is not a setting

1. Stop the candidate through its window.

   Result: The process ends. The fixture stays.

2. Write a settings file with the dark theme next to the store.

   ```sh
   python3 -c 'import json, sys
path, theme = sys.argv[1], sys.argv[2]
document = json.load(open(path))
document["appearance"]["theme"] = theme
json.dump(document, open(path, "w"), indent=2)' <home>/.horizon/config.json dark
   sha256sum <home>/.horizon/config.json > <run>/file.sha256
   ```

   Result: The home holds a store and a settings file with the dark theme.

3. Start the candidate again with the same home, and read the log.

   ```sh
   env DISPLAY=<display> HOME=<home> <bin>/horizon --ephemeral > <run>/s02.log 2>&1 &
   rg 'is not read' <run>/s02.log
   ```

   Result: The window uses the theme of the store. The log names the store and
   the settings file.

4. Prove that Horizon did not change the file.

   ```sh
   sha256sum -c <run>/file.sha256
   ```

   Result: The check passes. The file stays as you wrote it.

### 6.3 S03 — A YAML file of an older Horizon is not read

1. Stop the candidate through its window.

   Result: The process ends.

2. Write an old YAML file in a home with no store, and start the candidate
   without `--config`.

   ```sh
   mkdir -p <run>/legacy/.horizon
   printf 'version: 11\nappearance:\n  theme: dark\n' > <run>/legacy/.horizon/config.yaml
   sha256sum <run>/legacy/.horizon/config.yaml > <run>/legacy.sha256
   env DISPLAY=<display> HOME=<run>/legacy <bin>/horizon --ephemeral > <run>/s03.log 2>&1 &
   ```

   Result: The window uses the default theme (`auto`), because the file is YAML
   and this build reads JSON. The log says that no settings were found.

3. Prove that the start wrote no file and changed no byte.

   ```sh
   sha256sum -c <run>/legacy.sha256
   ls -A <run>/legacy/.horizon
   ```

   Result: The check passes. The home holds only `config.yaml`: a start with the
   default settings writes no store.

4. Ask the candidate to read the YAML file as its settings.

   ```sh
   env DISPLAY=<display> HOME=<run>/legacy <bin>/horizon \
     --config <run>/legacy/.horizon/config.yaml --ephemeral > <run>/s03b.log 2>&1; echo $?
   ```

   Result: The command reports that the settings file format is JSON now, and the
   status is not zero. The file keeps its bytes.

### 6.4 S04 — A damaged store comes back from the settings file

> **CAUTION:** THIS STEP DAMAGES THE TEST STORE. Use only the store under
> `<home>`.

1. Stop the candidate through its window.

   Result: The process ends.

2. Damage the store and start the candidate again.

   ```sh
   printf 'not a database' > <home>/.horizon/settings.sqlite
   env DISPLAY=<display> HOME=<home> <bin>/horizon --ephemeral > <run>/s04.log 2>&1
   ```

   Result: The window opens with the settings of the settings file. The output
   names the store and the path of the damaged file. The file
   `<home>/.horizon/settings.sqlite.damaged-*` exists and holds the words
   `not a database`.

3. Damage a store that has no settings file, in a new home.

   ```sh
   mkdir -p <run>/bare && printf 'not a database' > <run>/bare/settings.sqlite
   env DISPLAY=<display> HOME=<home> <bin>/horizon --config <run>/bare/config.json \
     --ephemeral > <run>/s04b.log 2>&1; echo $?
   ```

   Result: Horizon reports the path and the reason, and the command status is
   not zero. It does not start with the default settings, and
   `<run>/bare/settings.sqlite` stays where it is.

### 6.5 S05 — Every saved session still opens

1. Open each workspace in the window, and one panel in each workspace.

   Result: Each panel shows its terminal and its name, as in your own window.
   A panel that cannot start again says why, in the same words as before.

2. Compare the count of the sessions with the count in your own home.

   ```sh
   ls <home>/.horizon/sessions | wc -l; ls "$HOME/.horizon/sessions" | wc -l
   ```

   Result: The two counts are the same.

### 6.6 S06 — A migration writes the store and keeps the old text

1. Stop the candidate through its window.

   Result: The process ends.

2. Write an older configuration version in a new home, and start the candidate
   there.

   ```sh
   mkdir -p <run>/old && printf '{"shortcuts": {"command_palette": "Alt+K"}}' \
     > <run>/old/config.json
   env DISPLAY=<display> HOME=<home> <bin>/horizon --config <run>/old/config.json \
     --ephemeral > <run>/s06.log 2>&1 &
   ```

   Result: The window opens. `<run>/old/settings.sqlite` exists, and
   `<run>/old/config.json` keeps its text.

3. Examine the settings file and the store.

   ```sh
   cat <run>/old/config.json
   sqlite3 <run>/old/settings.sqlite \
     "SELECT data FROM settings WHERE section = 'shortcuts';"
   ```

   Result: The file holds exactly `{"shortcuts": {"command_palette": "Alt+K"}}`.
   The store holds `Alt+K` for the command palette, because a migration keeps a
   value that a person set, and it holds a default for each shortcut that the old
   file did not have.

4. Prove that the migration wrote no other file.

   ```sh
   ls <run>/old
   ```

   Result: The output has `config.json` and `settings.sqlite`, and no other file.

### 6.7 S07 — The Settings window saves the store only

1. Keep the candidate of S06 open and open the Settings window.

   Press <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>,</kbd>.

   Result: The window shows the tabs General, Keys, Presets, Browser, Remote,
   Secrets, Server and Speech on one row. There is no tab of a text format.

2. In General, select another theme, and press **Save**.

   Result: The status line at the bottom of the window shows `Saved`. The window
   answers the pointer and the keyboard while it saves. A fast store finishes
   the write in one frame, so `Saving` can be too short to see; the check of the
   store in the next step is the proof of the write.

3. Read the store, and prove that no settings file appeared or changed.

   ```sh
   sqlite3 <home>/.horizon/settings.sqlite \
     "SELECT data FROM settings WHERE section = 'appearance';"
   ls -A <home>/.horizon
   ```

   Result: The row holds the new theme. The home has the same files as before the
   save, and the settings file of S02 keeps its bytes.

4. Set a value in **Browser host** and in **Server**, and press **Save**. Then
   type a profile with no SSH target, and press **Save** again.

   ```sh
   sqlite3 <home>/.horizon/settings.sqlite \
     "SELECT data FROM settings WHERE section IN ('browser_host', 'server');"
   ```

   Result: The first save writes both sections, and the store holds the target and
   the command that you typed. The second save reports the rule of the entry
   (`browser_host.profile names a profile of a host, so browser_host.ssh_target is
   required`) in the status line, and the store keeps the value of the first save.

5. Set a value in **Remote hosts** and in **Browser**, and press **Save**.

   ```sh
   sqlite3 <home>/.horizon/settings.sqlite \
     "SELECT data FROM settings WHERE section IN ('remote_hosts', 'browser');"
   ```

   Result: The store holds the workspace name, the VNC port and the per-host
   port that you typed, and it holds the engine path, the picture quality, the
   options of a recording and the passkey command that you typed. The tab
   `Browser` has a field for each of them, and a value that a rule refuses, such
   as an empty workspace name, shows that rule under its field.

6. Press **Save** again without a change.

   Result: The status line shows `Saved` again. The store keeps the same values,
   and the window does not report an error.

## 7. Pass criteria

- The first start leaves the settings file as it is, byte for byte, in S01.
- The store holds every section in S01.
- A settings file next to the store changes no setting, and keeps its bytes, in
  S02.
- A YAML file changes no setting, its home gets no store, and `--config` on it
  reports the JSON format, in S03.
- S04 reports the damaged store, keeps it under its new name, and brings the
  settings back from the settings file. A damaged store with no settings file
  stops the start.
- Every session opens in S05, and the session counts are equal.
- The settings file and the migrated store agree in S06, and the migration writes
  no other file.
- The window saves and shows `Saved` in S07; only the store holds the saved
  value, and a refused entry reports its rule. The tabs of the window hold a
  field for every setting that a person changes: General, Keys, Presets,
  Browser, Remote, Secrets, Server and Speech.

## 8. Cleanup

1. Stop the candidate and the fixture, and remove only `<run>`.

   Result: The Device panel of this run is closed. Your own Horizon and desktop
   did not change.
