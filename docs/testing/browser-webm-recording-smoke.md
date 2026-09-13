# Browser WebM recording smoke

Temporary plan for [#587](https://github.com/peters/horizon/issues/587). Delete after the UI validation pass unless asked to keep it.

Launch the exact candidate (`target/debug/horizon`) with an isolated `--config` and `--ephemeral` so this does not reuse the operator session. Unset `HORIZON` for the child. Do not touch `~/github/horizon`; use a task-owned worktree at the exact PR SHA.

## Lanes

1. **Chrome Record control (Linux Chromium or macOS/Metal Chrome)**
   - Open a Browser panel to a local fixture or `example.com`.
   - Click Record. Confirm a red elapsed timer appears and the page still paints.
   - Navigate and scroll for ~5 seconds.
   - Pause: timer shows paused, page stays live.
   - Resume: timer continues, file keeps growing.
   - Stop: timer clears; hover Record shows a `.webm` path under the panel `captures/` directory.
   - Open the file in a player or Chromium. Playback duration matches active (non-paused) time. Horizon chrome is not in the video.

2. **Static page**
   - Record a fully loaded static page for ~3 seconds without input.
   - Stop. File is non-empty and plays a still of the last page.

3. **Compression knobs**
   - Set `browser.video.quality: 40` and `compression_level: 0` in the temp config, record 3 seconds, note size.
   - Repeat with `quality: 90` and `compression_level: 8`. Higher quality/higher compression should not crash; sizes may differ.

4. **MCP**
   - Unattended runner: `python3 scripts/browser-smoke/video_smoke.py --backend chromium --ephemeral` (add `--chromium-command` on macOS).
   - `browser_video` start → status (`recording`) → pause → resume → stop.
   - Returned `path` exists and starts with EBML `1A 45 DF A3`.
   - Audit records the operation and numeric options, not pixels.

## macOS / fintermac (Metal)

Checkout the exact PR SHA into `/Users/fintermac/horizon-smoke/pr-590-webm` (never `~/github/horizon`). `PATH=/opt/homebrew/bin:$PATH git lfs pull`, then `cargo build`. Run lane 4 against `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`. Capture a screenshot of the task-owned window after Record if a console session is available. Close only that PID.

5. **Backend switch / close**
   - Start recording, switch Chromium → Firefox (or close the panel).
   - Encoder thread is gone (`pgrep -af browser-video-encode` empty for that process). A `.webm` remains in captures.

6. **File limit**
   - Start with `max_file_bytes: 65536`. Record until status reports `file_limit_reached`. File is still playable.

## Not covered

Audio, canvas-wide capture, crash-resume of encoder state, Safari unless a macOS lane is requested.
