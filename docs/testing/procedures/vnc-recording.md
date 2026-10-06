---
procedure: vnc-recording
feature: Device panel video
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# VNC recording test procedure

## 1. Purpose

This procedure tests video capture from a Device panel through the UI and MCP.
It tests access control, background capture, file completion, and temporary storage.

## 2. Applicability

- Use a current Horizon candidate. The app includes the video encoder.
- Use synthetic content on an isolated desktop.
- This procedure does not test audio or browser video.
- Linux uses the local device fixture. Other platforms need a separate isolated desktop.

## 3. Safety

> **CAUTION: USE ONLY THE ISOLATED DESKTOP.** The recording contains the full remote desktop, including areas outside the visible panel.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- A live view through a task-owned Device panel in the user's workspace.
- An isolated VNC source with a clock and changing synthetic content.
- An MCP agent in the candidate's workspace.
- A WebM player or FFmpeg to examine the completed file.

## 5. Setup

1. Start the candidate with the local device fixture and `--native-view`.
2. Create the live view with the public `device_panel` tool.
3. Examine three inspections, two seconds apart.

   Result: The panel displays the image and its frame counter advances.

4. Create a Device panel inside the candidate for the isolated VNC source.

## 6. Tasks

### VNC-VIDEO-01 — UI capture

1. Click **Record video** in the candidate's Device panel.

   Result: **Recording VNC desktop** and **Stop recording** appear.

2. Change the synthetic content for five seconds.
3. Click **Stop recording**.
4. Wait for the frame count and **Copy video path**.
5. Copy the path and play the file.

   Result: The file shows the full remote desktop in WebM format without audio.
   Its frames show the changes in the correct order.

6. Start another recording, resize the source, and change the panel's crop and scale.

   Result: The recording continues. The panel controls do not crop the recording.

### VNC-VIDEO-02 — MCP and background capture

1. Use the candidate's public MCP server to create the source panel.
2. Send `device_panel` with `{"operation":"video","panel_id":"<id>","action":"start"}`.
3. Hide the panel with the public visibility operation.
4. Change the synthetic source content for five seconds.
5. Send the same video operation with `"action":"status"`.

   Result: `recording.capture.frames_encoded` advances while the panel is hidden.

6. Send the video operation with `"action":"stop"`.
7. Poll status until `recording.capture.active` and `recording.finalizing` are false.
8. Examine `encoder_failed`, `frames_encoded`, and the file at `recording.capture.path`.

   Result: `encoder_failed` is false. The file has decodable frames and includes the hidden interval.

### VNC-VIDEO-03 — Access and lifecycle

1. Try start, status, and stop from a different agent.

   Result: Each operation returns `not_owner` without a file path.

2. Try each operation from another workspace.

   Result: Each operation returns `panel_unavailable`.

3. Try start before the source sends its first frame.

   Result: The operation fails without a recording.

4. Start twice on the same panel.

   Result: The second start fails and the first recording continues.

5. Stop the source during a recording.

   Result: The recording finishes without more UI frames or MCP requests.

6. Reconnect the source.

   Result: The old recording stays stopped. A new recording requires start.

7. Close a panel during a recording.

   Result: Other panels remain responsive. The background task finishes and removes its temporary file.

8. Restore a saved session.

   Result: No recording starts automatically.

### VNC-VIDEO-04 — Limits and retention

1. Record for five minutes.

   Result: Capture stops automatically. Status reports the completed file.

2. Make four subsequent recordings on the same panel.

   Result: The oldest file is removed. The four latest files remain.

3. Examine file permissions on Unix.

   Result: The directory is private and each WebM file has mode 0600.

4. Run the standalone browser-library test with `cargo test -p horizon-browser --no-default-features native_recorder_refuses`.

   Result: The library refuses recording without an output file. This does not
   test a Horizon app configuration; the app always includes the encoder.

## 7. Pass criteria

- UI and MCP use the same recorder.
- Background capture works without panel rendering.
- Unauthorized operations return no pixels or file paths.
- Completed files decode. Disconnect and close do not block the UI.
- Temporary files follow the retention limit.

## 8. Cleanup

1. Save any required synthetic evidence before panel close.
2. Close the task-owned panels and stop the isolated fixtures.
3. Make sure their processes exit and temporary recordings disappear.

## 9. Record of results

Put the candidate hash, platform, task results, and evidence limits in the PR.
Keep private evidence outside the repository.
