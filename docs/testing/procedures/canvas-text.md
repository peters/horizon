---
procedure: canvas-text
feature: Text of the canvas at every zoom, and the weights of the interface
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Canvas text test procedure

## 1. Purpose

This procedure proves that the text of the canvas is sharp at every canvas
zoom. It also proves that the interface uses the weights of the approved
mockups, and that a zoom gesture stays cheap.

## 2. Applicability

- Candidate: a build that has `zoom_text`, `typeface::Weight` and the hinted
  Inter and JetBrains Mono faces.
- Platforms: Linux, because the fixture needs an isolated X11 desktop. The
  screenshots of this procedure show the result on a display of scale 1 and on
  a display of scale 2.
- This procedure does not test: the Fit zoom rules. It uses the Fit zoom of the
  fixture and does not change it.

## 3. Safety

No step rents compute, deletes data, sends a secret or changes access. The
fixture keeps its state in its own folder and touches no real Horizon home.

## 4. Equipment and preconditions

- A Linux computer with the prerequisites of
  [the device smoke fixture](../../../scripts/device-smoke/README.md).
- A Horizon native VNC Device panel in the current workspace, for the live view.
- A card panel with a heading, a row title, a caption and a primary button on
  the board.

## 5. Setup

1. Start the fixture with the native view.

   `python3 scripts/device-smoke/serve.py --native-view`

   Result: the fixture prints a `vnc_address`.

2. Open the panel of the fixture in a Device panel.

   Result: `connection` says connected, `image_received` and `image_displayed`
   are true, and `frame_sequence` advances.

3. Open a workspace with one terminal panel and one card panel.

   Result: the canvas shows both panels at canvas zoom 1.0.

## 6. Tasks

### 6.1 CT-100 — The text at zoom 1.0

1. Take a screenshot of the card panel.

   Result: the text is sharp. The heading is heavier than its second line. The
   caption in upper case is the heaviest line of the card.

2. Zoom the canvas to 100 % with `Ctrl+0`.

   Result: the text does not change.

### 6.2 CT-067 — The text at a zoom of 0.67

1. Zoom the canvas out with the wheel until the window shows the zoom of about
   0.67.

   Result: the letters keep an even weight while the gesture runs.

2. Stop the wheel and wait one second.

   Result: the letters are sharp again. No letter is a stretched copy.

3. Take a screenshot.

   Result: the stems of one word have the same thickness. The heading is still
   heavier than the body text.

### 6.3 CT-094 — The text at the Fit zoom of a workspace

1. Press the Fit key of the workspace with three panels in one row.

   Result: the canvas settles at a zoom of about 0.69.

2. Take a screenshot of the title bar of a panel and of a terminal.

   Result: the title bar text and the terminal text are sharp. The cursor and
   the underlines of the terminal sit on their cells. A character of two cells
   keeps two cells.

### 6.4 CT-150 — The text at a zoom of 1.5

1. Zoom the canvas in with the wheel until the window shows the zoom of about
   1.5.

   Result: the letters are sharp. The text does not become a large block of
   colour.

2. Take a screenshot.

   Result: the heading of the card is heavier than its second line, and the row
   title is heavier than the body text.

### 6.5 CT-REPAINT — The repaint of a settled zoom

1. Watch the frame rate of the toolbar while the canvas is still.

   Result: the rate falls to the idle value of the build.

2. Move the wheel by one step and watch the frame rate.

   Result: the rate rises while the zoom moves, and falls back after about one
   second. The window does not repaint while it is still.

### 6.6 CT-LAYOUT — The layout of a wrapped paragraph

1. Zoom from 1.0 to 0.67 and back.

   Result: a wrapped paragraph of a card breaks at the same words at both
   zooms. No word moves to another line.

### 6.7 CT-SCALE2 — A display of scale 2

1. Repeat CT-094 on a display of scale 2.

   Result: the text is sharp there too.

## 7. Pass criteria

- The text of the card, the title bar, the workspace label and the terminal is
  sharp at the zooms 0.67, 0.94, 1.0 and 1.5.
- The weights of section 6.1 and 6.4 hold at every zoom.
- A wrapped paragraph breaks at the same words at every zoom.
- The window repaints once after a zoom settles, and not while it is still.

## 8. Cleanup

1. Close the Device panel of the fixture.

   Result: the connection is released. The fixture keeps running.

2. Stop the fixture with `Ctrl+C` in its terminal.

   Result: the fixture removes its own state.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
