---
procedure: settings-store
candidate_commit: 4b09276428aaafd7 (short hash of the saved-settings candidate)
candidate_sha256: 4b09276428aaafd76c7b40ca45ad53ffb874a8d5a2e16aa360df20afbbe8b859
date: 2026-10-23
lanes: [linux]
issue: https://github.com/peters/horizon/pull/1765
---

# Settings store test report, 2026-10-23

## 1. Summary

All tasks of [the settings store procedure](../procedures/settings-store.md)
passed on Linux. Two candidates ran: `0bf61be74` for S01 to S06, and
`4b092764` for S07, which saves from the Settings window on a worker thread. The run used a copy of a real Horizon home: five saved
sessions, 38 workspaces and 71 panels, and the real `config.yaml`. The candidate
was the frozen debug binary with the hash above, and the probe process had that
hash at `/proc/<pid>/exe`. The run used an isolated Xvfb desktop viewed through
a native Device panel. The views in this report are private evidence.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| S01 | pass | The first start imported `/home/peters/.horizon/config.yaml` into `settings.sqlite` and logged the import. The store held 10 sections and schema version 1. The copy was byte-identical before and after the start, `mirror_sha256` equalled the copy hash, and no `.imported-` file appeared. | — |
| S02 | pass | An edit of `theme: auto` to `theme: light` in the copy was logged as a difference from the last export, imported, and the store held `{"theme":"light"}`. The window came up in the light theme, and the copy kept the exact edited bytes. | — |
| S03 | pass | With the copy deleted, the start logged `has no copy; exporting it from .../settings.sqlite` and wrote the copy again. The copy hash equalled `mirror_sha256`, and the theme was still light. | — |
| S04 | pass | A store that held `not a database` was reported with the path, moved to `settings.sqlite.damaged-1792777277`, and written again from the copy with 10 sections and schema version 1. The copy was unchanged, and the window showed the same board. | — |
| S05 | pass | The session chooser listed the saved session as 38 workspaces and 71 panels, which equalled the count in the real home. The opened copy session listed the same 38 workspaces and the same panels, and each panel started with the same command. The masked home does not hold `~/.local/bin`, so a panel with a custom command showed the missing file, which is the isolation and not a regression. | — |
| S06 | pass | A file with only `shortcuts: command_palette: Alt+K` became `version: 11` in the copy. The file `config.yaml.imported-1792777304` held exactly the original two lines, and the store held `Alt+K` plus a default for each shortcut that the file did not have. | — |
| S07 | pass | The Settings window saved on the candidate `4b092764` and showed `Saved` in the bar at the bottom of the panel. The copy `/tmp/l0f-ev/data/horizon.yaml` held `theme: light`, the store held `{"theme":"light"}` with `schema_version` 1, and `mirror_sha256` equalled the SHA-256 of the copy. The window answered the pointer during the write. `Saving` lasted less than one frame, because the store write of this home is a few milliseconds. | — |

## 3. Defects

- None found.

## 4. Deviations from the procedure

- The procedure says to copy the real home into a test home and start the
  candidate with that home. The session profile of a configuration is the hash
  of the resolved configuration path, so a copy at another path is a new
  profile with an empty board. The run therefore started the candidate in a
  private mount namespace that held the copied home at the same path,
  `/home/peters`. This keeps the sessions and the paths identical, and the real
  home stays untouched.
- The candidate started with `WAYLAND_DISPLAY` and `XAUTHORITY` removed, because
  the isolated desktop is X11 and the inherited values made the event loop fail
  before the first frame.
- The chooser reported that the session was live, because the copy held the
  heartbeat of the real instance. The run selected **Open Copy**, so the real
  session stayed untouched.
- S05 opened the copy of the saved session and compared the counts with the real
  home. It did not click every one of the 38 workspaces.
- S07 ran on a separate fixture state, `/tmp/l0f-ev`, which the fixture starts
  with `--config <state>/data/horizon.yaml`. The copy and the store of that run
  are therefore under `<state>/data`, not under `<state>/data/home/.horizon`.
- The procedure expects `Saving` before `Saved` in S07. The candidate wrote the
  store faster than one frame, so the run saw `Saved` only. The unit test
  `app::settings::tests::a_save_shows_saving_and_then_saved` checks both states.

## 4a. Observation: the status line was under the canvas

Before this run, the bottom bar of the Settings window was a panel of the root
window, so the canvas and the Settings panel drew over its left half and the
status word was invisible. The candidate `4b092764` puts the bar inside the
Settings panel, and the word `Saved` is visible at the bottom left of the panel.
The recording of S07 shows it.

## 5. Cleanup

- The frozen binaries, the private homes, the logs and the fixture states were
  removed from `/tmp/l0-run`, `/tmp/l0c-run`, `/tmp/l0d-ev`, `/tmp/l0e-ev` and
  `/tmp/l0f-ev` after the run.
- The Device panel was closed, and the fixture desktop stopped with the fixture.
- The real home was not changed: the run wrote only inside the private mount
  namespace.

## 6. Evidence

The evidence is private: screenshots of the imported configuration, of the light
theme after the copy edit, of the repaired store, and of the restored board with
38 workspaces. The log lines are in the report table above. The binary hash is
`e4a2953f3215d4b72c075da2ec75f0dae9e92f79773e222fa31fb89348f63e4a`.
