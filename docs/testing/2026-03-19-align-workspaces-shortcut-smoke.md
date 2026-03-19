# Align Workspaces Shortcut Smoke Test

## Scope

Validate `Ctrl/Cmd+Shift+A` for the main-window workspace alignment flow, with
specific attention to detached workspaces being excluded from both alignment and
focus targeting.

## Environment

- Build from the current checkout.
- Run Horizon with a temporary `HOME` so the test does not mutate real session
  data.
- Use a config with at least two workspaces where the workspace that will be
  detached starts left of the visible main-window workspace.

## Baseline

1. Launch Horizon and confirm the root window renders normally.
2. Confirm the sidebar lists both workspaces.
3. Confirm both workspaces are initially visible in the main canvas.
4. Confirm panning and zooming still work before any detach/align action.

## Primary Flow

1. Detach the leftmost workspace into a native window.
2. Confirm the root window now renders only the remaining attached workspace.
3. Focus the root window.
4. Trigger `Ctrl/Cmd+Shift+A`.
5. Confirm the root canvas stays centered on the attached workspace instead of
   empty space.
6. Confirm the detached window remains open and its workspace content is still
   rendered in that detached window.
7. Confirm the attached workspace moves only relative to other attached
   workspaces.

## Edge Cases

1. Trigger the shortcut when only one attached workspace remains and confirm it
   is a no-op.
2. Reattach the detached workspace and trigger the shortcut again; confirm both
   visible workspaces align in the root window.
3. Repeat after zooming in and zooming out to ensure focus targeting still
   lands on visible content.

## Persistence

1. Detach a workspace, close Horizon, and relaunch.
2. Confirm detached workspace state restores correctly.
3. Trigger the shortcut and confirm detached workspaces still do not pull the
   root viewport to empty space.

## Visual Checks

1. Capture a screenshot after launch showing both workspaces before detaching.
2. Capture a screenshot after detaching the leftmost workspace and triggering
   the shortcut.
3. Confirm the root-window screenshot still shows a visible workspace frame and
   panel content after alignment.
