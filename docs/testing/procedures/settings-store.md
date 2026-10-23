---
procedure: settings-store
feature: settings and configuration in SQLite
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Settings store test procedure

## 1. Purpose

This procedure proves that Horizon keeps its settings in one SQLite file, that
it imports an old `config.yaml` one time, and that it never reads a YAML file
again. It also proves that an export is not a setting, and that a damaged store
does not replace the settings of a person with defaults.

## 2. Applicability

- Candidate: a build that keeps its settings in `settings.sqlite` and writes no
  `config.yaml`.
- Platforms: Linux.
- Each task runs on a copy of a real Horizon home, so the run shows the
  migration of the settings that a person has today.
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

2. Copy the real Horizon home of this computer into the test home. This is the
   instance that the procedure migrates.

   ```sh
   cp -a "$HOME/.horizon/sessions" <home>/.horizon/
   cp -a "$HOME/.horizon/config.yaml" <home>/.horizon/config.yaml
   sed -i 's/^  theme: .*/  theme: light/' <home>/.horizon/config.yaml
   sha256sum <home>/.horizon/config.yaml > <run>/copy.sha256
   ```

   Result: The test home holds the real sessions and the real configuration,
   with the light theme.

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

### 6.1 S01 — The first start imports the configuration one time

1. Look at the files of the Horizon home.

   ```sh
   ls -A <home>/.horizon
   ```

   Result: The output has `settings.sqlite`, `config.yaml.imported-<time>`, and
   `sessions`. It has no `config.yaml`, because the start kept the imported file
   aside.

2. Compare the kept file with the copy that you made.

   ```sh
   sed 's|config\.yaml$|config.yaml.imported-*|' <run>/copy.sha256 | sha256sum -c -
   ```

   Result: The check passes. The import did not change one byte of the file.

3. Read the sections of the store.

   ```sh
   sqlite3 <home>/.horizon/settings.sqlite 'SELECT section FROM settings ORDER BY section;'
   ```

   Result: The output names `appearance`, `browser`, `features`, `overlays`,
   `presets`, `remote_hosts`, `shortcuts`, `version`, `window`, and
   `workspaces`.

### 6.2 S02 — A YAML file next to the store is not a setting

1. Stop the candidate through its window.

   Result: The process ends. The fixture stays.

2. Write a configuration file with the dark theme next to the store.

   ```sh
   sed 's/^  theme: .*/  theme: dark/' <home>/.horizon/config.yaml.imported-* \
     > <home>/.horizon/config.yaml
   sha256sum <home>/.horizon/config.yaml > <run>/yaml.sha256
   ```

   Result: The home holds a store and a `config.yaml` with the dark theme.

3. Start the candidate again with the same home, and read the log.

   ```sh
   env DISPLAY=<display> HOME=<home> <bin>/horizon --ephemeral > <run>/s02.log 2>&1 &
   rg 'is not read' <run>/s02.log
   ```

   Result: The window uses the light theme. The log names the store and the
   `config.yaml` file.

4. Prove that Horizon did not change the file.

   ```sh
   sha256sum -c <run>/yaml.sha256
   ```

   Result: The check passes. The file stays as you wrote it.

### 6.3 S03 — An export is a copy, not a setting

1. Stop the candidate through its window.

   Result: The process ends.

2. Write an export and examine it.

   ```sh
   <bin>/horizon --export-config <run>/export.yaml
   rg 'theme' <run>/export.yaml
   ```

   Result: The command reports the path of the store and the path of the export.
   The export holds `theme: light`.

3. Change the export, start the candidate, and read the theme.

   ```sh
   sed -i 's/theme: light/theme: dark/' <run>/export.yaml
   env DISPLAY=<display> HOME=<home> <bin>/horizon --ephemeral > <run>/s03.log 2>&1 &
   ```

   Result: The window uses the light theme, because the export is not a setting.

4. Ask for an export in a home without a store.

   ```sh
   <bin>/horizon --config <run>/none/config.yaml --export-config <run>/none.yaml
   ```

   Result: The command reports that there are no settings to export. It writes
   no file.

