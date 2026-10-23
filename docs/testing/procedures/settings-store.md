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
it imports `config.yaml` one time, and that it imports an edited
`config.yaml` at the next start. It also proves that a damaged store stops the
start instead of replacing the settings of a person with defaults.

## 2. Applicability

- Candidate: a build that keeps its settings in `settings.sqlite`.
- Platforms: Linux.
- Each task runs on a copy of a real Horizon home, so the run shows the
  migration of the settings that a person has today.
- This procedure does not test these functions:
  - The changes in the Settings window. Use
    [the Horizon home procedure](horizon-home.md) for the window.
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

2. Write a configuration with a value that you can see in the window.

   ```sh
   cat > <run>/home/.horizon/config.yaml <<'YAML'
   version: 11
   appearance:
     theme: light
   YAML
   ```

   Result: The file has two settings.

3. Copy the real Horizon home of this computer into the test home. This is the
   instance that the procedure migrates.

   ```sh
   cp -a "$HOME/.horizon/sessions" <home>/.horizon/
   cp -a "$HOME/.horizon/config.yaml" <home>/.horizon/config.yaml
   sha256sum <home>/.horizon/config.yaml > <run>/copy.sha256
   ```

   Result: The test home holds the real sessions and the real configuration.

4. Start the fixture with the private Horizon home in `<home>` and note the
   display.

   ```sh
   HOME=<home> python3 scripts/device-smoke/serve.py --horizon <bin>/horizon \
     --native-view --state <state>
   ```

   Result: The output shows `vnc_address` and `display`. The Device panel shows
   the Horizon window with the same board as your own window: the same
   workspaces, the same panels and the same settings.

## 6. Tasks

### 6.1 S01 — The first start imports the configuration

1. Look at the files of the Horizon home.

   ```sh
   ls -A <home>/.horizon
   ```

   Result: The output has `config.yaml`, `settings.sqlite`, and `sessions`. It
   has no file with `.imported-`, because a start that imports nothing keeps no
   second copy.

2. Compare the configuration copy with the copy that you made.

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

### 6.2 S02 — An edited configuration arrives at the next start

1. Stop the candidate through its window.

   Result: The process ends. The fixture stays.

2. Change the theme in the copy.

   ```sh
   sed -i 's/theme: light/theme: dark/' <home>/.horizon/config.yaml
   ```

   Result: The copy says `theme: dark`.

3. Start the candidate again with the same home.

   ```sh
   env DISPLAY=<display> HOME=<home> <bin>/horizon --ephemeral > <run>/s02.log 2>&1 &
   ```

   Result: The window uses the dark theme.

4. Read the theme in the store.

   ```sh
   sqlite3 <home>/.horizon/settings.sqlite \
     "SELECT data FROM settings WHERE section = 'appearance';"
   ```

   Result: The row contains `dark`.

### 6.3 S03 — A missing copy comes back from the store

1. Stop the candidate through its window.

   Result: The process ends.

2. Remove the copy and start the candidate again.

   ```sh
   rm <home>/.horizon/config.yaml
   env DISPLAY=<display> HOME=<home> <bin>/horizon --ephemeral > <run>/s03.log 2>&1 &
   ```

   Result: The window uses the dark theme, and `<home>/.horizon/config.yaml`
   exists again.

### 6.4 S04 — A damaged store comes back from the copy

> **CAUTION:** THIS STEP DAMAGES THE TEST STORE. Use only the store under
> `<home>`.

1. Stop the candidate through its window.

   Result: The process ends.

2. Damage the store and start the candidate again.

   ```sh
   printf 'not a database' > <home>/.horizon/settings.sqlite
   env DISPLAY=<display> HOME=<home> <bin>/horizon --ephemeral > <run>/s04.log 2>&1
   ```

   Result: The window opens with the settings of the copy, and the theme is the
   one of S02. The output contains `settings store` and the path of the damaged
   file. The file `<home>/.horizon/settings.sqlite.damaged-*` exists and contains
   the words `not a database`. `<home>/.horizon/config.yaml` is unchanged.

### 6.5 S05 — Every saved session still opens

1. Open each workspace in the window, and one panel in each workspace.

   Result: Each panel shows its terminal and its name, as in your own window.
   A panel that cannot start again says why, in the same words as before.

2. Compare the count of the sessions with the count in your own home.

   ```sh
   ls <home>/.horizon/sessions | wc -l; ls "$HOME/.horizon/sessions" | wc -l
   ```

   Result: The two counts are the same.

### 6.6 S06 — A migration keeps the text of the person

1. Stop the candidate through its window.

   Result: The process ends.

2. Write an older configuration version in a new test home, and start the
   candidate there.

   ```sh
   mkdir -p <run>/old && printf 'shortcuts:\n  command_palette: Alt+K\n' \
     > <run>/old/config.yaml
   env DISPLAY=<display> HOME=<home> <bin>/horizon --config <run>/old/config.yaml \
     --ephemeral > <run>/s06.log 2>&1 &
   ```

   Result: The window opens. `<run>/old/settings.sqlite` exists, and
   `<run>/old/config.yaml` holds the migrated settings with
   `version: 11`.

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

## 7. Pass criteria

- Each file of the Horizon home in S01 is present after the first start.
- The import changes no byte of the configuration copy in S01.
- The store holds every section in S01.
- The window and the store hold the edited theme in S02.
- The copy comes back in S03.
- S04 reports the damaged store, keeps it under its new name, and brings the
  settings back from the copy.
- Every session opens in S05, and the session counts are equal.
- The kept text and the migrated store agree in S06.

## 8. Cleanup

1. Stop the candidate and the fixture, and remove only `<run>`.

   Result: The Device panel of this run is closed. Your own Horizon and desktop
   did not change.
