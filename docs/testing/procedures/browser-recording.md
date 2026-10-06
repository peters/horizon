---
procedure: browser-recording
feature: Browser panel video
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Browser recording test procedure

## 1. Purpose

This procedure tests video capture from a Browser panel through the UI and MCP.
It tests the camera, record, and stop icons, pause and resume, and the completed file.

## 2. Applicability

- Use a current Horizon candidate. The app includes the video encoder.
- Use Chromium on Linux, or Chrome on macOS.
- Use a local page or a public fixture page.
- This procedure does not test audio, canvas capture, or Safari.
- Device panel video is in the VNC recording procedure.

## 3. Safety

> **CAUTION: USE A TASK-OWNED CANDIDATE.** The recording contains the browser page.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- An isolated config. The run does not use the operator session.
- A WebM player or FFmpeg.

## 5. Setup

1. Start the candidate from a task-owned worktree. Use an isolated config and `--ephemeral`.

   Result: The candidate does not use the operator session.

2. Open a Browser panel.

## 6. Tasks

### BROWSER-VIDEO-01 — Icons

1. Look at the chrome before the page has a frame.

   Result: The record disc is rose. The camera is dim.

2. Open a page that produces a frame.

   Result: The camera is bright. Its hover text names the copy action.

3. Put the pointer on the record icon.

   Result: The hover text names the WebM recording.

### BROWSER-VIDEO-02 — Record, pause, and stop

1. Click the record icon.

   Result: A red elapsed timer appears. The page still paints. The stop icon is shown.

2. Put the pointer on the stop icon.

   Result: The hover text says **Stop recording**.

3. Navigate and scroll for five seconds.
4. Click the pause control.

   Result: The timer shows paused. The page stays live.

5. Click the resume control.

   Result: The timer continues. The file size increases.

6. Click the stop icon.

   Result: The timer clears. The record icon returns.

7. Put the pointer on the record icon.

   Result: The hover text shows a `.webm` path under the panel captures directory.

### BROWSER-VIDEO-03 — Playback

1. Open the completed file in a player.

   Result: The duration matches the active record time. The Horizon chrome is not in the video.

### BROWSER-VIDEO-04 — Static page

1. Record a fully loaded static page for three seconds. Do not send input.
2. Click the stop icon.

   Result: The file is not empty. It shows a still of the last page.

### BROWSER-VIDEO-05 — Compression

1. Set `browser.video.quality` to 40 and `compression_level` to 0.
2. Record for three seconds and note the file size.
3. Repeat with quality 90 and `compression_level` 8.

   Result: The candidate does not crash. The two file sizes can differ.

### BROWSER-VIDEO-06 — MCP

1. Run `python3 scripts/browser-smoke/video_smoke.py --backend chromium --ephemeral`.
2. Start, read status, pause, resume, and stop through `browser_video`.

   Result: The returned path exists. The file starts with the EBML bytes `1A 45 DF A3`.

### BROWSER-VIDEO-07 — Backend switch

1. Start a recording.
2. Switch the backend, or close the panel.

   Result: The encoder process for that recording is gone. A `.webm` file remains.

### BROWSER-VIDEO-08 — File limit

1. Set `max_file_bytes` to 65536.
2. Record until the status says `file_limit_reached`.

   Result: The file still plays.

## 7. Pass criteria

- The camera is dim until a page frame exists, then bright.
- The record disc is rose when capture can start. The stop icon replaces it during capture.
- Pause and resume keep one file. Playback matches the active time.
- MCP returns a private WebM file and does not return pixels in the audit.

## 8. Cleanup

1. Save any required synthetic evidence before you close the panel.
2. Close the task-owned panels and stop the candidate.

   Result: The candidate process exits.

## 9. Record of results

Put the candidate hash, platform, task results, and evidence limits in the PR.
Keep private evidence out of the repository.
