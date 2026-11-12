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
source program: that the viewer connects, that its pictures move, that a person
opens a viewer, that the pointer and the keyboard of a person reach the program,
that a hidden viewer sends no picture, that the size follows the panel, and that
`close` stops the program.

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

> **CAUTION:** DO NOT TYPE A TARGET WITH AN ARGUMENT. THE TARGET OF A VIEWER IS ONE
> ARGUMENT OF THE LINE OF THE SETTINGS, AND HORIZON REFUSES A VALUE THAT HOLDS A
> SPACE OR BEGINS WITH `-`.

## 4. Equipment and preconditions

- A Linux computer with the fixture of `scripts/device-smoke/README.md`, or a
  macOS computer with `python3`, `Xvfb` and the same fixture.
- A debug build of Horizon: `cargo build`.
- A Horizon native VNC Device panel in the current workspace, which shows the
  isolated desktop of the fixture live.
- The stand-in program `scripts/device-smoke/frame_source_standin.py`, which the
  fixture copies into its state folder. The placeholder `{standin}` names that
  copy.

## 5. Setup

1. Start the fixture with the stand-in as one frame source, a panel of the
   scripted client of the `device_panel` tool, and the native view:

   ```bash
   python3 scripts/device-smoke/serve.py --horizon target/debug/horizon \
     --state "${TMPDIR:-/var/tmp}/frame-source-run" --native-view \
     --frame-source-agent \
     --frame-source "standin=/usr/bin/python3 {standin} --width 640 --height 360 --rate 10 --input ${TMPDIR:-/var/tmp}/frame-source-run/input.jsonl --report ${TMPDIR:-/var/tmp}/frame-source-run/report.json"
   ```

   Result: the fixture prints one JSON line with `display`, `vnc_address` and
   `pids`, and Horizon opens on its own desktop with a panel `Frame source agent`.

2. Select the panel `Frame source agent` and read its last line.

   Result: the line `frame-source-agent: ready, type create <name> [target],
   inspect, watch [n], hide, show, close or list`.

The words of this procedure are typed on the terminal of that panel. Each word is
one operation of the tool `device_panel` of the candidate, and each answer is one
line that starts with `frame-source-agent: `.

## 6. Tasks

### 6.1 F1 — The viewer connects and shows a moving pattern

1. Type `create standin`.

   Result: the answer holds `"panel_id": "d1"`, `"kind": "frame_source"` and
   `"status": "panels"`, and a panel `Frame source standin` opens in the active
   workspace.

2. Type `inspect`.

   Result: `"connection": "connected"`, `"image_received": true`,
   `"image_displayed": true`, and `"desktop_size": [640, 360]`.

3. Type `watch 3`.

   Result: three lines, one each second. `"frame_sequence"` and
   `"received_frame_sequence"` increase, and `"frames_dropped"` is 0.

4. Read the panel.

   Result: the pattern moves, and the number at its top left changes.

5. Read the tag of the row of the panel in the sidebar, and the first line of the
   panel.

   Result: the tag is `VIEW`, the first line is `Frame source standin`, and the
   second line is `Frame source program`.

6. Open `Connection details`.

   Result: the rows `Frame source standin`, `Dropped pictures`, `Age of the newest
   picture` and `Source name` hold the values of the program, and no row says
   `VNC name`. The picture carries no sentence about a desktop with nothing open.

### 6.2 F2 — A person opens a viewer of a program of the settings

1. Open the Settings window, the tab `Remote`, and the section `Frame source
   programs`.

   Result: the row of the program `standin` holds the name, the buttons `Remove`
   and `Open`, and the command line. The line of this run holds `{target}` in no
   argument, so the row holds no field `target`.

2. Press `Open`.

   Result: a second panel `Frame source standin` opens in the active workspace,
   and it connects as in F1.

3. Select the second panel and close it with the button of its titlebar.

   Result: the panel closes, and the first panel is unchanged.

4. Close the Settings window. Press the shortcut of the command palette, and type
   `Open frame source:`.

   Result: the palette lists `Open frame source: standin`.

5. Press the key `Enter`.

   Result: a third panel `Frame source standin` opens and connects.

6. Add the word `{target}` to the end of the command line of the row, and select
   another row of the tab.

   Result: a field `target` shows in the row.

