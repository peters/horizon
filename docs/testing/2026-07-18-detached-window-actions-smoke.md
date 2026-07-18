# Detached Window Actions Smoke Test

## Goal

Verify three detached-workspace fixes:

1. Detached windows survive panel fullscreen in the main window (previously
   every detached native window was destroyed the moment a panel went
   fullscreen and recreated on exit).
2. The shortcuts advertised in the detached toolbar tooltips (Fit Workspace,
   Show/Hide Minimap) now work while a detached window is focused.
3. The workspace context menu inside a detached window: Fit Workspace and
   Focus Workspace now act on the detached window's canvas (previously both
   were guaranteed no-ops there).

## Fixes Under Test

- `app/lifecycle.rs`: `render_active_view` renders detached viewports while a
  panel is fullscreen.
- `app/detached_viewports.rs`: `handle_detached_shortcuts` dispatches Fit and
  Minimap shortcuts from the detached viewport's own input state.
- `app/workspace.rs` + `app/view.rs`: context-menu Fit/Focus route to
  `fit_workspace_in_rect` / the new `focus_workspace_in_rect` when the
  workspace is the one shown in the detached window.

## Setup

- Build the debug binary: `cargo build -p horizon-ui`.
- Launch `target/debug/horizon` with an isolated `HOME` and isolated runtime
  state.
- Create two workspaces, each with at least one terminal panel; keep a
  long-running output command (`ping localhost` or similar) in the workspace
  you will detach so liveness is observable.
- Detach one workspace (right-click its label → Detach Workspace, or the
  sidebar detach control). Note the detached window's on-screen position.

## Primary Flow: Fullscreen Keep-Alive

- Focus a panel in the MAIN window and press F11 (panel fullscreen).
- The detached window must remain open, at its position, with its terminal
  output still updating live while the main window shows the fullscreen panel.
- Press Escape (or F11) to leave fullscreen: the detached window must not
  flicker, close, relaunch, or move; its native position and stacking order
  must be exactly as before.
- Repeat with two detached workspaces at distinct positions; both must
  survive, both positions intact.
- While a panel is fullscreen, interact with the detached window (type into
  its terminal, pan its canvas); it must stay fully functional.

## Primary Flow: Detached Shortcuts

- Focus the detached window, pan and zoom its canvas away from the workspace.
- Press the Fit Workspace shortcut (default Ctrl+Shift+9, Cmd+Shift+9 on
  macOS): the workspace must fit within the detached window, matching the
  toolbar button's behavior. The main window's view must not change.
- Press the Minimap shortcut (Ctrl/Cmd+Shift+M): the minimap toggles in the
  detached window. Note the minimap visibility flag is app-global, matching
  the existing toolbar button semantics.
- Hover both toolbar buttons and confirm the advertised tooltip shortcuts
  match the bindings that now work.
- With a terminal focused inside the detached window, press both shortcuts:
  the app action must fire (same policy as the main window's global
  shortcuts).

## Primary Flow: Context Menu Fit and Focus

- In the detached window, pan/zoom far away from the workspace.
- Right-click the workspace title label → Fit Workspace: the workspace must
  fit inside the detached window's canvas.
- Pan away again, right-click → Focus Workspace: the workspace must center at
  the current zoom (no zoom change).
- Repeat both actions from the MAIN window on a non-detached workspace:
  behavior must be unchanged (animated pan / fit on the main canvas).

## Interaction Edge Cases

- Fullscreen a panel while the detached window is focused instead of the main
  window; the detached window must still survive main-window fullscreen
  entered afterwards via the main window.
- Close the detached window while a main-window panel is fullscreen: the
  workspace must reattach cleanly (existing close-to-reattach flow) without
  disturbing the fullscreen panel.
- Reattach via the toolbar button during fullscreen: same expectation.
- Detach a workspace WHILE a panel is fullscreen (from the sidebar if
  reachable): the new window must appear and survive.
- Main-window sidebar context menu on the detached workspace: Fit/Focus there
  still intentionally do nothing to the main canvas (the workspace is not on
  it); confirm no crash and no view jump.

## Persistence

- With one workspace detached, quit and relaunch into the same runtime state:
  the detached window must restore at its saved position.
- Enter panel fullscreen immediately after the restore and confirm the
  restored detached window survives.

## Visual and Performance Regression Checks

- Compare screenshots of the detached window before/during/after main-window
  fullscreen: identical chrome, toolbar, canvas content.
- Confirm no repaint loop: with everything idle (fullscreen active, detached
  window open), CPU must settle to baseline.
- Fit/Focus from the context menu must not leave the canvas mid-animation
  artifacts; the detached fit is an immediate snap, matching the toolbar
  button.

## Cleanup

- Stop only the Horizon PID launched for this test and remove the isolated
  home. Delete this temporary smoke-test plan after the validation pass.
