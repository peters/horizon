# SSH Restore Snapshot Smoke Test

## Scope

Validate two SSH lifecycle fixes:

1. Persisted-session restore after restarting the local machine restores SSH panels as disconnected snapshots instead of silently reconnecting.
2. A live SSH panel drops out of `Connected` after the remote host dies or reboots instead of staying green indefinitely.

## Setup

- Build the exact branch under test.
- Start from a clean Horizon session store for the test user, or record the session ID created for this run.
- Ensure at least one reachable SSH target is available.
- Prefer a target you can reboot on demand and verify independently.
- If multiple Horizon processes may exist, target the exact PID under test.

## Baseline

1. Launch Horizon.
2. Open `Remote Sessions`.
3. Connect to a known host.
4. Confirm the panel shows terminal output and eventually shows the green `Connected` badge.
5. Run a command that leaves visible output in scrollback, such as `echo restore-marker`.
6. Record a screenshot of the connected state.

## Persisted Restore

1. Close Horizon normally so the runtime state and transcript are persisted.
2. Relaunch Horizon into the same persisted session.
3. Confirm the SSH panel is present in the same workspace and position.
4. Confirm the panel body still shows the prior transcript, including `restore-marker`.
5. Confirm the badge is red `Disconnected`, not green `Connected`.
6. Confirm Horizon did not silently open a fresh remote shell prompt during restore.
7. Open the panel context menu and use `Reconnect`.
8. Confirm the badge transitions through `Connecting...` and back to `Connected` once the remote prompt returns.
9. Confirm new input works only after reconnect.

## Remote Reboot / Drop Detection

1. With an active connected SSH panel, reboot the remote host or otherwise terminate the remote session path.
2. Do not type in Horizon after triggering the reboot; wait for client keepalive detection.
3. Confirm the panel badge changes from `Connected` to `Disconnected` within roughly one keepalive cycle plus grace period.
4. Confirm the panel remains visible and does not auto-close.
5. Confirm the transcript from before the disconnect remains available for scrolling and copy.
6. Use `Reconnect` after the remote host is back online.
7. Confirm the session reconnects successfully and the badge returns to `Connected`.

## Edge Cases

1. Restore a persisted session with multiple SSH panels and confirm all restore as disconnected snapshots.
2. Restore a mixed workspace with shell, agent, and SSH panels and confirm only SSH panels avoid auto-reconnect.
3. Restore an SSH panel whose transcript file is empty and confirm it still restores as a disconnected panel without crashing.
4. Confirm an SSH panel opened fresh from `Remote Sessions` still connects immediately and does not start disconnected.
5. If user config overrides SSH options through `extra_args`, confirm reconnect still works with those overrides present.

## Persistence / Regression Checks

1. After restoring a disconnected snapshot, close Horizon again without reconnecting.
2. Relaunch and confirm the panel is still restored as a disconnected snapshot.
3. Reconnect, close Horizon normally, and relaunch.
4. Confirm the panel again restores as disconnected rather than auto-reconnecting.

## Visual Checks

1. Capture a screenshot after restore showing the red `Disconnected` badge and preserved terminal transcript.
2. Capture a screenshot after reconnect showing the green `Connected` badge.
3. Confirm the panel title, badge placement, and chrome spacing remain aligned in both states.

## Notes

- This is a temporary validation artifact for the SSH restore/reconnect change.
- Delete it after live validation is complete unless the user asks to keep it.
