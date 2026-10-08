---
procedure: panel-resize-grip
feature: Panel and cloud resize grip
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Panel and cloud resize grip test procedure

## 1. Purpose

This procedure tests the six dots on the resize handle of a panel and of a
cloud. The dots make a triangle that points into the corner. The panel handle
stays 32 screen points at each canvas zoom.

## 2. Applicability

- Use a Horizon candidate that contains this change.
- Platforms: Linux, with the local device fixture.
- A cloud card is necessary for task 6.4. A cloud that is not deployed is
  satisfactory. It rents no compute.

## 3. Safety

> **CAUTION: USE ONLY THE ISOLATED DESKTOP.** A drag on the developer desktop can move a real panel.

## 4. Equipment and preconditions

- A frozen Horizon candidate and its SHA-256.
- The local device fixture with `--native-view`.
- A live Device panel in the current workspace.

## 5. Setup

1. Start the fixture with the frozen candidate and `--native-view`.

   Result: The fixture prints a loopback VNC address.

2. Open that address in a Device panel.

   Result: The panel shows the isolated Horizon window.

## 6. Tasks

### 6.1 DOTS — Corner mark

1. Find the bottom-right corner of a panel.

   Result: You see six small dots in a triangle: three on the bottom row, two
   above them and one at the top, on the right edge. You do not see a filled
   square.

2. Move the pointer onto the corner.

   Result: The dots change to the accent color on a soft accent backing. The
   pointer shows the resize cursor.

3. Drag the corner down and to the right. Then drag it up and to the left.

   Result: The panel grows, then becomes smaller. The dots stay in the corner.

### 6.2 ZOOM — Canvas zoom

1. Zoom the canvas out. Then zoom the canvas in.

   Result: The dots keep about the same size on the screen. This stays true when the handle is smaller than 32 screen points and the corner can hold the dots.

### 6.3 BODY — Content click

1. Click the panel body, away from the dots.

   Result: The click goes to the panel content. The panel size does not change.

### 6.4 CLOUD — Cloud corner

1. Find the bottom-right corner of a cloud card.

   Result: You see the same triangle of six dots as on a panel. The dots are
   clearly visible on the dark canvas.

2. Move the pointer onto the corner.

   Result: The dots change to the accent color on a soft accent backing. The
   pointer shows the resize cursor and the hover text is
   `Drag to resize this cloud.`

3. Drag the corner down and to the right.

   Result: The cloud frame grows. The dots stay in the corner.

4. Zoom the canvas out. Then zoom the canvas in.

   Result: The dots keep about the same size on the screen.

## 7. Pass criteria

- The corner shows six dots in a triangle and no filled square, on a panel
  and on a cloud.
- The dots change to the accent color on a soft backing while the pointer is
  on them.
- A drag on the dots changes the panel size.
- A click away from the dots does not change the panel size.
- The dot size on screen stays about the same after a zoom.

## 8. Cleanup

1. Close the Device panel. Stop the fixture processes that this run started.

   Result: The candidate process stops. The developer desktop does not change.

## 9. Record of results

Put the results in the pull request. Keep private evidence out of the repository.
