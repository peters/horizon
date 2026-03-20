# GitHub Agent Workspace Smoke Test

## Goal

Validate the GitHub-first agent-workspace flow added on branch
`feat/github-agent-workspace` in worktree
`/home/peters/github/horizon-github-agent-workspace`.

This change adds:

- command-palette actions to create a workspace from a GitHub issue, pull
  request, or review comment
- a fixed three-panel launch template: `Research`, `Implement`, `Review`
- task-aware panel titlebar badges for branch, PR state, and waiting status
- task-aware attention items that behave like a review/input queue
- task-level usage rollups in the usage dashboard

## Pass Criteria

1. Each GitHub launch command creates exactly one new workspace bound to the
   selected work item.
2. The created workspace contains exactly three task panels:
   - `Research`
   - `Implement`
   - `Review`
3. Each task panel shows task-specific titlebar badges:
   - branch
   - PR state
   - waiting status
4. Task-generated attention items identify the work item and role panel and
   open the correct panel when selected.
5. The usage dashboard shows per-task usage cards in addition to existing
   per-tool totals.
6. Closing and relaunching Horizon restores the task-bound workspace and the
   task panel metadata.
7. Baseline non-task behavior still works:
   - normal panel creation
   - non-task attention items
   - non-task usage dashboard totals

## Environment

- Run from the exact feature worktree:
  `/home/peters/github/horizon-github-agent-workspace`
- Branch under test: `feat/github-agent-workspace`
- Use a temporary `HOME` so runtime state is isolated.
- `gh` must be installed and authenticated for a repository the tester can
  access.
- The repository used for the test must have:
  - at least one issue
  - at least one pull request
  - at least one review comment or review thread comment URL/reference
- Prefer X11 with `DISPLAY=:1` when available so screenshots and automation are
  stable.

## Recommended Test Data

Pick one repository that satisfies all of the following and note the concrete
 values before starting:

- owner/name
- one issue reference
- one pull request reference
- one review comment reference

Accepted input formats:

- short issue or PR reference such as `#123`
- full GitHub URL for an issue or PR
- review comment URL or `discussion_r...` reference

## Seeded Config

Use a simple seeded config so the board starts predictably and the new
workspace is easy to identify:

```yaml
window:
  width: 1440
  height: 920
  x: 80
  y: 60
workspaces:
  - name: Base
    position: [0, 40]
    terminals:
      - name: Shell
        kind: shell
        position: [60, 90]
        size: [560, 340]
```

## Launch Procedure

1. Create a temporary test directory.
2. Write the seeded config into that directory.
3. Launch Horizon from the feature worktree with the temporary `HOME`.
4. Confirm the root window opens with the `Base` workspace and one normal shell
   panel.

Example:

```bash
TMPDIR="$(mktemp -d /tmp/horizon-github-agent-workspace-smoke-XXXXXX)"
mkdir -p "$TMPDIR/home"
cat > "$TMPDIR/config.yaml" <<'EOF'
window:
  width: 1440
  height: 920
  x: 80
  y: 60
workspaces:
  - name: Base
    position: [0, 40]
    terminals:
      - name: Shell
        kind: shell
        position: [60, 90]
        size: [560, 340]
EOF

cd /home/peters/github/horizon-github-agent-workspace
HOME="$TMPDIR/home" DISPLAY=:1 cargo run -p horizon-ui -- --new-session --config "$TMPDIR/config.yaml"
```

## Evidence To Capture

Capture:

- screenshot immediately after launch
- screenshot of the command palette showing the three GitHub workspace actions
- screenshot after creating an issue-backed workspace
- screenshot after creating a PR-backed workspace
- screenshot after creating a review-comment-backed workspace
- screenshot showing the attention queue with at least one task item
- screenshot of the usage dashboard with task usage cards visible
- screenshot after relaunch restoring a task-bound workspace
- notes for any mismatch including:
  - input used
  - expected result
  - observed result
  - whether the issue is deterministic or intermittent

## Baseline Checks

1. Launch Horizon and confirm the seeded `Base` workspace appears.
2. Confirm the existing shell panel works normally.
3. Open the command palette with `Ctrl/Cmd+K`.
4. Search for `github`.
5. Confirm all three new actions are present:
   - `Create Workspace From GitHub Issue`
   - `Create Workspace From GitHub Pull Request`
   - `Create Workspace From GitHub Review Comment`
6. Confirm existing unrelated commands are still searchable and executable.

## Primary Flow A: Issue -> Workspace

1. Open the command palette.
2. Run `Create Workspace From GitHub Issue`.
3. In the overlay, enter a valid issue reference for the active repository.
4. Submit the overlay.
5. Confirm Horizon creates a new workspace and switches focus to it.
6. Confirm the new workspace title identifies the issue.
7. Confirm the workspace contains exactly three panels:
   - `Research`
   - `Implement`
   - `Review`
