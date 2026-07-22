# macOS Minimap Navigator Smoke Test

## Goal

Validate minimap focus indicators, hover feedback, hit precedence, click and
double-click navigation, drag panning, detached-window behavior, persistence,
and pointer-frame performance on macOS/Metal.

Run every step against the exact PR head that will be pushed and merged. Scope
native automation, screenshots, logs, and window inspection to the Horizon PID
started for this test.

## Required Environment

- macOS with Metal rendering and Retina scaling
- Rust stable 1.88 or newer
- Xcode Command Line Tools
- Accessibility permission for any native pointer automation
- `gh`, `screencapture`, and standard macOS process tools

## Build And Static Checks

From the PR worktree:

```bash
cargo build --locked -p horizon-ui --bin horizon
cargo test --locked -p horizon-ui --bin horizon app::minimap
cargo fmt --all -- --check
./scripts/check-maintainability.sh
RUSTFLAGS="-D warnings" cargo test --workspace
cargo clippy --all-targets --all-features -- -D warnings
cargo clippy --workspace --lib --bins --examples -- \
  -D warnings -D clippy::unwrap_used -D clippy::expect_used
cargo clippy --workspace --all-targets --all-features -- \
  -D warnings -W clippy::pedantic
```

Record the exact commit:

```bash
git rev-parse HEAD
```

## Isolated Runtime

Create a disposable root outside the repository:

```bash
export SMOKE_ROOT="$(mktemp -d /tmp/horizon-minimap-smoke.XXXXXX)"
mkdir -p "$SMOKE_ROOT/home"
```

Create `$SMOKE_ROOT/config.yaml`:

```yaml
version: 8
window:
  width: 1440
  height: 920
appearance:
  theme: dark
overlays:
  minimap_height: 260
  minimap_width: 420
workspaces:
  - name: Alpha Build
    cwd: /tmp
    terminals:
      - name: Alpha Shell
        kind: shell
        position: [60.0, 80.0]
        size: [620.0, 420.0]
      - name: Alpha Overlay
        kind: shell
        position: [360.0, 250.0]
        size: [560.0, 380.0]
  - name: Beta Services
    cwd: /tmp
    terminals:
      - name: Beta Shell
        kind: shell
        position: [1160.0, 120.0]
        size: [620.0, 420.0]
      - name: Beta Overlay
        kind: shell
        position: [1430.0, 280.0]
        size: [560.0, 380.0]
  - name: Empty Notes
    cwd: /tmp
    position: [2300.0, 180.0]
    terminals: []
```

Launch the debug binary with a dedicated log:

```bash
HOME="$SMOKE_ROOT/home" RUST_LOG=info \
  target/debug/horizon --config "$SMOKE_ROOT/config.yaml" --ephemeral \
  >"$SMOKE_ROOT/horizon.log" 2>&1
```

Record the exact PID and its native window ID before interacting. Do not target
another installed or development Horizon process by application name.

## Baseline And Visual State

1. Capture the initial window before changing the view.
2. Use Fit so all three workspaces and the full minimap are visible.
3. Confirm the active workspace has the brighter fill, stronger stroke, and a
   crisp accent halo that is not clipped by the minimap edge.
4. Confirm the focused panel has a visible 1 px foreground outline and is drawn
   above an overlapping non-focused panel.
5. Change panel focus and workspace focus. The outline and halo must move to the
   new targets without stale pixels.
6. Resize the window narrower and shorter, then wider and taller. The minimap,
   labels, halo, panel outline, and viewport rectangle must remain contained and
   must not overlap unrelated controls.
7. Capture screenshots at launch, fitted desktop size, narrow size, and restored
   size.

## Hover And Tooltip

1. Hover empty minimap ground. No hand cursor or tooltip should appear.
2. Hover `Alpha Build` outside a panel rectangle. The workspace stroke should
   brighten, the cursor should become a pointing hand, and the tooltip should
   read `Alpha Build - 2 panels` using the app's typographic dash.
3. Hover `Alpha Shell`. The tooltip must match the panel chrome title.
4. In `Alpha Shell`, run:

   ```bash
   printf '\033]0;Runtime Build Running\007'
   ```

   Hover its minimap panel again. The tooltip must show the runtime-aware display
   title, not the stale configured title alone.
5. Sweep between overlapping minimap panels. Hover and tooltip precedence must
   match the panel visibly painted on top.
6. Leave and re-enter the minimap, then stop rapidly over a target. The final
   highlight and tooltip must match the final pointer location without flicker.

## Attached Navigation

1. Click an inactive workspace outside its panel rectangles. It must become
   active and glide to the center through the existing pan animation.
2. Click a panel in a non-active workspace. It must become focused, its workspace
   must become active, and the panel must center.
