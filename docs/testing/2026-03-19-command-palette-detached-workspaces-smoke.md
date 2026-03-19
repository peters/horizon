# Command Palette Detached Workspace Smoke Test

## Scope

Validate the command palette and `Ctrl/Cmd+Shift+A` after the unified palette
change, with focus on detached workspaces staying out of the root-window target
list and the workspace-alignment command remaining reachable.

## Environment

- Build from the current checkout.
- Run Horizon with a temporary `HOME` so the test does not mutate real session
  data.
- Use a board with at least three workspaces:
  - two attached workspaces visible in the root window
  - one workspace with at least one panel that will be detached
- Give each workspace a distinct name and at least one visible panel title so
  palette filtering is easy to confirm.

## Baseline

1. Launch Horizon and confirm the root window renders all attached workspaces.
2. Confirm the command palette opens with `Ctrl/Cmd+K`.
3. Confirm the Actions section contains `Align Workspaces`.
4. Confirm the Panels section lists panels from every workspace before any
   detach action.

## Primary Flow

1. Detach one workspace into its own native window.
2. Return focus to the root window.
3. Open the command palette and search for the detached workspace name.
4. Confirm the root-window palette does not list the detached workspace.
5. Search for a panel title that exists only inside the detached workspace.
6. Confirm that panel is not listed in the root-window palette.
7. Search for `align workspaces` and execute the action from the palette.
8. Confirm only the attached workspaces are rearranged in the root window.
9. Confirm the root viewport stays centered on visible attached content instead
   of empty canvas.
10. Confirm the detached workspace window remains open and still renders its
    panels.

## Edge Cases

1. With only one attached workspace remaining, trigger `Ctrl/Cmd+Shift+A` and
   confirm it is a no-op.
2. Reattach the detached workspace and reopen the palette.
3. Confirm the reattached workspace and its panels appear in the root-window
   palette again.
4. Trigger `Ctrl/Cmd+Shift+A` and confirm all attached workspaces align in a
   horizontal row.
5. Repeat the palette alignment flow after zooming in and after zooming out to
   confirm focus targeting still lands on visible content.

## Persistence

1. Detach a workspace, close Horizon, and relaunch from the same temporary
   session state.
2. Confirm the detached workspace restores in its own window.
3. In the root window, open the command palette and confirm detached workspace
   names and panel titles are still excluded.
4. Trigger `Ctrl/Cmd+Shift+A` and confirm detached workspaces remain untouched
   while attached workspaces still align.

## Visual Checks

1. Capture a screenshot after launch with all workspaces attached.
2. Capture a screenshot of the root window after detaching one workspace and
   opening the command palette.
3. Capture a screenshot after running `Align Workspaces` with one workspace
   detached.
4. Confirm the root-window screenshots always show visible attached workspace
   frames and panel content.
