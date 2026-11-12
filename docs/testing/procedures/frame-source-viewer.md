---
procedure: frame-source-viewer
feature: native Device panel of a frame source program
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Frame source viewer test procedure

## 1. Purpose

This procedure proves that a native Device panel shows the pictures of a frame
source program of the settings: that the viewer connects, that its pictures
move, that a hidden viewer sends no picture, that the size follows the panel,
and that `close` stops the program.

## 2. Applicability

- Candidate: a build that holds `crates/horizon-frame-source` and the section
  `frame_sources` of the settings.
- Platforms: Linux and macOS. The fixture needs `python3`.
- This procedure does not test: a real frame source program, a screen capture
  program, and any program of another product. The stand-in program draws a
  pattern of its own and opens no device.

## 3. Safety

> **CAUTION:** DO NOT START A CAPTURE PROGRAM OF THIS COMPUTER IN THIS PROCEDURE. THE
> STAND-IN OF THE FIXTURE IS THE ONLY PROGRAM THAT THESE STEPS START.

## 4. Equipment and preconditions

- A Linux computer with the fixture of `scripts/device-smoke/README.md`, or a
  macOS computer with `python3`, `Xvfb` and the same fixture.
- A debug build of Horizon: `cargo build`.
- A Horizon native VNC Device panel in the current workspace, which shows the
  isolated desktop of the fixture live.
- The stand-in program `scripts/device-smoke/frame_source_standin.py`.

## 5. Setup

1. Start the fixture with the stand-in as one frame source and the native view:

   ```bash
   python3 scripts/device-smoke/serve.py --horizon target/debug/horizon \
     --state "${TMPDIR:-/var/tmp}/frame-source-run" --native-view \
     --frame-source "standin=/usr/bin/python3 $(pwd)/scripts/device-smoke/frame_source_standin.py --width 640 --height 360 --rate 10 --report ${TMPDIR:-/var/tmp}/frame-source-run/report.json"
   ```

   Result: the fixture prints one JSON line with `display`, `vnc_address` and
   `pids`, and Horizon opens on its own desktop.

2. Create the viewer in the calling agent's workspace with the public
   `device_panel` tool:

   ```json
   { "operation": "create", "frame_source": { "name": "standin" } }
   ```

   Result: the answer holds a `panel_id` and `status: "panels"`.

## 6. Tasks

### 6.1 F1 — The viewer connects and shows a moving pattern

1. Inspect the viewer with `{ "operation": "inspect", "panel_id": "<panel_id>" }`.

   Result: `kind` is `frame_source`, `connection` is `connected`, and
   `server.desktop_size` is `[640, 360]`.

2. Watch the panel in the native Device panel of the workspace.

   Result: the pattern moves, and the number at its top left changes.

3. Wait two seconds and inspect again.

   Result: `frame_sequence` and `received_frame_sequence` are higher, and
   `frames_dropped` and `age_ms` are present.

### 6.2 F2 — The picture follows the size of the panel

1. Make the panel smaller with the resize handle of its corner.

   Result: the picture gets smaller, and it keeps its shape.

2. Wait one second and inspect the viewer.

   Result: `server.desktop_size` is the new size and not larger than `[640, 360]`.

### 6.3 F3 — A hidden viewer sends no picture

1. Hide the panel with `{ "operation": "visibility", "panel_id": "<panel_id>",
   "visible": false }`.

   Result: the panel is not drawn.

2. Wait three seconds and inspect the viewer.

   Result: `connection` is `connected`, `image_displayed` is false, and `age_ms`
   is larger than before.

3. Show the panel with `visible: true`.

   Result: the picture moves again, and `image_displayed` is true after the next
   frame.

### 6.4 F4 — Interact is off for a source that reads no input

1. Turn Interact on in the panel with the person's own choice.

   Result: the control is disabled, and its hover text says that the frame source
   takes no input, with the reason that the program gave.

### 6.5 F5 — Close stops the exact program

1. Read the `pid` of the report of the fixture with

   ```bash
   cat "${TMPDIR:-/var/tmp}/frame-source-run/report.json"
   ```

   Result: one JSON line holds the `pid`, `socket_fd: 3` and `socket: true`.

2. Close the viewer with `{ "operation": "close", "panel_id": "<panel_id>" }`.

   Result: the answer is `status: "closed"`.

3. Check that the program is gone with `ps -p <pid>`.

   Result: `ps` prints no process.

## 7. Pass criteria

- F1 to F5 pass.
- No step starts a program that reads the screen of the computer.
- The person's own Horizon, terminals and desktop are unchanged.

## 8. Cleanup

1. Close the fixture window normally.

   Result: the fixture ends and its processes are gone.

2. Remove the state folder of the run.

   Result: the folder is gone.

## 9. Record of results

Put the results in the pull request. Keep the screenshots and the video of the
run private.
