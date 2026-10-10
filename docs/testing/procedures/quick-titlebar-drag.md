---
procedure: quick-titlebar-drag
feature: Quick titlebar drag
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Quick titlebar drag test procedure

## 1. Purpose

This procedure tests a quick drag on a panel titlebar. The drag starts with no
pause after the press. The window system can then send the press and the
motion after it before Horizon paints a new frame. The panel must move, and
in an arranged cloud the panels must change places.

## 2. Applicability

- Use a Horizon candidate that contains this change.
- Platforms: Linux with X11, with the local device fixture.
- Task 6.3 needs a workspace with Rows layout and two or more panels.

## 3. Safety

> **CAUTION: USE ONLY THE ISOLATED DESKTOP.** A drag on the developer desktop can move a real panel.

## 4. Equipment and preconditions

- A frozen Horizon candidate and its SHA-256.
- The local device fixture with `--native-view`.
- A live Device panel in the current workspace.
- `xdotool` on the fixture display.

## 5. Setup

1. Start the fixture with the frozen candidate and `--native-view`.

   Result: The fixture prints a loopback VNC address and its X display.

2. Open that address in a Device panel.

   Result: The panel shows the isolated Horizon window.

3. Make sure that the running process is the candidate: compare the SHA-256
   of the executable of the process with the frozen candidate.

   Result: The two values are the same.

## 6. Tasks

One `xdotool` command does not make sure that Horizon gets its input in one
batch: Horizon can paint a frame between two of its requests. Thus each
quick drag uses a controlled stall:

1. Find the process ID of the candidate (the process whose executable has the
   candidate SHA-256). Use this exact ID only.
2. Move the pointer to the titlebar and wait 0.4 seconds.
3. Send `SIGSTOP` to the candidate. While it is stopped, send `mousedown 1`
   and 20 `mousemove_relative` steps with `xdotool`. Wait 0.3 seconds, then
   send `SIGCONT`. The X server keeps the events until Horizon reads them, so
   Horizon reads the press and all the motion together.
4. Wait 0.5 seconds, then send `mouseup 1`. Take the titlebar position from a
   fresh screenshot.

A frame-per-event drag is the control. Do not stop the candidate. Send
`mousedown 1`, then each of the 20 steps in its own `xdotool` command with
90 milliseconds between them, so that Horizon paints a frame between two
events.

A Horizon without this change does not move the panel for a quick drag. It
moves the panel for a frame-per-event drag. If you have such a build, do one
quick drag and one frame-per-event drag with it first, to make sure that the
stall gives one batch on your fixture.

### 6.1 DOWN: Quick vertical drag

1. Do a quick drag of 90 points down on the title text of a panel.

   Result: The panel moves down by the drag distance.

2. Do a quick drag of 70 points up on the blank right part of the titlebar.

   Result: The panel moves up by the drag distance.

### 6.2 ZOOM: Reduced canvas zoom

1. Zoom the canvas out to about 50 percent. Do the two drags of task 6.1
   again.

   Result: The panel moves each time. A drag is never lost.

### 6.3 ROWS: Arranged cloud

1. Select Rows for a workspace with two panels.

   Result: The panels show one above the other.

2. Do a quick drag from the blank part of the upper titlebar down to the
   lower panel.

   Result: A drop preview shows during the drag. After the release the two
   panels change places.

### 6.4 FRAMES: Frame-per-event control

1. Do a frame-per-event drag of 90 points down on the title text of a panel.

   Result: The panel moves down by the drag distance, minus at most one step.

2. In the Rows workspace of task 6.3, do a frame-per-event drag from the
   blank part of the upper titlebar down to the lower panel.

   Result: After the release the two panels change places.

### 6.5 CLICK: Plain click

1. Click a titlebar once, with no motion.

   Result: The panel gets focus. The panel does not move.

## 7. Pass criteria

- Every quick drag on a titlebar moves the panel, on the title text and on the
  blank part, at each zoom.
- A quick vertical drag in a Rows workspace changes the order of the panels.
- Each frame-per-event drag gives the same result as the quick drag.
- A click with no motion does not move a panel.

## 8. Cleanup

1. Close the Device panel. Stop the fixture processes that this run started.

   Result: The candidate process stops. The developer desktop does not change.

## 9. Record of results

Put the results in the pull request: for each drag, the delivery (quick or
frame-per-event), the start and end titlebar positions, and the candidate
SHA-256. Keep private evidence out of the repository.