8. Confirm the three panels are agent panels rather than empty/static panels.
9. Confirm each panel titlebar shows:
   - a branch badge
   - a PR-state badge
   - a waiting-status badge
10. Confirm the initial branch badge is non-empty.
11. Confirm the initial waiting status is sensible rather than blank or broken.

## Primary Flow B: Pull Request -> Workspace

1. Return to the command palette.
2. Run `Create Workspace From GitHub Pull Request`.
3. Enter a valid PR reference.
4. Submit the overlay.
5. Confirm a separate new workspace is created for the PR.
6. Confirm it also contains exactly the three role panels.
7. Confirm at least one panel shows a PR-related state that is not obviously
   generic or stale.
8. Confirm the workspace switch does not disturb the earlier issue workspace.

## Primary Flow C: Review Comment -> Workspace

1. Open the command palette.
2. Run `Create Workspace From GitHub Review Comment`.
3. Enter a valid review-comment reference or review-comment URL.
4. Submit the overlay.
5. Confirm Horizon creates a separate task workspace for that review comment.
6. Confirm the three panels are present and titled by role.
7. Confirm the created workspace still shows branch, PR state, and waiting
   badges for each panel.

## Attention Queue Checks

The task queue is driven by task attention items. Use natural agent interaction
if available, or wait for a panel to request input/review.

1. Trigger or wait for at least one task panel to request user input or review.
2. Open the attention feed.
3. Confirm the feed heading reads `Attention Queue`.
4. Confirm the task item includes:
   - the GitHub work-item label
   - the role name (`Research`, `Implement`, or `Review`)
   - an action-oriented label rather than a generic warning only
5. Click the task item or its action button.
6. Confirm Horizon navigates to the correct workspace and relevant panel.
7. Confirm non-task attention items still render and remain distinguishable from
   task queue entries.

## Panel Status Checks

1. In a task-bound workspace, note the branch badge shown on all three panels.
2. In the `Implement` panel, switch to a different git branch inside the repo.
3. Wait for Horizon to refresh panel metadata.
4. Confirm the branch badge updates to the new branch name.
5. If the PR state changes during the session, confirm the PR badge updates
   without breaking layout.
6. Confirm the waiting-status badge can show task-oriented states such as:
   - `Running`
   - `Needs input`
   - `Needs review`
   - `Blocked`
   - `Done`
7. Confirm badge rendering remains readable when the window is narrower and
   panel widths shrink.

## Usage Dashboard Checks

1. Open the usage dashboard after the task workspaces have been created.
2. Confirm the existing per-tool totals still render.
3. Confirm a task-specific section is present.
4. Confirm each task card identifies the GitHub work item.
5. Confirm each task card shows separate Claude and Codex usage rollups.
6. Confirm non-task panels do not appear as synthetic task cards.
7. If usage is still zero at first launch, interact with the agent panels and
   confirm the task cards remain stable rather than disappearing or panicking.

## Failure And Edge Cases

Run each of these and confirm the UI fails cleanly:

1. Start from a panel/workspace that is not inside a GitHub repo.
2. Run any GitHub workspace command.
3. Confirm the overlay or error path explains that no active GitHub repository
   context is available.

4. Enter an invalid issue or PR reference.
5. Confirm Horizon shows an inline error and does not create a partial
   workspace.

6. Enter a valid URL for a different repository than the active one.
7. Confirm Horizon rejects the mismatch or otherwise fails cleanly without
   creating a corrupted task workspace.

8. If possible, simulate missing `gh` authentication or revoke access.
9. Confirm the UI reports a resolution failure without crashing or hanging.

10. Cancel the overlay instead of submitting.
11. Confirm no new workspace is created and normal keyboard focus returns.

## Persistence And Restore

1. Create at least one issue-backed or PR-backed task workspace.
2. Close Horizon normally.
3. Relaunch with the same temporary `HOME`.
4. Confirm the task-bound workspace restores.
5. Confirm the restored workspace still contains the same three role panels.
6. Confirm branch, PR state, and waiting badges are still visible after
   restore.
7. Confirm the attention feed and usage dashboard still open without errors
   after restore.

## Visual Regression Checks

1. Confirm task titlebar badges do not overlap panel titles or buttons at
   default size.
2. Resize the main window narrower and confirm badge truncation remains legible.
3. Resize one task panel smaller and confirm the titlebar still paints cleanly.
4. Compare a non-task workspace and a task-bound workspace:
   - task-bound panels should show task badges
   - normal panels should not gain stray task chrome
5. Confirm the attention feed remains readable when it contains a mix of task
   items and non-task items.
6. Confirm the usage dashboard layout remains readable when task cards are
   present.

## Notes

- This feature depends on `gh` resolving GitHub references from the active repo
  context. Record the exact repository and work-item references used in the
  smoke notes.
- Leave this document in the branch as a validation artifact until the smoke
  pass is complete.
