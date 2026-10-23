---
procedure: settings-store
candidate_commit: f6ea89074
candidate_sha256: 4c2655890683a42ee8e0085db9c34b250f8b1bd7c9a38ab6cb267988d9cff47f
date: 2026-10-23
lanes: [linux]
issue: https://github.com/peters/horizon/pull/1769
---

# Settings store test report, store-only candidate, 2026-10-23

## 1. Summary

All seven tasks of [the settings store procedure](../procedures/settings-store.md)
passed on Linux, on the candidate that removes the live `config.yaml`. The
candidate was the frozen debug binary with the hash above, built from commit
`f6ea89074`, and `/proc/<pid>/exe` of the fixture process had that hash. The run
used a copy of a real Horizon home at the path `/home/peters` in a private mount
namespace, so the session profile, the six saved sessions and the paths were the
same as in the real home. The desktop was an isolated Xvfb display, 1600 by 1000,
with its own window manager and loopback VNC server, watched in one native Device
panel. The views and the logs in this report are private evidence.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| S01 | pass | The first start logged `importing /home/peters/.horizon/config.yaml into .../settings.sqlite` and then `kept as /home/peters/.horizon/config.yaml.imported-1792792961`. The home held `settings.sqlite`, the kept file and `sessions`, and no `config.yaml`. The kept file had SHA-256 `af44f040…`, which was the hash of the copy before the start. The store held the ten sections, schema version 1, and `{"theme":"light"}`, and the window opened in the light theme. | — |
| S02 | pass | A `config.yaml` with `theme: dark` beside the store was not read: the log said `the settings of this computer are in .../settings.sqlite; .../config.yaml is not read`, the store still held `light`, and the window stayed light. The SHA-256 of the file was the same after the start. | — |
| S03 | pass | `--export-config` with no path wrote `/home/peters/.horizon/config.yaml` and reported the store path and `Horizon does not read this file`. The export held `theme: light`. An edit of that export to `theme: dark` changed no setting at the next start, and the file kept its bytes. In a home with no store, `--export-config` reported `no settings to export: .../none/settings.sqlite does not exist`, exit code 2, and wrote no file. | — |
| S04 | pass | A store that held `not a database` was reported, moved to `settings.sqlite.damaged-1792793359`, and written again from `config.yaml`; the window came up in the dark theme of that copy, and the copy was kept as `config.yaml.imported-1792793359`. The damaged file still held `not a database`. A damaged store in a home with no copy stopped the start with `cannot read the settings of ...: ... is damaged` and `Move the file aside to start with the default settings`, exit code 2, and the damaged file stayed where it was. | — |
| S05 | pass | The Sessions chooser listed the saved session as 38 workspaces and 74 panels. **Switch Selected** opened it: the sidebar listed 38 workspaces, and the panels of the visible workspaces drew their content. | — |
| S06 | pass | A file with only `shortcuts: {command_palette: Alt+K}` was imported, migrated to version 11 in `old/settings.sqlite`, and kept byte for byte as `config.yaml.imported-1792793401`. The store held `Alt+K` for the command palette and a default for every other shortcut. The folder held the kept file and the store, and no `config.yaml`. | — |
| S07 | pass | In the Settings window, the **Height** field changed from 1000 to 985 and **Save** answered `Saved` in the bar at the bottom of the panel. The store then held `{"width":1600.0,"height":985.0,"x":null,"y":null}`, the log said `settings saved`, and no YAML file appeared in the home. | — |

## 3. Defects

- None found in this candidate.

## 4. Deviations from the procedure

- The procedure writes a small `config.yaml` into the test home. The run copied
  the real `config.yaml` instead, because the settings of the person are the
  interesting input for a migration, and it set `theme: light` before the start
  so that the theme shows which file won.
- The copy carried the real window geometry, 5120 by 1300, which is larger than
  the test desktop. The window was resized with `xdotool windowsize 1600 1000`
  before the interactive steps. The first attempt to open Settings drew the panel
  at the far right of the 5120-pixel window, outside the desktop, which looked
  like a failed shortcut.
- On the General tab the canvas still draws over the left part of the Settings
  panel ([issue #1768](https://github.com/peters/horizon/issues/1768)). S07
  therefore changed the **Height** field, which lies to the right of the overlap,
  and not the theme.
- `Saving` was not visible: the store write of this home finishes in a few
  milliseconds. The bar showed `Saved`. The unit test
  `app::settings::tests::a_save_shows_saving_and_then_saved` covers both states.
- S03 exported while the candidate was open. The export reads the store and
  writes one file, so it does not need the window to be closed.
- The first start of the run did not set `HORIZON_AGENT_INTEGRATIONS=off`, so the
  candidate synced its bundled agent skills into
  `~/.claude/skills/synced/<lease>/` of the real home, as the running Horizon
  does. Every later start set the variable to `off`. The synced bundle is the
  same content that the running Horizon installs, and its next sync rewrites it.
- S05 opened the saved session once and compared the workspace count, 38, with
  the real home. It did not open each of the 38 workspaces.
- The device panel was created, revealed once, and showed
  `presentation: "displayed"` with an advancing frame sequence before each
  screenshot.

## 5. Cleanup

- The frozen binary stayed in `/tmp/l0b-bin`, and the run directory `/tmp/l0n2`
  was removed after the run.
- The Device panel of this run was closed, and the Xvfb display, the window
  manager and the VNC server of the run were stopped.
- The real home was not changed by the run: the candidate wrote into the private
  mount namespace only.
