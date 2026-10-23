---
procedure: terminal-scroll-to-bottom
feature: Terminal scrollbar and panel corner grip
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Terminal scrollbar to the bottom test procedure

## 1. Purpose

This procedure proves that the scrollbar of a terminal panel selects the
newest output at the foot of its track. It also proves that the corner resize
grip keeps its own area.

## 2. Applicability

- Candidate: a Horizon build that contains the scrollbar clearance change.
- Platforms: Linux, with the local device fixture.
- This procedure does not test: the wheel over the terminal body, the canvas
  scroll chain, or the cloud corner grip.

## 3. Safety

> **CAUTION: USE ONLY THE ISOLATED DESKTOP.** A drag on the developer desktop
> can move a real panel.

## 4. Equipment and preconditions

- A frozen Horizon candidate and its SHA-256.
- The local device fixture with `--native-view`.
- A live Device panel in the current workspace.
- The `horizon-device` binary of the same candidate, to drive the isolated
  desktop.

## 5. Setup

1. Start the fixture with the frozen candidate and `--native-view`.

   Result: The fixture prints a loopback VNC address.

2. Open that address in a Device panel.

   Result: The panel shows the isolated Horizon window.

3. In the input terminal of the isolated Horizon, run `seq 1 200`.

   Result: The terminal shows the newest lines and its header shows history.

## 6. Tasks

### 6.1 TRACK — Foot of the track

1. Click the top area of the scrollbar track.

   Result: The terminal shows older lines. The thumb moves near the top.

2. Click the lowest point of the track, just above the corner dots.

   Result: The terminal shows the newest lines again. The prompt is at the
   bottom. The panel size does not change.

### 6.2 THUMB — Lowest part of the thumb

1. Put the pointer on the lowest part of the thumb. Drag the pointer to the top
   of the track.

   Result: The terminal scrolls up. The panel size does not change. The dots do
   not change color.

2. Drag the thumb back to the lowest point of the track.

   Result: The terminal shows the newest lines. The panel size does not change.

### 6.3 GRIP — Corner resize

1. Drag the corner dots up and to the left.

   Result: The panel becomes smaller. The hover text is
   `Drag to resize panel`. The terminal does not scroll.

### 6.4 ZOOM — Canvas zoom

1. Zoom the canvas out until the panel is about half its size. Then zoom the
   canvas in again.

   Result: At each zoom, the track ends above the corner dots. The dots keep
   about the same size on the screen.

2. Repeat task 6.1 and task 6.3 at the new zoom.

   Result: The behavior is the same as at 100 percent.

### 6.5 SHORT — Short panel

1. Make the panel short, with the corner drag, until about eight text lines
   remain.

   Result: The track is still visible and is longer than 20 screen points.

2. Drag the thumb to the top of the track, then click the lowest point of the
   track.

   Result: The terminal shows the newest lines.

## 7. Pass criteria

- A click on the lowest point of the track shows the newest output.
- A drag that starts on the lowest part of the thumb scrolls the terminal.
- A drag on the corner dots resizes the panel and does not scroll the terminal.
- The track stops above the corner dots at each canvas zoom.
- The panel size does not change during a scrollbar action.

## 8. Cleanup

1. Close the Device panel. Stop the fixture processes that this run started.

   Result: The candidate process stops. The developer desktop does not change.

## 9. Record of results

Put the results in the pull request. Keep private evidence out of the
repository.
