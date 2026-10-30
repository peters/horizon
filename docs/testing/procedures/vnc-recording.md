---
procedure: vnc-recording
feature: Device panel video
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# VNC recording test procedure

## 1. Purpose

This procedure tests video capture from a Device panel through the UI and MCP.
It tests access control, background capture, unchanged-desktop holding, the
capture-quality grade, file completion, and temporary storage.

## 2. Applicability

- Use a current Horizon candidate. The app includes the video encoder.
- Use synthetic content on an isolated desktop.
- This procedure does not test audio or browser video.
- PANEL-ICON-01 checks the camera and record icons on a Device panel.
- Browser recording is a separate procedure.
- Linux uses the local device fixture. Other platforms need a separate isolated desktop.
- The candidate targets 5 fps at the full desktop size. The file rate depends on
  the source desktop and the build. A 1600 × 900 desktop with continuous motion
  writes about 6 fps in a release build and about 3 fps in a debug build on a
  48-thread reference host.

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

1. Put the pointer on the record icon.

   Result: The hover text names the recording limits.

2. Click the record icon.

   Result: The status says **Recording VNC desktop**. The stop icon is shown.

3. Put the pointer on the stop icon.

   Result: The hover text says **Stop recording**.

4. Change the synthetic content for five seconds.
5. Click the stop icon.
6. Wait until the frame count and the clipboard icon are shown.
7. Put the pointer on the clipboard icon.

   Result: The hover text names Copy video path and the private recording file.

8. Copy the path and play the file.

   Result: The file shows the full remote desktop in WebM format without audio.
   Its frames show the changes in the correct order.

9. Start another recording, resize the source, and change the panel's crop and scale.

   Result: The recording continues. The panel controls do not crop the recording.

10. Make a recording of a desktop whose content does not change. Stop the
    recording after ten seconds.

    Result: The panel shows the frame count without a warning. The file holds one
    frame and its duration covers the recording.

### PANEL-ICON-01 — Camera and record icons

1. Open a Device panel that is not connected.

   Result: The camera is dim. The record disc is dim.

2. Connect the panel and look before the first desktop frame is on screen.

   Result: The camera is dim. The record disc is dim.

   If the frame is on screen, continue at the next step.

3. Wait until the panel shows the desktop.

   Result: The camera is bright. The record disc is rose.

4. Put the pointer on the camera icon.

   Result: The hover text names the copy action.

5. Put the pointer on the record icon.

   Result: The hover text names the recording limits.

### VNC-VIDEO-02 — MCP and background capture

1. Use the candidate's public MCP server to create the source panel.
2. Send `device_panel` with `{"operation":"video","panel_id":"<id>","action":"start"}`.
3. Hide the panel with the public visibility operation.
4. Change the synthetic source content for five seconds.
5. Send the same video operation with `"action":"status"`.

   Result: `recording.capture.frames_encoded` advances while the panel is hidden.

6. Send the video operation with `"action":"stop"`.
7. Poll status until `recording.capture.active` and `recording.finalizing` are false.
8. Examine `encoder_failed`, `recording.capture.quality`, `frames_encoded`, and
   `frames_offered`, then the file at `recording.capture.path`.

   Result: `encoder_failed` is false. The file has decodable frames and includes
   the hidden interval. `quality` is `good` when the host wrote at least four of
   every five desktop states it could write, and `unknown` when no frame was
   written yet.

9. Examine `recording.capture.fps`, `recording.capture.frames_repeated`, and
   `recording.capture.effective_fps` after a recording of a desktop that did not change.

   Result: `fps` is 5. `frames_repeated` advances. `effective_fps` is low
   although `quality` is `good`, because the desktop had one state to write.

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

### VNC-VIDEO-04 — Capture-quality warning

Use a source desktop that changes faster than the candidate can write it. A
2560 × 1440 desktop with continuous motion is one example. A debug build of the
candidate is another.

1. Start a recording in the UI and change the whole desktop for fifteen seconds.
2. Stop the recording.

   Result: The panel shows **Recording fell behind** or **Recording fell far
   behind**. The yellow text means `quality` is `degraded`. The red text means
   `quality` is `poor`.

3. Put the pointer on the warning.

   Result: The hover text gives the written rate, the target rate, the number of
   held samples, and the number of missed samples.

4. Send the video operation with `"action":"status"`.

   Result: `recording.capture.quality` is `degraded` or `poor`. The written rate
   in `recording.capture.effective_fps` is below `recording.capture.fps`.

5. Decode frames at the two ends of a burst of motion.

   Result: The frames show the motion that the file holds. Report that a short
   state can be absent. Do not claim that the file shows all motion.

6. Report the measured `quality`, `effective_fps`, `frames_offered`, and
   `frames_dropped` with the evidence.

### VNC-VIDEO-05 — Limits and retention

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

- The camera and the record disc are dim until the session has a desktop frame.
- UI and MCP use the same recorder.
- Background capture works without panel rendering.
- Unauthorized operations return no pixels or file paths.
- Completed files decode. Disconnect and close do not block the UI.
- An unchanged desktop writes one frame and reports no missed sample.
- `quality` is `good` when the host keeps at least four of every five desktop
  states it can write. A `degraded` or `poor` capture shows a warning in the
  panel and a written rate below the target.
- Temporary files follow the retention limit.

## 8. Cleanup

1. Save any required synthetic evidence before panel close.
2. Close the task-owned panels and stop the isolated fixtures.
3. Make sure their processes exit and temporary recordings disappear.

## 9. Record of results

Put the candidate hash, platform, task results, and evidence limits in the PR.
Keep private evidence outside the repository.
