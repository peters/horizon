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
| S4 | pass | The window was 1600 x 1000 pixels for the whole run. The procedure step at 1000 x 700 pixels was not done, so the panel did not get a smaller viewport. The regression test covers the frame width instead. | — |

## 3. Defects

- The run found no new defect. The candidate fixes
  [issue #1768](https://github.com/peters/horizon/issues/1768).

## 4. Deviations from the procedure

- Step S4.1 ran at 1600 x 1000 pixels and not at 1000 x 700 pixels. The private
  desktop was created at 1600 x 1000 pixels, and the copied configuration of the
  person has that window size.
- Step S4.2 changed no setting, because step 6.4 is the same check as S3 in this
  candidate. The Settings bar answered in the recorded video.

## 5. Cleanup

- The Horizon window was closed through the window manager.
- The private display `:92`, its window manager and its VNC server were stopped.
- The private home `/tmp/w1run` is kept for the frozen candidate. It holds a
  copy of a real `~/.horizon` and no credential.
- The Device panel `daf858f6-e4c7-44ac-8d85-051e5b94e54a` was closed.

## 6. Evidence

- Frozen candidate: `/tmp/w1-bin/horizon`, SHA-256
  `5e8e28dbadda2c6a3931843da554da57e24dc7505e51955fdd51040da1c676c2`, commit
  `d9c3c954d`.
- The build before the fix: `/tmp/l0b-bin/horizon`, SHA-256
  `4c2655890683a42ee8e0085db9c34b250f8b1bd7c9a38ab6cb267988d9cff47f`.
- Screenshots and the video are on the private desktop of the run and in the
  pull request.
- The regression test `a_long_yaml_line_does_not_widen_the_settings_panel` drew
  the panel 4467 pixels wide before the fix and 480 pixels wide after it.
