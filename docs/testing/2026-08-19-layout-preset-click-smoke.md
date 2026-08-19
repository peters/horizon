# Smoke Plan: Layout preset click arranges panels immediately

**Change under test:** `Board::arrange_workspace` no longer defers a preset
selected from manual placement. Clicking **Rows**, **Cols**, or **Grid** in the
workspace toolbar (or context menu) must re-arrange the panels immediately,
sized to fit the current content area. Clicking **Default** must still keep
current positions, and dragging a panel must still return the workspace to
manual placement.

**Build:** `cargo build` (debug) in the PR worktree; run `target/debug/horizon`.

**Isolation:** run with `--config <plan-yaml> --ephemeral` so the session store
is never touched. Example board (workspace in manual placement, no preset):

```yaml
version: 9
workspaces:
  - name: manual
    position: [40.0, 60.0]
    terminals:
      - name: alpha
        kind: editor
        position: [180.0, 260.0]
        size: [420.0, 280.0]
      - name: beta
        kind: editor
        position: [680.0, 140.0]
        size: [420.0, 280.0]
      - name: gamma
        kind: editor
        position: [420.0, 560.0]
        size: [420.0, 280.0]
```

Panels loaded with explicit positions put the workspace in manual placement,
so the toolbar shows **Default** selected at launch.

## Steps

| # | Action | Expected |
|---|--------|----------|
| 1 | Launch; screenshot baseline | 3 panels scattered as configured; toolbar shows Default selected |
| 2 | Click **Grid** (the regression) | Panels immediately snap into a 2-column grid anchored at the workspace frame's top-left; Grid becomes selected |
| 3 | Click **Rows** | Panels immediately re-arrange into a single column of rows |
| 4 | Click **Cols** | Panels immediately re-arrange into a single row of columns |
| 5 | Click **Default** | Layout selection clears; panel positions do not change |
| 6 | Drag one panel by its titlebar, then click **Grid** | Workspace returns to manual on drag; the Grid click again arranges immediately |
| 7 | Add a panel while a preset is active | New panel joins the arrangement (reflow), no overlap |

## Visual checks

- No panel overlap after any preset click.
- Arrangement stays inside the existing workspace frame area (no jump across
  the canvas).
- Repeat steps 2 to 4 twice; arrangement must be stable (no oscillation).

## Lanes

- Linux desktop (real display): primary lane, **pending**. Requires a human or
  an agent on a real display; see the headless limitation below.
- macOS: same steps; only needed if reviewers want platform confirmation, the
  logic under test is platform-independent board math.

## Headless limitation (verified 2026-08-19)

The Xvfb + xdotool lane cannot execute the click steps: XTEST synthetic
pointer events are generated at the X server (confirmed with
`xinput test-xi2 --root`) but are never delivered to the winit 0.30 window
(confirmed with `xev -id <win>` observing zero pointer events during moves and
clicks, with and without a window manager). Keyboard events do arrive
(`xdotool key --window <win> ctrl+shift+m` toggles the minimap). Additionally,
plain `xdotool mousemove` warps the pointer without generating XI2 motion
events; only `mousemove_relative` produces real motion. Launch, board
construction from config, and visual baseline were verified headlessly; the
click flow needs a real display.