3. Click the overlapping area of two panels. The visually topmost panel must win.
4. Change focus and repeat the overlap click. Focused-last paint and hit order
   must remain consistent.
5. Click empty minimap ground. The corresponding canvas point must center and no
   workspace or panel focus should change unexpectedly.
6. Double-click a workspace outside its panel rectangles. The workspace must fit
   the main canvas; the first click must not leave an incorrect intermediate
   focus or pan target.
7. Double-click directly over a non-last panel. Its containing workspace must
   fit and the clicked panel must remain focused after the fit.

## Drag And Motion Evidence

1. Start an 8-10 second native recording scoped to the display containing the
   exact test window.
2. Drag horizontally, vertically, and diagonally across the minimap.
3. Reverse direction twice and stop abruptly.
4. While dragging, the canvas must pan continuously and monotonically with the
   pointer. Hover highlights, tooltips, and hand cursor must stay suppressed.
5. On release, hover state may return once and must match the release target.
6. Confirm the minimap's own geometry and viewport rectangle do not jump, resize,
   or oscillate during the drag.
7. Save a post-motion screenshot in addition to the recording.

## Detached Window Lane

1. Detach `Beta Services` through the workspace controls and record the root and
   detached native window IDs for the exact PID.
2. Confirm the detached minimap contains only Beta workspace panels and does not
   draw the attached active-workspace halo around the entire map.
3. Confirm the focused panel outline is visible above the overlapping panel.
4. Click each detached minimap panel. The matching panel must focus and center in
   the detached window, without moving the root canvas.
5. Click the overlapping panel area. Hit precedence must match the detached
   canvas order, including after focus changes.
6. Click detached minimap ground and drag it. Only the detached canvas view must
   pan; the root view must remain stable.
7. Double-click the detached workspace. It must fit within the detached canvas
   rect rather than the root canvas.
8. Resize the detached window in both axes and repeat panel click, ground click,
   drag, and double-click fit.
9. Capture root and detached screenshots after focus, resize, and fit.

## Reassignment Ordering Edge Case

1. Reattach `Beta Services`.
2. Move `Alpha Shell` to `Beta Services`, so its global board order differs from
   its appended order inside the target workspace.
3. Detach `Beta Services` again and overlap `Alpha Shell` with an existing Beta
   panel if layout movement is needed.
4. Verify the detached canvas, minimap paint order, hover tooltip, and click hit
   target all agree about the top panel.
5. Focus the lower panel and verify focused-last behavior updates both paint and
   hit precedence.

## Theme, Persistence, And Compatibility

1. Switch to the light theme. Repeat active halo, focused outline, hover, tooltip,
   and overlap checks; all indicators must retain sufficient contrast.
2. Switch back to dark and verify no stale light-theme shapes remain.
3. Close the isolated app cleanly, preserving the session state, then relaunch it
   with the same `HOME` and config without `--ephemeral` if persistence requires a
   named session.
4. Confirm saved workspace/panel positions, focused target, canvas view, and
   detached state restore without malformed minimap geometry.
5. Confirm the existing version 8 config loads without migration or schema
   changes. This PR must not modify the config file merely by launching.

## Pointer Performance

Build the profiling binary:

```bash
cargo build --locked --profile profiling --features trace-profiling \
  -p horizon-ui --bin horizon
```

Run matched 8-10 second workloads with the same layout and isolated runtime:

1. idle with the minimap visible;
2. pointer movement over empty canvas outside the minimap;
3. pointer movement over empty minimap ground;
4. pointer movement across workspace and panel targets;
5. minimap drag panning.

Compare `horizon::app::update`, `render_active_view`, minimap rendering, and
`egui::context::pass` normalized per-call costs against the exact PR #244 base
commit. Confirm there is no continuous repaint after the pointer stops and no
eager per-panel event cloning or allocation loop was introduced.

## Pass Criteria

- Attached and detached focus, center, fit, and drag operations target the exact
  visual object under the pointer.
- Panel tooltips use runtime-aware display titles.
- Focused-last paint order and hit order agree after panel reassignment.
- Hover state is stable, suppressed during drag, and does not cause passive
  continuous repaint.
- Light/dark, Retina, resize, overlap, and edge clipping checks are clean.
- Existing config and persisted runtime state remain compatible.
- All validation commands pass with no crash, panic, clippy warning, or rendering
  validation error.

## PR Report And Cleanup

Post a PR comment using the required format:

```text
SMOKE-TEST REPORT (macOS/Metal)
- <step>: pass | fail - <short evidence>
Summary: <fixes made, remaining issues, exact head>
SMOKE-TEST: DONE
```

The final line must be exactly `SMOKE-TEST: DONE`. After a complete passing run,
delete this temporary plan from the final branch, close only the exact test PID,
and remove `$SMOKE_ROOT` plus temporary profiling artifacts.
