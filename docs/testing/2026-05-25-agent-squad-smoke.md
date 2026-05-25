# macOS Agent Squad Smoke Plan

This retained smoke plan covers the Agent Squad epic from the first worktree
primitive slice through the later UI fanout slices. Run it on macOS before
marking an Agent Squad PR ready when the PR touches core worktree logic,
composer/dashboard UI, panel spawning, slot status, reviewer automation, or
`squad.json` persistence.

## Environment

- macOS with Xcode Command Line Tools installed.
- Rust stable 1.88 or newer.
- A disposable Git repository with at least one committed file.
- A clean Horizon checkout on the branch under test.
- An isolated runtime home:

```bash
export HORIZON_SMOKE_HOME="$(mktemp -d)"
export HOME="$HORIZON_SMOKE_HOME"
```

## Current Slice: Worktree Primitives

1. Validate formatting and focused tests:

```bash
cargo fmt --all -- --check
cargo test -p horizon-core worktree
./scripts/check-maintainability.sh
```

2. In a disposable Git repository, exercise the core worktree path through a
   small Rust caller or unit-test harness:
   - create slot worktrees under
     `~/.horizon/squad-tmp/<run-id>/s{1,2,3}/`
   - edit one slot worktree
   - collect its diff
   - apply that diff into `~/.horizon/squad-tmp/<run-id>/_review/`
   - remove a slot worktree and confirm the directory is gone

3. Confirm the source repository still has its original worktree intact and
   that no performer worktree was mutated by applying into `_review`.

## Current Slice: Data Model And Persistence

1. Validate the model, transition helpers, and per-session persistence:

```bash
cargo test -p horizon-core squad
cargo test -p horizon-core session_store::tests::agent_squad
```

2. Inspect a saved `squad.json` fixture from an isolated smoke session and
   confirm it contains:
   - `version: 1`
   - one or more `runs`
   - researcher and reviewer `AgentPanelLink` fields
   - performer slot `work_item`, `scratch`, `panel_local_id`, and `report`
     fields
   - `primary_worktree` when review worktree creation has run

3. Exercise one transition through `SessionStore::update_agent_squad` and
   confirm the changed run status survives a fresh `load_agent_squad` call.

## UI Shell Slices

Run these once the composer and dashboard are wired.

1. Build and launch the debug app from the branch under test:

```bash
cargo build
HORIZON_HOME="$HORIZON_SMOKE_HOME/.horizon" target/debug/horizon
```

2. Open the Squad surface from the toolbar. Verify:
   - the dashboard opens without resizing or obscuring the terminal canvas
   - `New run` opens the composer
   - the composer keeps goal text, role selectors, performer count, isolation
     mode, and auto-start toggles visible at 1280x800 and 1728x1117

3. Open the command palette and run the Squad command. Verify it focuses the
   same Squad surface rather than creating duplicate overlays.

4. Resize the window smaller and larger. Capture screenshots at launch, after
   opening the composer, and after returning to the dashboard.

## Fanout And Review Slices

Run these once Start Run, performer panels, and reviewer automation are wired.

1. Use a disposable Git repository as Horizon's active panel cwd.
2. Open Squad composer, enter a goal, set 3 performers, keep worktree isolation,
   and start the run.
3. Verify on disk:
   - `~/.horizon/squad-tmp/<run-id>/s1/`
   - `~/.horizon/squad-tmp/<run-id>/s2/`
   - `~/.horizon/squad-tmp/<run-id>/s3/`
   - `~/.horizon/squad-tmp/<run-id>/_review/`
4. In each performer panel, run `pwd` and confirm it matches that slot's
   worktree path.
5. Verify each performer panel receives the generated brief through stdin and
   that focusing each slot returns to the correct panel.
6. Mark each slot Done manually. Confirm the reviewer panel auto-spawns only
   after all slots are Done.
7. In the reviewer panel, run `pwd` and confirm it is `_review`.
8. Confirm reviewer context includes the original goal, plan text, every slot
   report, and every slot diff.
9. Restart Horizon with the same isolated home. Verify the dashboard restores
   the run, slot statuses, worktree paths, and reviewer link from
   `~/.horizon/sessions/<sid>/squad.json`.
10. Create a blocked slot and mark the remaining slots Done. Confirm Horizon
    prompts for skip blocked or retry blocked instead of auto-spawning the
    reviewer blindly.

## Regression Checks

- Slot worktree creation must never write into the primary checkout.
- Applying a slot diff to `_review` must not modify the slot worktree.
- Restarting Horizon must not duplicate existing performer or reviewer panels.
- Deleting a run must remove only that run's scratch worktrees.
- Empty diffs must be accepted without changing `_review`.
- Invalid diffs must produce a visible failure and leave `_review` unchanged.

## Evidence To Attach To PR

- The exact commit SHA tested.
- Terminal output for the validation commands.
- Screenshot of the dashboard.
- Screenshot of the composer at the smallest tested window size.
- Screenshot of the run lane with three performers.
- `find ~/.horizon/squad-tmp/<run-id> -maxdepth 2 -type d | sort` output.
- `cat ~/.horizon/sessions/<sid>/squad.json` with secrets redacted if any are
  ever added.
