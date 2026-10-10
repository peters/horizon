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

Do each drag with `xdotool` on the fixture display: `mousemove` to the
titlebar, `mousedown 1`, then 20 `mousemove_relative` steps with no pause
before the first step, then `mouseup 1`. Take the titlebar position from a
fresh screenshot.

### 6.1 DOWN — Quick vertical drag

1. Do a quick drag of 90 points down on the title text of a panel.

   Result: The panel moves down by the drag distance.

2. Do a quick drag of 70 points up on the blank right part of the titlebar.

   Result: The panel moves up by the drag distance.

### 6.2 ZOOM — Reduced canvas zoom

1. Zoom the canvas out to about 50 percent. Do the two drags of task 6.1
   again.

   Result: The panel moves each time. A drag is never lost.

### 6.3 ROWS — Arranged cloud

1. Select Rows for a workspace with two panels.

   Result: The panels show one above the other.

2. Do a quick drag from the blank part of the upper titlebar down to the
   lower panel.

   Result: A drop preview shows during the drag. After the release the two
   panels change places.

### 6.4 CLICK — Plain click

1. Click a titlebar once, with no motion.

   Result: The panel gets focus. The panel does not move.

## 7. Pass criteria

- Every quick drag on a titlebar moves the panel, on the title text and on the
  blank part, at each zoom.
- A quick vertical drag in a Rows workspace changes the order of the panels.
- A click with no motion does not move a panel.

## 8. Cleanup

1. Close the Device panel. Stop the fixture processes that this run started.

   Result: The candidate process stops. The developer desktop does not change.

## 9. Record of results

Put the results in the pull request. Keep private evidence out of the repository.
