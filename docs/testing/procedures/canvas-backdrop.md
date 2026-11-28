---
procedure: canvas-backdrop
feature: The backdrop of the canvas
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Canvas backdrop test procedure

## 1. Purpose

This procedure proves that Horizon draws the canvas backdrop of the design
"Dusk": a sky that becomes lighter toward a horizon line low on the canvas, a
soft glow on that line, a darker ground under it and the dot grid over it. It
proves the two themes, the place of the horizon at a share of the canvas, the
backdrop that does not move with the canvas, the edges that cut it, a detached
workspace window and the first start page.

## 2. Applicability

- Candidate: a build that has `crates/horizon-ui/src/app/canvas_backdrop.rs` and
  the tokens `CANVAS_SKY_TOP`, `CANVAS_SKY_MID`, `CANVAS_SKY_HORIZON`,
  `CANVAS_GROUND`, `CANVAS_GLOW_WARM`, `CANVAS_GLOW_COOL` and
  `CANVAS_HORIZON_LINE` in `theme.rs`.
- Platforms: Linux, because the fixture needs an isolated X11 desktop.
- This procedure does not test: the colours of the panels, the workspace frame or
  the agent lines.
- This procedure does not test: the startup chooser and the loading view, which
  keep a flat fill and show no canvas.

## 3. Safety

No step rents compute, deletes data, sends a secret or changes access. The
fixture keeps its state in its own folder and touches no real Horizon home.

## 4. Equipment and preconditions

- A Linux computer with the prerequisites of
  [the device smoke fixture](../../../scripts/device-smoke/README.md).
- A Horizon native VNC Device panel in the current workspace, for the live view.
- One workspace with two panels, so that the canvas has content.
- The appearance setting **Theme** at `dark` for the tasks of section 6.1, and
  `light` for the tasks of section 6.2.

## 5. Setup

1. Start the fixture with the native view.

   `python3 scripts/device-smoke/serve.py --native-view`

   Result: the fixture prints a `vnc_address`.

2. Open the panel of the fixture in a Device panel.

   Result: `connection` says connected, `image_received` and `image_displayed`
   are true, and `frame_sequence` advances.

3. Create one workspace with two panels.

   Result: the canvas shows both panels on the backdrop.

## 6. Tasks

### 6.1 CB-1 — The night backdrop

1. Set **Theme** to `dark`.

   Result: the canvas shows a dark sky. The top of the canvas is almost black,
   it becomes a little lighter toward the horizon, the horizon line is a thin
   warm line, the glow around that line is violet at its sides and warm at its
   middle, and the ground under the line is dark again.

2. Look at the dots of the canvas.

   Result: the dots are a pale blue at low alpha. They are 22 points apart and
   each dot is 2.3 points wide at 100 percent of the zoom.

3. Look for an edge of the glow.

   Result: the glow has no edge and no ring. Its colour changes with no step
   that the eye can see.

### 6.2 CB-2 — The day backdrop

1. Set **Theme** to `light`.

   Result: the canvas shows a warm off-white sky that becomes a little warmer
   toward the horizon, a glow that is warm at its middle and violet at its
   sides, a thin warm horizon line, and a slightly darker warm ground under the
   line.

2. Look at the dots of the canvas.

   Result: the dots are a brown colour at low alpha, with the same spacing and
   the same width as in section 6.1.

### 6.3 CB-3 — The backdrop does not move with the canvas

1. Hold the middle mouse button on an empty place of the canvas and move the
   pointer to the left and up. Release the button.

   Result: the panels and the dots move with the canvas. The sky, the glow and
   the horizon line do not move.

2. Press the zoom out shortcut until the zoom is 50 percent, and then zoom in
   again.

   Result: the dots take a wider spacing and a smaller width, and then the
   spacing that they had. The horizon line and the glow keep their place.

### 6.4 CB-4 — A resize keeps the horizon at its share of the canvas

1. Measure the height of the canvas, from the foot of the toolbar to the foot of
   the window. Measure the height of the horizon line over that foot.

   Result: the horizon line stands at 72 percent of the height of the canvas,
   with a tolerance of 3 points.

2. Make the window 200 points higher and measure again.

   Result: the horizon line stands at 72 percent of the new height, with the same
   tolerance. The top of the canvas keeps the same colour and the foot of the
   canvas keeps the same colour.

### 6.5 CB-5 — The dock and the Settings panel cut the backdrop

1. Open the dock of the orchestrator.

   Result: the canvas ends at the left edge of the dock. No part of the sky, the
   glow or the horizon line shows under the dock.

2. Open the Settings panel and keep the dock open.

   Result: the canvas ends at the left edge of the Settings panel. The horizon
   line ends at that edge and it does not continue under the panel.

3. Open the Server tab of the Settings panel, and then every other tab.

   Result: the canvas ends at the same edge on every tab.

4. Make the window narrower until the canvas is empty, and then make it wide
   again.

   Result: the backdrop does not run backwards and it does not show under the
   panels. The canvas returns with its horizon at the same share of its height.

### 6.6 CB-6 — A detached workspace window

1. Detach the workspace to its own window.

   Result: the detached window shows the sky, the glow, the horizon line and the
   dots below its own toolbar. The horizon line stands at 72 percent of the height
   of the canvas of that window, with a tolerance of 3 points.

2. Resize the detached window.

   Result: the horizon line keeps its share of the new height.

3. Attach the workspace to the main window again.

   Result: the backdrop of the main window is unchanged.

### 6.7 CB-7 — The first start page

1. Remove the saved session of the fixture home, and then start the fixture
   again.

   Result: the first start page shows the sky, the glow, the horizon line and the
   dots behind its block. The backdrop starts under the toolbar, and the page
   block stands on it.

2. Resize the window.

   Result: the horizon line keeps its share of the height of the canvas, and the
   block of the page keeps its place in the canvas.

## 7. Pass criteria

- Both themes show the sky, the glow, the horizon line and the dots of sections
  6.1 and 6.2.
- The horizon line stands at 72 percent of the canvas height, with a tolerance
  of 3 points, in the root window, in a detached window and behind the first
  start page, at every window size.
- A pan and a zoom move the dots and no part of the backdrop.
- The dock, the Settings panel, the sidebar and the settings bar cut the backdrop
  at their own edge.
- The glow has no visible edge and the sky has no step that the eye can see.

## 8. Cleanup

1. Close the Device panel of the fixture.

   Result: the panel closes and the connection is released.

2. Stop the fixture in its terminal window.

   Result: the fixture ends and the isolated desktop is gone.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
