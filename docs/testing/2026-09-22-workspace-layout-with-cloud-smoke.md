# Workspace layout with an attached cloud

Smoke plan for the workspace Default / Rows / Cols / Grid controls when a
cloud panel is attached. The workspace arrangement and the cloud's own panel
layout are independent, and a workspace preset must not cover or drop the
cloud.

## Safety contract

- Use a task-owned debug build. Do not replace the user's running Horizon
  process.
- Open a copy of the session, or launch `--config <board>.yaml --ephemeral`,
  so the live session store is not overwritten.
- A real cloud is optional. A design-fixture or already-attached cloud named
  in the session is enough. Do not provision a new worker unless the lane
  says so.

## Board

One workspace containing at least two ordinary panels (shell, editor, or
agent) and one cloud panel whose body sits inside the workspace frame. Note
the cloud title, its canvas position, and which workspace preset is
highlighted before starting.

## Lanes

1. Baseline. With the cloud attached, the workspace header shows Default,
   Rows, Cols, Grid, and Detach. Detach stays disabled. The cloud's own
   Panel layout control is still present on the cloud card.
2. Grid. Click Grid. The cloud stays fully visible, including its runtime
   card. Ordinary panels move beside the cloud and do not cover it. Grid is
   highlighted. The cloud's internal panel layout is unchanged.
3. Rows and Cols. Click each. Same expectations as Grid: the cloud does not
   move or disappear, and only the ordinary panels rearrange.
4. Default. Click Default. Ordinary panels stay where the last preset put
   them. The cloud stays visible. Default is highlighted.
5. Cloud layout. On the cloud card, choose a different Panel layout from the
   workspace preset. Panels inside the cloud follow the cloud. The workspace
   preset highlight does not change, and ordinary panels do not jump.
6. Drag the cloud over an arranged ordinary panel, then release. The cloud
   stays where it was dropped. The ordinary panel moves aside so it no longer
   covers the cloud. The workspace preset stays selected.
7. Drag the workspace by its header. The cloud moves with the workspace.
   Neighboring workspaces are not pushed by a frame that still includes the
   cloud's old position.
8. Persistence. Restart the ephemeral or copied session from its saved
   runtime. The workspace preset, ordinary panel positions, and cloud
   position match what was on screen. The cloud is still attached to the
   same workspace.
9. Visual regression. At 100% zoom and after Fit, the workspace frame
   encloses both the ordinary panels and the cloud. No cloud chrome is
   clipped by an overlapping panel. Check a narrow window where the cloud
   is partly off the canvas: Grid must not scroll the cloud out of existence.

## Record

Run on 2026-09-22 against commit `17ca4fd0af822ea884894a3eb4b5d22dc306fc9a`.
OS: Linux 6.18.7-76061807-generic x86_64. Cloud: fixture `Smoke cloud`
(`LocalPrototype`, issue 7), not a live worker. Debug binary sha256
`79ef1422fa0716f48ec90a996b3c667cc61c9deaae51f9fc34d0a14d3324e12a`, checked
against the running child rather than the sandbox launcher. The user's Horizon
session was not opened.

Evidence is under
`docs/testing/evidence/2026-09-22-workspace-layout-with-cloud/`. The two
videos are X images of the isolated display with a crosshair drawn at the
real pointer each frame. `ffmpeg` x11grab produced no packets against this
Xvfb.

| Lane | Result | What was observed |
| --- | --- | --- |
| 1 Baseline | PASS, with one gap | Header shows Default, Rows, Cols, Grid, and Detach while the cloud is attached (`00-launch.png`, zoom 1). Clicking Detach left a single Horizon window and did not change the board. This fixture has no runtime card, so the cloud's Panel layout control is absent. See lane 5. |
| 2 Grid | PASS | Grid stays highlighted. Notes and Scratch sit beside the cloud. The cloud, including Inside, stays visible. Cloud layout stays Rows. `02-grid.png`, `02-grid-header.png`. |
| 3 Rows and Cols | PASS | Rows then Columns rearrange only the ordinary panels. Cloud position stayed `[700, 480]`, size `[548, 598]`, layout Rows. `03-rows.png`, `03-cols.png`, and the header crops. |
| 4 Default | PASS | Default is highlighted and the saved layout is unset. Ordinary panels stay at the Columns positions. The cloud stays put. `04-default.png`, `04-default-header.png`. |
| 5 Cloud layout | BLOCKED | No Panel layout control is drawn for this local prototype. The cloud context menu is Rename, Collapse, and Remove empty cloud. The cloud's own layout stayed Rows through every workspace preset. |
| 6 Drag the cloud | PASS | With Grid selected, dragging the cloud header dropped it at `[145.9, -45.4]`. Notes and Scratch moved up to `y = -465.4` (their bottoms at `-115.4`, above the cloud). Grid stayed selected. Cloud layout stayed Rows. Neighbor stayed at `[1584, 40]`. `06-drag-cloud.mp4`, `06-after.jpg`. |
| 7 Drag the workspace | PASS | Dragging the Smoke desk header moved the workspace from `[0, 40]` to `[338.8, 40]`. The cloud moved by the same `+338.8` on x, and its remembered workspace origin moved once. Neighbor stayed at `[1584, 40]`. Grid stayed selected. `07-drag-workspace.mp4`, `07-after.jpg`. |
| 8 Persistence | PASS | A new isolated process restored that saved runtime. Layout Grid, workspace `[338.8, 40]`, cloud `[484.7, -45.4]` layout Rows on `ws-smoke`, Inside inside the cloud, Neighbor `[1584, 40]`. `08-restart.png`. |
| 9 Fit and resize | PASS | Fit encloses the panels and the cloud (`09-fit.png`, zoom about 0.59). At 900×700 the cloud is partly past the window edge and still attached; clicking Grid does not remove it (`09-narrow.png`). Widening back to 1480×900 shows the cloud and both ordinary panels (`09-wide.png`). |