7. Type `lab:1` in the field `target`, and press `Open`.

   Result: a viewer opens, and the row `Target` of its `Connection details` holds
   `lab:1`.

8. Type `-one` in the field `target`, and press `Open`.

   Result: one red line under the row says that the target of a frame source
   viewer is not one target, and no viewer opens. Remove the word `{target}` from
   the command line again.

### 6.3 F3 — The keyboard of a person reaches the program

> **CAUTION:** TYPE ONLY THE TEXT OF THIS PROCEDURE INTO THE STAND-IN. THE STAND-IN
> WRITES EVERY `input` MESSAGE INTO ITS INPUT FILE, AND A REAL PROGRAM WOULD ACT ON
> THE KEYS.

1. Select the first viewer, turn `Interact` on, and click the middle of the
   picture.

   Result: the line under the toggle changes from `Click the image to type into it`
   to `Keyboard captured, Escape included; click outside the image to release it`.

2. Type `hi`, then press the keys `Up`, `Enter` and `Control+C`.

   Result: the input file of the run holds one `text` event with `hi`, one `key`
   event `up` down and one up, one `key` event `enter` down and one up, and the
   four `key` events of the chord `ctrl`, `c`, `c`, `ctrl`.

3. Read the input file:

   ```bash
   cat "${TMPDIR:-/var/tmp}/frame-source-run/input.jsonl"
   ```

   Result: the file holds no event before the first click on the picture, and each
   event of this step is one JSON line.

### 6.4 F4 — The pointer and the wheel of a person reach the program

1. Turn `Interact` on, and press the toggle `Interact` again.

   Result: the input file holds no `button` event at the point `0, 0`, and no
   `button` event of the press on the toggle.

2. Click the middle of the picture, then the empty part of the row of the picture,
   then the line above the picture.

   Result: the input file holds the press and the release of the middle of the
   picture only, and no event of the other two clicks.

3. Turn one notch of the wheel down over the picture.

   Result: the input file holds one `wheel` event with `"dy": 1`.

4. Turn one notch of the wheel up.

   Result: the input file holds one `wheel` event with `"dy": -1`.

5. Click outside the picture.

   Result: the line under the toggle says `Click the image to type into it`, and
   the input file holds the release of every key that was held.

### 6.5 F5 — The picture follows the size of the panel

1. Make the panel smaller with the resize handle of its corner.

   Result: the picture gets smaller, and it keeps its shape.

2. Type `inspect` in the panel `Frame source agent`.

   Result: `"desktop_size"` is the new size and not larger than `[640, 360]`.

3. Make the panel larger.

   Result: the picture gets larger, and `"desktop_size"` follows it, up to
   `[640, 360]`.

### 6.6 F6 — A hidden viewer sends no picture

1. Type `hide`.

   Result: the answer holds `"visible": false`, and the panel is not drawn.

2. Wait three seconds, and type `inspect`.

   Result: `"connection"` is `connected`, `"image_displayed"` is false,
   `"frame_sequence"` is unchanged, and `"age_ms"` is larger than in the last
   answer of F1.

3. Type `show`.

   Result: the picture moves again, and `"image_displayed"` is true after the next
   frame.

### 6.7 F7 — A source that reads no input is view only

1. Read the program `standin` of the settings, and remove `--input` and the path
   of the input file from its command line.

2. Create a viewer of it as in F1, and try to turn `Interact` on.

   Result: the control is disabled, and its hover text says that the frame source
   takes no input, with the reason that the program gave.

### 6.8 F8 — Close stops the exact program

1. Read the `pid` of the report of the fixture with

   ```bash
   cat "${TMPDIR:-/var/tmp}/frame-source-run/report.json"
   ```

   Result: one JSON line holds the `pid`, `socket_fd: 3` and `socket: true`.

2. Type `close`.

   Result: the answer is `"status": "closed"`, and the panel closes.

3. Check that the program is gone with `ps -p <pid>`.

   Result: `ps` prints no process.

## 7. Pass criteria

- F1 to F8 pass.
- No step starts a program that reads the screen of the computer.
- The person's own Horizon, terminals and desktop are unchanged.

## 8. Cleanup

1. Close the fixture window normally.

   Result: the fixture ends and its processes are gone.

2. Remove the state folder of the run, which holds the input file and the report.

   Result: the folder is gone.

## 9. Record of results

Put the results in the pull request. Keep the screenshots and the video of the
run private.
