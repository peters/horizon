## Theme Auto/Light/Dark Smoke Plan

Scope: validate the new appearance preference flow and the light-mode readability fixes.

Use the branch/worktree that contains this file. Run the debug app, not release:

```bash
cargo build
target/debug/horizon --blank --ephemeral
```

If you need an isolated config during testing, use a temp file and pass it with `--config <path>`.

### Baseline launch

1. Launch with:
   ```yaml
   version: 6
   appearance:
     theme: auto
   workspaces: []
   ```
2. Confirm Horizon opens normally.
3. Capture a screenshot at launch.
4. Resize the main window and capture another screenshot.

Expected:
- No crash or blank window.
- Window chrome, toolbar, panels, and canvas grid all use one coherent palette.
- Text remains readable after resize.

### Settings flow

1. Open Settings with `Ctrl+Shift+,`.
2. In `General > Appearance`, confirm the selector contains:
   - `Auto (system)`
   - `Dark`
   - `Light`
3. Confirm `Auto (system)` is the default when using a fresh config.
4. Capture a screenshot with Settings open.

Expected:
- The setting exists in the General tab.
- The label makes it clear that `Auto` follows the OS theme.

### Explicit override behavior

1. Switch from `Auto (system)` to `Light`.
2. Confirm the whole UI updates on the next frame without reopening the app.
3. Inspect:
   - toolbar
   - workspace chrome
   - settings panel
   - remote hosts overlay
   - terminal panels
4. Switch from `Light` to `Dark`.
5. Switch back to `Auto (system)`.

Expected:
- `Light` and `Dark` act as explicit overrides.
- Returning to `Auto (system)` stops forcing the override.
- No unreadable low-contrast text in the settings panel or overlays.

### Persistence

1. Save with `theme: light`.
2. Relaunch and confirm the app stays light even if the desktop/system theme differs.
3. Save with `theme: dark`.
4. Relaunch and confirm the app stays dark.
5. Save with `theme: auto`.
6. Relaunch and confirm the app follows the current system theme again.

Expected:
- `light` and `dark` persist as explicit overrides.
- `auto` persists as the system-following preference.

### Remote hosts readability

1. Open the Remote Hosts overlay in light mode.
2. Verify column headers, counts, rows, tags, hostname text, status text, and expanded details.
3. Use a catalog with several tags/status values if available.
4. Capture a screenshot of the overlay in light mode.

Expected:
- Alias, IPv4, tags, hostname, status, and last-seen text remain readable.
- Selected-row and hover-row fills are visible without washing out text.
- Status badges and tag colors are distinct in light mode.

### Terminal readability

1. Open a terminal panel in light mode.
2. Run output that exercises ANSI colors, especially dim colors and bright-black/gray variants.
3. If available, reproduce the Codex panel state that previously looked washed out.
4. Capture a screenshot showing terminal text in light mode.

Expected:
- Text does not disappear into the background for common ANSI color combinations.
- Dimmed text is still legible.
- Selection remains readable.

### System-theme follow test

1. Leave Horizon on `Auto (system)`.
2. Change the desktop/system appearance while Horizon is running, if the test environment supports it.
3. If live OS theme changes are not available, validate with two launches under known dark/light system settings.

Expected:
- Horizon follows the system theme when `Auto (system)` is selected.
- Explicit `Light` or `Dark` blocks that automatic behavior.

### Cleanup

After the smoke pass is complete, delete this file unless the user explicitly wants it kept.
