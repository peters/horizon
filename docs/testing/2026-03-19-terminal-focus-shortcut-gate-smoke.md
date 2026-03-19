# Terminal Focus Shortcut Gate Smoke Test

## Scope

Validate issue `#50` end to end:

- every non-fullscreen app shortcut is blocked while a terminal panel owns
  keyboard focus
- the only explicit global exceptions are:
  - panel fullscreen
  - exit panel fullscreen
  - window fullscreen
- the focused panel affordance is stronger than the unfocused state and remains
  readable in normal and fullscreen layouts

## Expected Result Summary

The pass condition is:

1. Terminal focus blocks app shortcuts that would otherwise open Horizon UI or
   mutate the canvas.
2. The same shortcuts work again immediately after focus moves to a
   non-terminal surface.
3. `F11`, `Escape`, and `Ctrl/Cmd+F11` remain functional while terminal focus is
   active.
4. The focused panel is visually obvious from the border and titlebar treatment
   alone.

## Test Environment

- Run from the exact issue branch or worktree that will be pushed.
- Use a temporary `HOME` so runtime state, sessions, and config stay isolated.
- Prefer X11 or a capture path that can reliably produce screenshots.
- Keep the default shortcut bindings unless the test explicitly says otherwise.

## Seeded Config

Use this config so the smoke run starts from a deterministic layout with one
terminal panel and one non-terminal panel:

```yaml
window:
  width: 1200
  height: 820
  x: 120
  y: 80
workspaces:
  - name: Smoke
    position: [0, 40]
    terminals:
      - name: Shell
        kind: shell
        position: [40, 80]
        size: [420, 260]
      - name: Notes
        kind: editor
        command: /absolute/path/to/a/markdown/file.md
        position: [520, 80]
        size: [420, 260]
```

Replace the markdown file path with any readable local file.

## Launch Procedure

1. Create a temporary directory for the smoke pass.
2. Write the seeded config above into that directory.
3. Launch Horizon with a temporary `HOME` and the explicit config path.
4. Confirm the window opens at roughly the configured size and position.
5. Confirm both panels are visible without dragging or zooming first.

Example launch command:

```bash
TMPDIR="$(mktemp -d /tmp/horizon-issue50-smoke-XXXXXX)"
mkdir -p "$TMPDIR/home"
$EDITOR "$TMPDIR/config.yaml"
HOME="$TMPDIR/home" cargo run -p horizon-ui -- --new-session --config "$TMPDIR/config.yaml"
```

## Evidence To Capture

Capture these artifacts during the run:

- screenshot with the terminal panel focused and the editor panel unfocused
- screenshot with the editor panel focused and the terminal panel unfocused
- screenshot after verifying a blocked shortcut while terminal focus is active
- screenshot with panel fullscreen active
- screenshot after relaunch during the persistence section
- short written notes for any failure including the exact shortcut, focused
  surface, and observed behavior

## Baseline Checks

1. Launch Horizon and confirm the main window is visible.
2. Confirm the `Shell` terminal panel and the `Notes` editor panel both render.
3. Click the terminal panel body.
4. Confirm the terminal now has the stronger focus treatment.
5. Confirm the editor panel remains visibly unfocused.
6. Capture the first screenshot.

## Core Shortcut Matrix

With the terminal panel focused, trigger each shortcut below and confirm it does
not fire:

- `Ctrl/Cmd+K` does not open the command palette.
- `Ctrl/Cmd+0` does not reset the canvas view.
- `Ctrl/Cmd++` does not zoom in.
- `Ctrl/Cmd+-` does not zoom out.
- `Ctrl/Cmd+Shift+A` does not align workspaces.
- `Ctrl/Cmd+,` does not open settings.
- `Ctrl/Cmd+B` does not toggle the sidebar.
- `Ctrl/Cmd+Shift+H` does not toggle the HUD.
- `Ctrl/Cmd+Shift+M` does not toggle the minimap.
- `Ctrl/Cmd+N` does not create a new panel.

For each blocked shortcut, verify there is no visible side effect. If a side
effect is subtle, repeat the keypress after intentionally creating a
non-default state first. Examples:

- pan or zoom the canvas slightly before checking `Ctrl/Cmd+0`
- leave the sidebar visible before checking `Ctrl/Cmd+B`
- keep only one workspace attached before checking `Ctrl/Cmd+Shift+A`

## Explicit Global Exceptions

With the terminal panel still focused:

1. Press `F11` and confirm the focused terminal enters panel fullscreen.
2. Capture the fullscreen screenshot.
3. Press `Escape` and confirm panel fullscreen exits immediately.
4. Press `Ctrl/Cmd+F11` and confirm the window enters native fullscreen.
5. Press `Ctrl/Cmd+F11` again and confirm the window exits native fullscreen.

These are the only shortcuts in scope that should remain active during terminal
focus.

## Focus Transfer Checks

1. Click the editor panel body.
2. Confirm the stronger focus treatment moves from the terminal panel to the
   editor panel.
3. Capture the second screenshot.
4. Re-run a representative subset of previously blocked shortcuts and confirm
   they now work again:
   - `Ctrl/Cmd+K`
   - `Ctrl/Cmd+0`
   - `Ctrl/Cmd+B`
5. Move focus back to the terminal and confirm those shortcuts stop working
   again.

## Text Entry Surface Checks

1. Focus the terminal panel.
2. Open a Horizon-owned text entry surface using the mouse only. Inline panel
   rename is the preferred path.
3. Confirm typed text goes into that field rather than into the terminal.
4. While the text entry surface is active, confirm normal field editing works.
5. Close or commit the text entry surface.
6. Confirm terminal focus resumes and the shortcut gate is back in effect.

## Drag And Resize Regression Checks

1. Focus the terminal panel and drag it to a different location.
2. Confirm the focused chrome remains attached to the moved panel.
3. Re-run `Ctrl/Cmd+K` and confirm it is still blocked.
4. Resize the focused terminal panel.
5. Confirm the focused chrome still looks correct after resize.
6. Re-run `Ctrl/Cmd+B` and confirm it is still blocked.

## Persistence Checks

1. Leave the terminal panel focused.
2. Close Horizon cleanly.
3. Relaunch with the same temporary `HOME` and config path.
4. Confirm the session restores successfully.
5. Confirm the focused panel chrome is still clearly visible after relaunch.
6. Re-run at least these shortcuts with terminal focus restored:
   - `Ctrl/Cmd+K` remains blocked
   - `Ctrl/Cmd+B` remains blocked
   - `F11` still works
7. Capture the relaunch screenshot.

## Visual Review Criteria

Every screenshot should satisfy all of the following:

- the focused panel is identifiable without reading terminal text colors
- the border and titlebar treatment are stronger on the focused panel
- title text remains legible
- the titlebar indicator does not collide with the title text
- the attention badge and history meter remain readable if present
- fullscreen mode does not remove the ability to visually identify the focused
  panel state before entering and after exiting fullscreen

## Failure Logging

If anything fails, record:

- the exact shortcut or interaction
- which panel kind was focused
- whether the failure was incorrect blocking, missing blocking, or visual
  regression
- whether it reproduced after relaunch
- the screenshot filename or other evidence path

## Cleanup

1. Quit Horizon.
2. Archive the screenshots and notes with the smoke result.
3. Delete the temporary `HOME` and config directory unless the investigating
   agent needs to keep it for a follow-up run.
