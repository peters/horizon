---
procedure: panel-resize-grip
feature: Panel resize grip
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Panel resize grip test procedure

## 1. Purpose

This procedure tests the six dots on the panel resize handle.
The handle stays 32 screen points at each canvas zoom.

## 2. Applicability

- Use a Horizon candidate that contains this change.
- Platforms: Linux, with the local device fixture.
- This procedure does not test the cloud panel resize handle.

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

   Result: You see six small dots. You do not see a filled square.

2. Move the pointer onto the corner.

   Result: The dots change to the accent color. The pointer shows the resize cursor.

3. Drag the corner down and to the right. Then drag it up and to the left.

   Result: The panel grows, then becomes smaller. The dots stay in the corner.

### 6.2 ZOOM — Canvas zoom

1. Zoom the canvas out. Then zoom the canvas in.

   Result: The dots keep about the same size on the screen.

### 6.3 BODY — Content click

1. Click the panel body, away from the dots.

   Result: The click goes to the panel content. The panel size does not change.

## 7. Pass criteria

- The corner shows six dots and no filled square.
- A drag on the dots changes the panel size.
- A click away from the dots does not change the panel size.
- The dot size on screen stays about the same after a zoom.

## 8. Cleanup

1. Close the Device panel. Stop the fixture processes that this run started.

   Result: The candidate process stops. The developer desktop does not change.

## 9. Record of results

Put the results in the pull request. Keep private evidence out of the repository.
