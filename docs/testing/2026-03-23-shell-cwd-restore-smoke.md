# Shell CWD Restore Smoke Test

## Scope

Validate that a shell panel changing directories updates persisted runtime state before shutdown, and that a recovered session restores the shell in that last directory instead of the workspace default.

## Environment

- Platform: Linux X11
- Build under test: `target/debug/horizon`
- Input automation: `xdotool`
- Profile isolation: temporary `HOME` and explicit `--config`

## Baseline Checks

1. Launch Horizon with a single workspace whose workspace `cwd` points at a known `start` directory.
2. Confirm the root Horizon window maps successfully and shows the expected single shell workspace.
3. Capture a launch screenshot for visual sanity.

## Primary Flow

1. Focus the shell panel.
2. Run `cd <restore-dir>`.
3. Wait past the runtime autosave debounce.
4. Inspect the active session `runtime.yaml` while Horizon is still running and confirm the panel `cwd` is `<restore-dir>`.
5. Kill the Horizon PID without graceful shutdown.
6. Relaunch Horizon against the same profile/config so session recovery uses the saved runtime state.
7. Focus the restored shell panel and run `pwd > <evidence-file>`.
8. Confirm the evidence file contains `<restore-dir>`.

## Edge Checks

1. Confirm the restored shell did not fall back to the workspace `cwd` (`<start-dir>`).
2. Confirm session recovery happens from the stale lease path, not by creating a fresh blank session.
3. Confirm no startup chooser blocks recovery in the single-session stale-lease case.

## Visual Checks

1. Capture a screenshot after first launch.
2. Capture a screenshot after recovery to ensure the restored board still renders normally.

## Pass Criteria

- Runtime state updates to the live shell `cwd` before shutdown.
- Recovered session restores the shell in `<restore-dir>`.
- The recovered board remains viewable and interactive.