### 6.4 S04 — A damaged store comes back from the kept configuration

> **CAUTION:** THIS STEP DAMAGES THE TEST STORE. Use only the store under
> `<home>`.

1. Stop the candidate through its window.

   Result: The process ends.

2. Damage the store and start the candidate again.

   ```sh
   printf 'not a database' > <home>/.horizon/settings.sqlite
   env DISPLAY=<display> HOME=<home> <bin>/horizon --ephemeral > <run>/s04.log 2>&1
   ```

   Result: The window opens with the settings of the kept configuration, and the
   theme is light. The output names the store and the path of the damaged file.
   The file `<home>/.horizon/settings.sqlite.damaged-*` exists and holds the
   words `not a database`.

3. Damage a store that has no kept file, in a new home.

   ```sh
   mkdir -p <run>/bare && printf 'not a database' > <run>/bare/settings.sqlite
   env DISPLAY=<display> HOME=<home> <bin>/horizon --config <run>/bare/config.yaml \
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
   mkdir -p <run>/old && printf 'shortcuts:\n  command_palette: Alt+K\n' \
     > <run>/old/config.yaml
   env DISPLAY=<display> HOME=<home> <bin>/horizon --config <run>/old/config.yaml \
     --ephemeral > <run>/s06.log 2>&1 &
   ```

   Result: The window opens. `<run>/old/settings.sqlite` exists, and
   `<run>/old/config.yaml.imported-*` holds the old text.

3. Examine the kept text and the store.

   ```sh
   cat <run>/old/config.yaml.imported-*
   sqlite3 <run>/old/settings.sqlite \
     "SELECT data FROM settings WHERE section = 'shortcuts';"
   ```

   Result: The kept file holds exactly `command_palette: Alt+K`. The store holds
   `Alt+K` for the command palette, because a migration keeps a value that a
   person set, and it holds a default for each shortcut that the old file did
   not have.

4. Prove that the migration wrote no new YAML file.

   ```sh
   ls <run>/old
   ```

   Result: The output has `config.yaml.imported-<time>` and `settings.sqlite`.

### 6.7 S07 — The Settings window saves the store only

1. Keep the candidate of S06 open and open the Settings window.

   Press <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>,</kbd>.

   Result: The window shows the settings in YAML form.

2. Change one value in the text, for example the theme, and press **Save**.

   Result: The status line at the bottom of the window shows `Saved`. The window
   answers the pointer and the keyboard while it saves. A fast store finishes
   the write in one frame, so `Saving` can be too short to see; the check of the
   store in the next step is the proof of the write.

3. Read the store, and prove that no YAML file appeared.

   ```sh
   sqlite3 <home>/.horizon/settings.sqlite \
     "SELECT data FROM settings WHERE section = 'appearance';"
   ls <home>/.horizon
   ```

   Result: The row holds the new theme. The home has no new file, and the
   `config.yaml` that you wrote in S02 is not there: a save writes no YAML file.

4. Press **Save** again without a change.

   Result: The status line shows `Saved` again. The store keeps the same values,
   and the window does not report an error.

## 7. Pass criteria

- The first start keeps the imported file aside, byte for byte, in S01.
- The store holds every section in S01.
- A `config.yaml` next to the store changes no setting, and keeps its bytes, in
  S02.
- An export writes no setting, and a home without a store exports nothing, in
  S03.
- S04 reports the damaged store, keeps it under its new name, and brings the
  settings back from the kept file. A damaged store with no kept file stops the
  start.
- Every session opens in S05, and the session counts are equal.
- The kept text and the migrated store agree in S06, and the migration writes no
  YAML file.
- The window saves and shows `Saved` in S07, and only the store holds the saved
  value.

## 8. Cleanup

1. Stop the candidate and the fixture, and remove only `<run>`.

   Result: The Device panel of this run is closed. Your own Horizon and desktop
   did not change.
