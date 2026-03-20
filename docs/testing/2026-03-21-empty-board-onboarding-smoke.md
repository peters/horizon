## Goal

Validate the empty-board startup flow after the onboarding and file-drop fixes:

- Horizon should open to a truly empty board without creating `Workspace 1`.
- The onboarding card should disappear as soon as the first workspace exists.
- Root-level file drops should create a workspace only when a supported editor file is actually dropped.

## Setup

1. Use the exact branch or worktree that contains this change.
2. Start from a clean Horizon profile or a fresh session whose runtime state has:
   - `workspaces: []`
   - no detached workspaces
   - no focused panel
3. Ensure no other Horizon process is running for the same profile.
4. Keep one small markdown file and one unsupported file type available for drag-and-drop checks.

## Baseline Launch

1. Launch Horizon.
2. Confirm the root window opens with the onboarding card visible.
3. Confirm no workspace label, workspace frame, or `Workspace 1` entry appears automatically.
4. Confirm the sidebar workspace list is empty.
5. Capture a screenshot of the initial empty board.

## Onboarding Dismissal

1. Click `New Workspace` in the onboarding card.
2. Confirm exactly one empty workspace appears.
3. Confirm the onboarding card disappears immediately after the workspace is created.
4. Confirm the empty-workspace hint inside the workspace remains visible.
5. Capture a screenshot showing the empty workspace without the onboarding card.

## New Terminal Flow

1. Restart Horizon back into an empty board state.
2. Click `New Terminal` from the onboarding card.
3. Confirm Horizon creates a workspace and opens a terminal in it.
4. Confirm the onboarding card disappears once the terminal appears.
5. Confirm the created workspace is focused and visible.

## Root File Drop

1. Restart Horizon back into an empty board state.
2. Drag a supported markdown or text file over the root window without dropping it.
3. Confirm no workspace is created during hover alone.
4. Drop the supported file on the root window.
5. Confirm Horizon creates a workspace only after the drop and opens an editor panel for the file.
6. Restart Horizon back into an empty board state again.
7. Drop an unsupported file type such as `.png`.
8. Confirm Horizon does not create a workspace for the unsupported drop.

## Persistence / Relaunch

1. With one empty workspace visible and the onboarding card already gone, close Horizon.
2. Relaunch Horizon into the same session.
3. Confirm the empty workspace restores and the onboarding card does not return.
4. Remove the workspace so the board becomes empty again, if supported by the current UX.
5. Confirm the onboarding card returns only when the board is truly empty.

## Visual Regression Checks

1. Verify the onboarding card remains centered on launch.
2. Verify the card does not overlap or occlude the first workspace after creation because it should be gone.
3. Verify toolbar, sidebar, and minimap layout remain unchanged relative to the pre-fix behavior.
