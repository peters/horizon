# SSH Reconnect Shortcut Smoke Test

Date: 2026-03-25
Scope: disconnected SSH panel local reconnect shortcut (`Ctrl+Shift+R`)

## Preconditions
- Build the target branch in the exact checkout under test.
- Launch Horizon with a persistent session so SSH panel restore behavior is exercised.
- Have one reachable SSH or Tailscale SSH target and one way to force a disconnect.
- Ensure no global Horizon shortcut is configured to `Ctrl+Shift+R`.

## Baseline
1. Launch Horizon.
2. Open `Ctrl+Shift+H`.
3. Connect to a remote host and confirm the SSH panel reaches `Connected`.
4. Right-click the SSH panel and confirm `Reconnect` still appears in the context menu.
5. Press `Ctrl+Shift+R` while the panel is connected.
Expected:
- No reconnect is triggered.
- No unexpected panel restart occurs.
- The keypress is not treated as a new global Horizon shortcut.

## Primary Flow
1. Disconnect the SSH session or force the remote process to exit so the panel reaches `Disconnected`.
2. Focus the terminal body of that disconnected SSH panel.
3. Press `Ctrl+Shift+R` once.
Expected:
- The same panel reconnects in place.
- The SSH status transitions to `Connecting` and then `Connected` if the target is reachable.
- Panel identity, position, size, title, and workspace stay unchanged.

## Persistence / Restore
1. With a disconnected SSH panel present, close Horizon cleanly.
2. Relaunch Horizon and restore the same session.
3. Confirm the SSH panel restores as a disconnected snapshot.
4. Focus the restored panel and press `Ctrl+Shift+R`.
Expected:
- The restored panel reconnects in place.
- No duplicate panel is created.
- Transcript/history remains visible until new session output arrives.

## Conflict / Guardrail Checks
1. Temporarily bind any existing global Horizon shortcut to `Ctrl+Shift+R` in config.
2. Reload Horizon or restart the app so the new shortcut config is active.
3. Put an SSH panel into `Disconnected`.
4. Focus the disconnected SSH panel and press `Ctrl+Shift+R`.
Expected:
- The local reconnect shortcut is disabled.
- Only the configured global shortcut behavior runs.
- No double-trigger or duplicate restart occurs.

## Edge Cases
1. Focus a normal shell panel and press `Ctrl+Shift+R`.
2. Focus an agent panel and press `Ctrl+Shift+R`.
3. Focus a disconnected SSH panel and hold `Ctrl+Shift+R` long enough for key repeat.
4. Open the SSH panel context menu and use `Reconnect`.
Expected:
- Non-SSH panels ignore the local shortcut.
- Repeated keypress events do not queue multiple reconnects.
- Context-menu reconnect still works.

## Visual Checks
1. Capture a live screenshot after launch with a connected SSH panel.
2. Capture a live screenshot with a disconnected SSH panel before reconnect.
3. Capture a live screenshot after reconnect succeeds.
4. Resize the panel and fit the workspace once during the pass.
Expected:
- No new chrome artifacts or badge regressions.
- No focus loss or layout regression around the terminal body.
