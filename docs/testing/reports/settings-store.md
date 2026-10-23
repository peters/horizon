---
procedure: settings-store
candidate_commit: 0bf61be743e5d4e1e422c2d87e01d3e22ba97ac7
candidate_sha256: e4a2953f3215d4b72c075da2ec75f0dae9e92f79773e222fa31fb89348f63e4a
date: 2026-10-23
lanes: [linux]
issue: https://github.com/peters/horizon/pull/1765
---

# Settings store test report, 2026-10-23

## 1. Summary

All tasks of [the settings store procedure](../procedures/settings-store.md)
passed on Linux. The run used a copy of a real Horizon home: five saved
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

## 5. Cleanup

- The frozen binary, the private home, the logs and the fixture state were
  removed from `/tmp/l0-run` after the run.
- The Device panel was closed, and the fixture desktop stopped with the fixture.
- The real home was not changed: the run wrote only inside the private mount
  namespace.

## 6. Evidence

The evidence is private: screenshots of the imported configuration, of the light
theme after the copy edit, of the repaired store, and of the restored board with
38 workspaces. The log lines are in the report table above. The binary hash is
`e4a2953f3215d4b72c075da2ec75f0dae9e92f79773e222fa31fb89348f63e4a`.
