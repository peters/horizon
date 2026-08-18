# Smoke plan — layout switch from Default must keep original panel positions

Branch: `fix/layout-switch-keeps-panels` (worktree `.worktrees/fix-layout-switch-keeps-panels`)

**Status: executed on 2026-08-18, all steps passed** (evidence below).

## Behavior under test

Switching a workspace's layout **from Default (manual placement) to any preset
(Rows / Cols / Grid)** must record the preset **without moving or resizing the
existing panels**. The preset takes effect on the next reflow (panel added,
closed, or resized). Switching **between** two presets still re-arranges.
Clicking "Default" (clear) still preserves positions (pre-existing behavior).

Covered deterministically by unit tests in
`crates/horizon-core/src/board/tests/layout.rs`:

- `switching_from_manual_to_preset_keeps_panel_positions` (all 3 presets)
- `preset_switched_from_manual_applies_on_next_reflow` (panel close)
- `preset_switched_from_manual_applies_on_panel_add` (panel add — the UI-driven reflow)
- `switching_between_presets_still_rearranges` (exact Rows slots + layout)

This live plan verifies the same flow through the real UI (toolbar buttons +
keyboard + panel drag) on a fresh isolated session.

## Prerequisites

- Linux, X11 session (here: `DISPLAY=:1`), `xdotool`, `xwininfo`, `scrot`
- Worktree build: `cargo build` (debug is fine)
- Smoke-home `config.yaml` must contain a plain shell preset as the first
  preset, e.g.:

  ```yaml
  version: 9
  presets:
  - name: Shell
    alias: sh
    kind: shell
    command: null
    args: []
    resume: fresh
  ```

  **Why:** `Ctrl+Shift+N` creates a panel from the *first preset*, and every
  non-SSH preset requires the workspace cwd. On a fresh workspace (no cwd) the
  first `Ctrl+Shift+N` opens the dir-picker modal instead of spawning a panel.
  Press `Enter` in the picker to confirm the seeded directory — after that the
  workspace has a cwd and further `Ctrl+Shift+N` spawns shell panels directly.

## Setup — isolated session

```bash
SMOKE_HOME=/tmp/horizon-layout-smoke-home
rm -rf "$SMOKE_HOME" && mkdir -p "$SMOKE_HOME/.horizon"
cat > "$SMOKE_HOME/.horizon/config.yaml" <<'EOF'
version: 9
presets:
- name: Shell
  alias: sh
  kind: shell
  command: null
  args: []
  resume: fresh
EOF
cd <worktree>
DISPLAY=:1 HOME="$SMOKE_HOME" ./target/debug/horizon >/tmp/horizon-layout-smoke.log 2>&1 &
HORIZON_PID=$!
sleep 10
# Find the smoke window (fresh 1600x1000, e.g. 0x4a00004). Scope every
# xdotool call to this window id; the user's own Horizon may be running.
```

- Read state from `$SMOKE_HOME/.horizon/sessions/<id>/runtime.yaml`
  (auto-saved shortly after each change; wait ~2–3 s after each action).
- Screen mapping (verified in the run):
  `screen = window_client_origin + (sidebar_w, 46) + pan_offset + canvas_point * zoom`
  - sidebar_w = 210 at a 1600px viewport (`effective_sidebar_width`),
    46 = TOOLBAR_HEIGHT
  - `pan_offset` / `zoom` from `canvas_view` in runtime.yaml
  - workspace frame top-left = `(min_panel_x - 16, min_panel_y - 16 - 38)`
    where min is over ALL panel positions (the panel that ends up on top after
    manual moves may not be the one you dragged!)
  - label rect = frame top-left + (14, 12), height 30, width =
    `clamp(estimate(name) + 60, 110, 260)` (uppercase 8.6 / whitespace 4.5 /
    other 7.6 per char)
  - toolbar min = (label.max.x + 10, label.min.y); inner margin (6, 5)
  - "Default" button center = toolbar min + (6 + 30, 5 + 12)
  - "Grid" button center = toolbar min + (6 + 60 + 4 + 44 + 4 + 44 + 4 + 22, 5 + 12)
  - The toolbar is NOT rendered when its screen rect intersects an overlay
    zone (minimap/attention feed) — pan the canvas if clicks miss.
  - Drag a panel by its title bar (canvas point `pos + (size.x/2, 8)`),
    ~10 xdotool steps, 80 ms apart.

## Steps (as executed)

| # | Action | Observed in runtime.yaml | Result |
|---|--------|--------------------------|--------|
| 1 | Fresh launch; `Ctrl+Shift+N` + `Enter` (dir picker), then `Ctrl+Shift+N` ×2 | 3 panels in Grid: (20,60) (560,60) (20,420) | pass |
| 2 | Click **Default** (toolbar) | layout → `null`, all positions unchanged | pass |
| 3 | Drag P0 title bar by (+420, +260) | P0 → (440,320), layout stays `null` | pass |
| 4 | Click **Grid** (the regression) | layout → `Grid`, **P0 stays (440,320), P1/P2 unchanged, all sizes unchanged** | pass — old code snapped every panel to a grid at the workspace origin |
| 5 | Click **Default** again | layout → `null`, positions unchanged (round-trip clean) | pass |
| 6 | `Ctrl+Shift+N` (manual mode) | new panel at first free tile (20,60); originals untouched | pass |
| 7 | Click **Grid** (arms from manual), then `Ctrl+Shift+N` | layout `Grid`; reflow arranged all 5 panels in a 3-col grid at the origin | pass — preset applies on next reflow |

`scrot /tmp/horizon-layout-smoke-final.png` captured after step 7 (kept in
`/tmp`, not in the repo). No errors/panics in the smoke log.

## Pass criteria

- Steps 2, 4, 5: panel positions byte-identical (±1 px) across the switches.
- Step 4: `layout: Grid` persisted.
- Step 7: reflow re-arranges as before.
- No crashes in the smoke log.

## Cleanup (done)

```bash
kill $HORIZON_PID
rm -rf "$SMOKE_HOME" /tmp/horizon-layout-smoke-*.png /tmp/horizon-layout-smoke.log
```
