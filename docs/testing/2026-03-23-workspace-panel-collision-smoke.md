# Workspace Panel Collision Smoke Test

## Setup

- Build `target/debug/horizon`.
- Launch Horizon with a temporary `HOME` and a config containing:
  - one left workspace with a single panel at `[0, 40]`
  - one right workspace with a single panel at `[620, 40]`
- Keep the left workspace active on launch.

## Baseline

- Confirm both workspaces render side by side without overlap before interaction.
- Capture a launch screenshot showing the initial spacing between the left and right workspace frames.

## Primary Flow

- Add a new panel to the active left workspace with `Ctrl+Shift+N`.
- Wait for runtime state autosave to flush.
- Confirm the right workspace shifts right after the left workspace expands.
- Capture a post-interaction screenshot showing the new panel and the shifted right workspace.

## Edge Cases

- Confirm the new panel lands in the left workspace, not the right workspace.
- Confirm the right workspace remains otherwise intact: same panel count, same workspace name, no detach/reattach behavior.

## Persistence And Migration

- Inspect the saved runtime state and confirm the right workspace `position` increased on the x-axis after panel creation.
- Restart is not required for this fix, but the saved runtime should reflect the moved workspace immediately.

## Visual Regression Check

- Verify workspace chrome remains readable after the shift.
- Verify panel frames do not overlap after the add-panel action.

## Execution Note

- This plan is included for execution on a different machine or by a different agent.
- Do not run this smoke test on this PC.
