# Shortcut Config Smoke Test

## Goal

Verify that every documented app-level keyboard shortcut is editable through the settings config file, applied live, persisted after save, and reflected in runtime behavior without breaking existing panel/workspace flows.

This pass must cover all shortcuts documented in the README, not just a subset.

## Environment

- Build from the exact worktree under test.
- Launch Horizon on a desktop session with keyboard focus available.
- Start from a writable config file so the settings editor can save changes.
- Keep at least one Markdown editor panel and one shell panel available during the pass.
- Create or restore at least three attached workspaces before starting shortcut checks.
- Place the attached workspaces at visibly different positions so alignment and reset-view behavior are easy to detect.

## Baseline

1. Launch Horizon with the config file you intend to validate.
2. Confirm the app opens without config parse errors.
3. Open the settings editor and verify the `shortcuts:` block includes:
   - `quick_nav`
   - `new_terminal`
   - `toggle_sidebar`
   - `toggle_hud`
   - `toggle_minimap`
   - `align_workspaces_horizontally`
   - `toggle_settings`
   - `reset_view`
   - `zoom_in`
   - `zoom_out`
   - `fullscreen_panel`
   - `exit_fullscreen_panel`
   - `fullscreen_window`
   - `save_editor`
4. Confirm the settings editor shows the active config path you launched with.
5. Confirm the initial UI state is easy to observe:
   - sidebar visible
   - HUD visible
   - minimap visible
   - window not fullscreen
   - no panel is fullscreen
6. Note which workspace is active before starting shortcut checks.

## Default Shortcut Coverage

1. With a shell panel focused, verify:
   - `quick_nav` opens the quick navigator and allows selecting another workspace
   - `zoom_in` increases the zoom level
   - `zoom_out` decreases the zoom level
   - `align_workspaces_horizontally` moves the visible attached workspaces into a left-to-right row with a shared top edge
   - `fullscreen_panel` puts the active panel into fullscreen
   - `exit_fullscreen_panel` exits active panel fullscreen
   - `fullscreen_window` toggles the application window fullscreen state on and off
2. With the canvas deliberately zoomed and panned away from the workspaces, verify `reset_view`:
   - restores the default zoom level
   - keeps at least one attached workspace visible after the reset
   - does not leave the canvas centered on empty space
3. With focus outside terminal text entry, verify:
   - `new_terminal` creates a panel
   - `toggle_sidebar` hides and restores the sidebar
   - `toggle_hud` hides and restores the HUD
   - `toggle_minimap` hides and restores the minimap
   - `toggle_settings` opens and closes the settings editor
4. With a Markdown editor panel focused, modify text and verify `save_editor` persists the file.

## Rebinding Coverage

1. In the settings editor, assign every shortcut a different non-conflicting binding.
2. Use at least one alternate binding from each category:
   - primary + letter
   - primary + shift + letter
   - primary + symbol
   - function key
   - unmodified key
3. Save the config and confirm the status switches to `Saved`.
4. Verify every remapped shortcut now uses the new binding and the old binding no longer triggers the action:
   - `quick_nav`
   - `new_terminal`
   - `toggle_sidebar`
   - `toggle_hud`
   - `toggle_minimap`
   - `align_workspaces_horizontally`
   - `toggle_settings`
   - `reset_view`
   - `zoom_in`
   - `zoom_out`
   - `fullscreen_panel`
   - `exit_fullscreen_panel`
   - `fullscreen_window`
   - `save_editor`
5. Include at least one remap that changes `align_workspaces_horizontally` away from `Ctrl+Shift+A` and confirm the remapped key performs the alignment.
6. Include at least one remap that changes `reset_view` away from `Ctrl+0` and confirm the remapped key restores the default zoom while leaving workspaces visible.

## Live Preview And Persistence

1. Before saving the remapped config, verify live preview updates runtime behavior for at least:
   - one letter shortcut
   - one shifted letter shortcut
   - one symbol shortcut
2. Close and reopen the settings editor and confirm the edited bindings remain in the buffer.
3. Restart Horizon and confirm the saved shortcuts still work after relaunch.
4. Confirm the config file on disk still lists every shortcut entry under `shortcuts:`.

## Validation Failures

1. Enter a shortcut with an invalid key token such as `Ctrl+Nope`.
2. Confirm the editor shows an invalid config error and does not silently apply/save it.
3. Enter duplicate bindings for two actions.
4. Confirm the editor reports the duplicate shortcut conflict.
5. Enter overlapping bindings such as `toggle_sidebar: Ctrl+B` and `align_workspaces_horizontally: Ctrl+Shift+B`.
6. Confirm the editor reports the overlap conflict instead of silently accepting the near-duplicate shortcuts.
7. Revert back to a valid config before ending the pass.

## Visual Regression Checks

1. Capture a screenshot immediately after launch with settings closed.
2. Capture a screenshot with settings open and the `shortcuts:` block visible.
3. Resize the window narrower while settings remain open and confirm:
   - the settings panel stays visible
   - the bottom settings bar remains usable
   - sidebar/minimap/HUD layout still looks correct
4. Capture a screenshot after the resize pass.
5. Capture a screenshot after the remapped-shortcut pass with the workspaces aligned horizontally.
