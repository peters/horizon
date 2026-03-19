# Shortcut Config Smoke Test

## Goal

Verify that every documented app-level keyboard shortcut is editable through the settings config file, applied live, persisted after save, and reflected in runtime behavior without breaking existing panel/workspace flows.

## Environment

- Build from the exact worktree under test.
- Launch Horizon on a desktop session with keyboard focus available.
- Start from a writable config file so the settings editor can save changes.
- Keep at least one Markdown editor panel and one shell panel available during the pass.

## Baseline

1. Launch Horizon with the config file you intend to validate.
2. Confirm the app opens without config parse errors.
3. Open the settings editor and verify the `shortcuts:` block includes:
   - `quick_nav`
   - `new_terminal`
   - `toggle_sidebar`
   - `toggle_hud`
   - `toggle_minimap`
   - `toggle_settings`
   - `reset_view`
   - `zoom_in`
   - `zoom_out`
   - `fullscreen_panel`
   - `exit_fullscreen_panel`
   - `fullscreen_window`
   - `save_editor`
4. Confirm the settings editor shows the active config path you launched with.

## Primary Flows

1. With a shell panel focused, verify:
   - quick nav opens with the configured `quick_nav`
   - reset view works with the configured `reset_view`
   - zoom in and zoom out work with the configured `zoom_in` and `zoom_out`
   - fullscreen panel enters with `fullscreen_panel`
   - fullscreen panel exits with `exit_fullscreen_panel`
   - window fullscreen toggles with `fullscreen_window`
2. With focus outside terminal text entry, verify:
   - `new_terminal` creates a panel
   - `toggle_sidebar` hides and restores the sidebar
   - `toggle_hud` hides and restores the HUD
   - `toggle_minimap` hides and restores the minimap
   - `toggle_settings` opens and closes the settings editor
3. With a Markdown editor panel focused, modify text and verify `save_editor` persists the file.

## Live Preview And Persistence

1. Change at least two shortcuts in the settings editor to alternate bindings that do not conflict.
2. Before saving, verify live preview updates runtime behavior for those shortcuts.
3. Save the config and confirm the status switches to `Saved`.
4. Close and reopen the settings editor and confirm the edited bindings remain in the buffer.
5. Restart Horizon and confirm the saved shortcuts still work.

## Validation Failures

1. Enter a shortcut with an invalid key token such as `Ctrl+Nope`.
2. Confirm the editor shows an invalid config error and does not silently apply/save it.
3. Enter duplicate bindings for two actions.
4. Confirm the editor reports the duplicate shortcut conflict.
5. Revert back to a valid config before ending the pass.

## Visual Regression Checks

1. Capture a screenshot immediately after launch with settings closed.
2. Capture a screenshot with settings open and the `shortcuts:` block visible.
3. Resize the window narrower while settings remain open and confirm:
   - the settings panel stays visible
   - the bottom settings bar remains usable
   - sidebar/minimap/HUD layout still looks correct
4. Capture a screenshot after the resize pass.
