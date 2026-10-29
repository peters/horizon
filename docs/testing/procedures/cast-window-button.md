---
procedure: cast-window-button
feature: The whole-window Cast button in the root toolbar
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Cast the whole window from the root toolbar test procedure

## 1. Purpose

This procedure makes sure that a person can start and stop a cast of the whole
Horizon window from the root toolbar, without first opening the picker of a
panel, a workspace or a cloud card.

## 2. Applicability

- Candidate: each candidate that changes the root toolbar, the Cast picker or
  the application cast source.
- Platforms: Linux, with the local device fixture and one Apple TV on the same
  network.
- This procedure does not test: panel, workspace and cloud casts, which
  [the workspace procedure](cast-workspace-clouds.md) covers; pairing; encoder
  selection; the MCP and CLI operations, which the unit tests cover.

## 3. Safety

> **CAUTION:** USE ONLY THE ISOLATED DESKTOP. The whole-window source shows
> every workspace that is on screen. Do not show a key, a token or a private
> terminal in the window before you start the cast.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The [local device smoke fixture](../../../scripts/device-smoke/README.md) with
  `--native-view`.
- A Device panel that shows a live view of the fixture.
- One Apple TV that the candidate can pair with. Steps that select a receiver
  need it.

## 5. Setup

1. Start the fixture with the frozen candidate and open its address in a Device
   panel.

   Result: The panel shows the isolated Horizon window.

2. Move the pointer away from the toolbar.

   Result: The window makes no motion.

## 6. Tasks

### 6.1 BTN — The button is in the root toolbar

1. Look at the right side of the root toolbar, between the New cloud button and
   **Menu**.

   Result: A cast icon is present. The icon is the same glyph that a panel shows.

2. Move the pointer over the icon.

   Result: The icon shows the accent color. A hint says that the cast shares the
   whole window.

### 6.2 OPEN — A first click without a known TV

Do this task when the candidate has no remembered whole-window TV.

1. Click the cast icon.

   Result: The Cast picker opens below the icon. It stays inside the window.

2. Read the source in the picker.

   Result: The source is **Entire Horizon**. The picker shows the hint of the
   whole-window source and the **Allow this workspace's agents** option.

3. Read the icon.

   Result: The icon stays in the accent color while the picker is open.

4. Click the cast icon again.

   Result: The picker closes. The icon loses the accent color.

### 6.3 START — One click on the remembered TV

1. Select the Apple TV in the picker and start one whole-window cast. Stop it.

   Result: The candidate now remembers this TV for the whole window.

2. Click the cast icon.

   Result: The cast starts on the same TV. The picker does not open.

3. Look at the TV.

   Result: The TV shows the whole window with the active workspace, its panels,
   the sidebar and the toolbar. The cast icon stays in the accent color.

4. Compare the TV picture with the window.

   Result: The picture has the same layout. A second workspace is not in the
   picture, because the window does not show it.

5. Click the cast icon.

   Result: The cast stops. The cast icon loses the accent color.

### 6.4 CHOOSER — Shift-click opens the picker

1. Hold **Shift** and click the cast icon.

   Result: The picker opens. No cast starts.

2. Right-click the cast icon.

   Result: The picker closes.

3. Click the cast icon of a panel.

   Result: The picker opens next to that panel. The root toolbar icon does not
   change its state.

4. Click the cast icon in the root toolbar.

   Result: The picker closes.

### 6.5 EDGE — A narrow window

1. Resize the window until its width is about 900 pixels.

   Result: The search field stays at its minimum width. The cast icon stays left
   of **Menu**, and **Menu** stays the last control.

2. Click the cast icon.

   Result: The picker opens inside the window.

3. Click the cast icon again.

   Result: The picker closes.

## 7. Pass criteria

- The cast icon is in the root toolbar on Linux, and no other platform shows it.
- A first click without a known TV opens the picker under the icon with the
  whole-window source, and the same click closes it.
- A click with a remembered TV starts the cast without a picker, and a click
  during that cast stops it.
- Shift-click and right-click open the picker.
- A whole-window session shows the window as it appears, and the icon shows the
  session state.
- A picker that covers the window pauses the picture and does not stop the
  session.
- No step changes another workspace, panel or cloud.

## 8. Cleanup

1. Stop each cast.
2. Close the window with the close button of the window manager.

   Result: The candidate and the harness exit.

3. Close the Device panel. Delete the state path of the fixture.

## 9. Record of results

Put the results in the pull request. Keep private evidence out of the
repository.
