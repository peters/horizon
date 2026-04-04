# macOS Smoke Plan: Workspace Docking + Sidebar Drag

## Purpose

Validate the new workspace docking and sidebar workspace-drag behavior on macOS without relying on any Linux-specific tooling or assumptions.

## Machine Requirements

- macOS 13 or newer
- Windowed desktop session, not a headless login
- Local build prerequisites already installed for Horizon
- Mouse or trackpad available for drag testing
- Screen recording permission available if motion capture is needed

## Build + Launch

1. Open Terminal in the Horizon checkout.
2. Run `cargo build`.
3. Launch `target/debug/horizon`.
4. Keep Horizon in a normal, non-fullscreen macOS window for the main pass.

## Test Data Setup

1. Start from a clean or disposable runtime state if possible.
2. Create four attached workspaces named `Alpha`, `Beta`, `Gamma`, and `Delta`.
3. Add at least one panel to each workspace so every workspace has a visible board frame.
4. Spread the workspaces apart enough that later moves are easy to see.
5. Leave the sidebar open for the full test.

## Baseline Checks

1. Click each workspace row in the sidebar and confirm focus/pan still works.
2. Open the workspace context menu from the sidebar and confirm the existing menu still opens and closes correctly.
3. Confirm panel rows in the sidebar still focus panels and do not act like workspace rows.
4. Drag a workspace on the board and release far from any other workspace. Confirm ordinary free movement still works.

## Canvas Docking Checks

1. Drag `Gamma` close to the right side of `Alpha` and release.
2. Confirm `Gamma` snaps beside `Alpha` instead of ending overlapped.
3. Repeat with `Gamma` docked to the left side of `Alpha`.
4. Repeat with `Gamma` docked above `Alpha`.
5. Repeat with `Gamma` docked below `Alpha`.
6. After each drop, confirm the target workspace stays fixed and any third workspace in the way is pushed clear instead of the target being displaced.
7. Confirm the dragged workspace label still shows the expected grab/grabbing cursor during the interaction.

## Sidebar Drag Checks

1. Drag the `Delta` workspace row onto the upper half of the `Alpha` row.
2. Confirm a visible insertion indicator appears on the target row during drag.
3. Release and confirm `Delta` moves before `Alpha` in the sidebar.
4. Confirm `Delta` also moves to the left of `Alpha` on the board.
5. Drag `Delta` onto the lower half of the `Gamma` row.
6. Confirm `Delta` moves after `Gamma` in the sidebar and to the right of `Gamma` on the board.
7. Confirm the dropped workspace becomes the focused/panned workspace after the drop.

## Edge Cases

1. Start dragging a workspace row and release over empty sidebar space. Confirm no reorder occurs and drag state clears.
2. Drag a workspace row onto itself. Confirm there is no reorder and no board jump.
3. Drag the first workspace after the last workspace.
4. Drag the last workspace before the first workspace.
5. Confirm both boundary cases update sidebar order and board position correctly.
6. If detached workspaces are in scope for the branch, detach one workspace on macOS, drag its sidebar row before or after an attached workspace, then reattach it and confirm the new board position is preserved.

## Persistence Checks

1. After completing several board docks and sidebar reorders, close Horizon cleanly on macOS.
2. Relaunch `target/debug/horizon`.
3. Confirm the sidebar workspace order matches the final drag result from the previous session.
4. Confirm the board positions match the final docking/reorder layout from the previous session.
5. If detached workspaces were part of the run, confirm their restore path still works after relaunch.

## macOS-Specific Checks

1. Move the Horizon window after a few workspace operations and confirm no drag state remains stuck.
2. Switch away from Horizon and back with `Cmd+Tab`, then repeat one sidebar drag and one canvas dock.
3. If using a trackpad, repeat one sidebar drag and one canvas dock with trackpad input to catch gesture-related pointer differences.
4. If available, repeat one drag after connecting an external display to catch coordinate or scaling issues.

## Motion Validation

1. If any issue looks motion-sensitive, capture a short video with `screencapture -V 8 ~/Desktop/horizon-workspace-drag.mov`.
2. Use still screenshots only as supporting evidence, not as the primary proof for drag behavior.

## Failure Evidence To Save

- Exact drag path that failed
- Whether the failure was canvas docking, sidebar reorder, persistence, or detached-window behavior
- Screenshot or short video
- The final visible sidebar order
- The final visible board arrangement
- Whether the issue reproduces with mouse, trackpad, or both

## Explicit Non-Goals For This Pass

- No release build benchmarking
- No Windows- or Linux-specific automation
- No installer validation
- No unrelated panel drag or resize regression sweep beyond the baseline checks above
