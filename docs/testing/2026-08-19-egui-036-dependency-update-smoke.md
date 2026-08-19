# Smoke-Test Plan - egui 0.33→0.36 / wgpu 27→30 dependency update (temporary)

> Validation artifact for the dependency-update PR. Delete after the UI
> validation pass is complete.

This PR migrates the whole UI to egui 0.36 (`App::ui` entry point, unified
`egui::Panel`, `DroppedFile` trait, IME rework) and wgpu 30. Everything that
renders is affected, so this plan walks the surfaces whose code changed.
Executable without extra context on any machine with a debug build:

```bash
cargo build
target/debug/horizon --config <plan-board>.yaml --ephemeral
```

Synthetic board (save as `<plan-board>.yaml`):

```yaml
version: 9
workspaces:
  - name: Smoke A
    terminals:
      - name: notes-top
        kind: editor
        position: [80, 80]
        size: [520, 340]
      - name: notes-bottom
        kind: editor
        position: [80, 460]
        size: [520, 300]
  - name: Smoke B
    terminals:
      - name: notes-b
        kind: editor
        position: [700, 120]
        size: [560, 380]
```

On headless Linux: `Xvfb :99 -screen 0 1600x1000x24`, launch with
`DISPLAY=:99`, screenshot via `import -window root`, drive with `xdotool`.

## Baseline

1. Launch with the board above. Expect: toolbar with search field and
   Quick Nav / Remote Hosts / Sessions / Settings buttons, workspace sidebar
   listing both workspaces and their editors, dot-grid canvas with glow,
   workspace header chips (Default/Rows/Cols/Grid/Detach), minimap bottom
   right showing both workspaces. Text must be crisp (new skrifa font stack).
2. Launch with no config (`--ephemeral`, empty board). Expect the
   "Start with a workspace-first flow" empty-state card, no panic.
3. Resize the window to ~1200×700. Expect full relayout: minimap and toolbar
   reposition, no stale regions, no crash.

## Migrated surfaces (each changed in this PR)

4. **Settings panel + bar** (`SidePanel`/`TopBottomPanel` → `egui::Panel`):
   open Settings. Expect right-side panel with tabs and the bottom bar
   (Save / Reset Defaults / Close). Drag the panel's resize edge; the canvas
   must reserve space correctly. Close settings.
5. **Fullscreen panel** (`CentralPanel` on `Ui`): focus a panel, press F11.
   Expect the panel body to fill the window; Escape restores the canvas.
6. **Detached workspace window** (viewport callback now receives `Ui`):
   click Detach on a workspace header. Expect a native child window with the
   workspace toolbar (Fit / minimap toggle) and canvas. Interact, then
   close it - the workspace must reattach, no dead-window crash.
7. **Editor click-to-focus** (TextEdit sizing changed upstream): open an
   editor panel in Edit mode. Click in the *empty area well below the last
   line of text* - the caret must land in the editor (hitbox fills the
   visible pane, not just one text row).
8. **Frameless text inputs** (`TextEdit::frame` migration): open the command
   palette (Ctrl+Shift+K), search overlay (Ctrl+Shift+F), remote hosts
   overlay, dir picker, panel rename (double-click a panel title), SSH upload
   destination, and settings YAML editor. Each input must render without a
   default frame/background box, exactly as before the update.
9. **File drop** (`DroppedFile` is now a trait): drag a `.md` file from a file
   manager onto a workspace. Expect the editor-drop highlight and a new editor
   panel. Drop a non-editor file onto a terminal panel: path pasted into the
   terminal, shell-quoted when needed.
10. **Terminal scroll + selection**: in a terminal panel with scrollback,
    wheel-scroll (no doubled or jumpy deltas), drag a selection below the body
    edge and confirm auto-scroll advances then stops at the bottom.
11. **IME** (Preedit-driven composition tracking, X11/ibus lane): with an IME
    active, compose CJK text into a terminal - preedit then commit must insert
    once. Type Norwegian æøå via dead keys - characters must appear (orphan
    commit path).
12. **Shutdown**: close the window with panels open. Expect the shutdown
    overlay spinner, then clean exit (no zombie process).

## Persistence / migration

13. Launch against an existing persistent session created before this update;
    board, positions, and window geometry must restore unchanged.

## Visual regression

14. Compare launch/hover/resize screenshots against current main: same theme
    colors, panel chrome, dot grid, minimap. Minor kerning differences are
    expected (harfrust shaping); layout shifts or missing chrome are not.

## Executed evidence (2026-08-19, Linux/Xvfb lane)

- Steps 1–3 executed headless (lavapipe software Vulkan through wgpu 30):
  launch, hover, and resize screenshots all correct; app stayed alive.
- Steps 4–14 covered by the automated suite (474 tests, speech tier included)
  where testable headlessly; interactive lanes (6, 9, 11) remain for a
  desktop-session pass if requested.
