---
procedure: settings-panel
candidate_commit: d9c3c954d
candidate_sha256: 5e8e28dbadda2c6a3931843da554da57e24dc7505e51955fdd51040da1c676c2
date: 2026-10-24
lanes: [linux-x11]
issue: https://github.com/peters/horizon/issues/1768
---

# Settings panel test report, 2026-10-24

## 1. Summary

Tasks S1 to S4 passed on the frozen candidate. The Settings panel kept its
default width on every tab, the YAML editor scrolled in both directions, and the
canvas stayed visible beside the panel. The same home and display with the build
before the fix made the panel about 700 pixels wide, and the panel covered the
canvas and its own first tab.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| S1 | pass | The panel opened at 480 of 1600 pixels, and the canvas kept the rest. | — |
| S2 | pass | The YAML tab drew the highlighted YAML in the same width. The tab row stayed inside the panel. | — |
| S3 | pass | `Shift` and the wheel moved the text to the end of the long lines, and both scroll bars showed. The width did not change. | — |
| S4 | pass | The window was resized to 1000 x 700 pixels. The panel kept 478 of those 1000 pixels, which is 48 percent and below the 50 percent limit, and the canvas stayed visible. `Height` changed from 1000 to 985, the bar showed `Revert` and then `Saved`, and the store held 985. | — |

## 3. Defects

- The run found no new defect. The candidate fixes
  [issue #1768](https://github.com/peters/horizon/issues/1768).

## 4. Deviations from the procedure

- Step S4.1 resized the window with `xdotool windowsize` on a private desktop of
  1600 x 1000 pixels, and not by starting the desktop at 1000 x 700 pixels. The
  result is the same: the panel is sized from the window, not from the desktop.
- Step S4.2 set `Height` to 985 in the General tab and clicked `Save`, because
  the field alone shows a live preview and leaves the write to the bar.

## 5. Cleanup

- The Horizon window was closed through the window manager.
- The private display `:92`, its window manager and its VNC server were stopped.
- The private home `/tmp/w1run` of the first run held a copy of a real
  `~/.horizon`, so its recording showed the settings of the person. That run was
  repeated on a synthetic home `/tmp/w2run`, which holds one synthetic
  `config.yaml` with a long line and an empty `sessions` folder, and the report
  and the pull request use that run only.
- The Device panel `daf858f6-e4c7-44ac-8d85-051e5b94e54a` was closed.

## 6. Evidence

- Frozen candidate: `/tmp/w1-bin/horizon`, SHA-256
  `5e8e28dbadda2c6a3931843da554da57e24dc7505e51955fdd51040da1c676c2`, commit
  `d9c3c954d`.
- The build before the fix: `/tmp/l0b-bin/horizon`, SHA-256
  `4c2655890683a42ee8e0085db9c34b250f8b1bd7c9a38ab6cb267988d9cff47f`.
- The recording of the synthetic run is in the pull request. The first run at
  `/tmp/w1run` was not published, because its recording showed the settings of
  the person.
- The two screenshots of the resize lane are in a comment on the pull request:
  the panel beside the canvas at 1000 x 700 pixels, and the same window after
  `Save` with `Saved` in the bar.
- The store of the run held `window.height: 985.0` after `Save`, which
  `--export-config` showed.
- The regression test `a_long_yaml_line_does_not_widen_the_settings_panel`
  compares the panel with a short line and with a long line on the same machine,
  because the default width and the scroll bar differ by platform. On Linux the
  panel drew 4467 pixels wide with the long line before the fix, and 479 pixels
  with the fix, which is the width of the short line.
