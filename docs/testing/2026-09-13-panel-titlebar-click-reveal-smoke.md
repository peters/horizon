# Smoke Plan: Reveal Panel On Titlebar Click

Temporary validation artifact for the change that makes a panel titlebar
click reveal the panel the same way a sidebar panel-row click does.

Delete this file after the UI validation pass is complete unless it is
explicitly needed longer.

## Target

- Repository: `peters/horizon`
- Branch: `fix/panel-click-reveal`
- Binary: `target/debug/horizon` (debug is enough)
- Platform: Linux X11/Wayland on this workstation; repeat on macOS only if
  detached-window focus needs a second machine

## Build + Launch

1. In the PR worktree, run `cargo build -p horizon-ui --bin horizon`.
2. Launch with an isolated session so this does not touch the user's boards:

   ```bash
   unset HORIZON
   TMPCFG=$(mktemp -d)
   cat > "$TMPCFG/config.yaml" <<'EOF'
   features:
     attention_feed: true
     sidebar_accordion: true
   EOF
   HORIZON= target/debug/horizon --config "$TMPCFG/config.yaml" --ephemeral
   ```

3. Keep the window mapped and the sidebar visible.

## Test Data Setup

1. Create workspace `Near` with two shell panels, `Left` and `Right`.
2. Create workspace `Far` with one tall shell panel. Drag `Far` well off to
   the right so a later reveal must pan.
3. Optionally detach `Far` into its own window for the detached lane.

## Baseline

1. Click `Right` in the sidebar. The canvas pans/zooms so `Right` is visible,
   workspace left-aligned, no zoom-in.
2. Click `Left` in the sidebar. Same reveal path; `Left` is focused.
3. Click the `Far` workspace/panel in the sidebar. The view moves to `Far`.
4. Fit workspace and reset zoom still work from the existing shortcuts/menu.

## Primary: titlebar click

1. Sidebar-select `Right`, then pan the canvas so `Right`'s titlebar is still
   visible but the panel is no longer in the sidebar-reveal pose (nudge pan
   ~100px).
2. Click `Right`'s titlebar (not close, not the body).
   - Expected: the view returns to the same reveal pose as a sidebar click.
   - Expected: `Right` is focused.
3. Click `Left`'s titlebar.
   - Expected: same reveal as clicking `Left` in the sidebar.
4. Drag `Right` by its titlebar.
   - Expected: the panel moves; the canvas does not jump to a reveal pose
     mid-drag.

## Body click (no reveal)

1. Sidebar-select `Right` so both `Left` and `Right` stay on screen.
2. Click inside `Left`'s terminal body.
   - Expected: `Left` focuses.
   - Expected: the canvas does **not** pan/zoom.
3. Type a character in `Left`.
   - Expected: it lands in `Left`; the view stays put.

## Other click-to-focus paths

1. Command palette (`Ctrl+P` / `@Left`): selecting `Left` reveals it the
   same way as the sidebar, including when `Left` lives in a detached
   workspace (that OS window is focused instead of panning the main canvas).
2. Toolbar terminal search: picking a match in `Far` scrolls the terminal to
   the match **and** reveals the panel.
3. Attention feed: if an attention item exists for a panel, `Go to panel`
   and clicking the item reveal that panel.

## Edge cases

1. Double-click a titlebar: rename starts; the canvas does not snap away
   under the editor.
2. Click the titlebar close control: the panel closes; no stray reveal of a
   dying panel.
3. Click a panel that is already fully in the sidebar-reveal pose: focus
   stays, no jittery pan.
4. Oversized/tall panel: titlebar click bottom-aligns at minimum zoom, same
   as sidebar.
5. Accordion sidebar: workspace-row click still reveals the selected panel.

## Persistence / restart

Not in scope. Reveal is a view action, not a persisted layout change beyond
the existing canvas_view dirty flag.

## Visual regressions

1. Screenshot after launch with `Near` revealed.
2. Screenshot after titlebar-clicking `Left`.
3. Screenshot after body-clicking `Right` (view must match the previous
   reveal, only the focus chrome changes).
4. Confirm titlebar, close, resize handle, and sidebar rows still look and
   hit-test as before.

## Pass / fail

- Pass: titlebar click matches sidebar reveal; body click focuses only;
  palette/search/feed use the same detached-aware helper; no drag/rename/close
  regressions.
- Fail: titlebar click only focuses; body click pans; detached palette
  selection pans the main canvas; drag or close triggers reveal.
