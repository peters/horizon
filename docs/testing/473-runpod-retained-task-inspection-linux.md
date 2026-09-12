# Retained RunPod task inspection — Linux smoke

Temporary validation plan; remove after the applicable local UI pass. This change
enables the existing **Environments → Show session panels → Check retained task**
action for already retained persistent RunPod workers. It does not create, start,
reconnect, stop, delete or repair anything, and does not establish repository or
saved task-intent readiness. Observations are point-in-time, not monitoring.

## Preconditions and containment

- Freeze and record the exact candidate commit, tree, binary SHA and clean source.
  Use its debug binary; never substitute a modified provider endpoint or binary.
- Use fresh task-owned private state, synthetic credentials and fixtures. Do not
  read the user's config, keys or environment credentials. Unset `HORIZON` for
  the test app and use a disposable explicit shell, not an inherited agent.
- Launch on a verified-unused virtual display with a task-owned window manager.
  Track exact app/display/WM processes and newly owned container IDs. Never
  signal or automate pre-existing sessions. Screenshots must use unused paths.
- Use existing reviewed image/fixture patterns with fresh intents; do not replay
  consumed harnesses. A LocalDocker fixture is not positive RunPod cloud proof.
- Real RunPod observation requires separate approved access to an exact retained
  persistent worker, complete saved host pin/key and immutable storage selection.
  No allocation, credential registration, task start or cleanup authority follows
  from this plan. Do not repair a missing pin/key or adopt a discovered worker.

## Focused no-network checks

Run the core configured status and UI reopen inspection tests. Require actual
nonzero counts. Cover current owner/summary/profile/panel admission, management
intent, missing retained worker/pin/key, invalid target/DC, credential refusal,
post-credential allocation drift, exact selected-volume provider binding, and
expired setup retention on a persistent worker. Tests inject credentials; none
may read or modify global environment variables or make provider calls.

Assert fixed redacted errors and unchanged allocation, retained key, selection
and database bytes. Status inspection must not impose Shell launch-intent
requirements, create/reconcile a missing worker, renew a setup window or grant
task-start/reconnect authority. Existing provider/SSH suites retain coverage for
actual attachment mismatch, pinned handshakes and bounded protocol failures.

## Native local UI lanes

1. Baseline: launch the exact candidate with isolated state. Open Environments,
   select a saved row, then Show session panels. With no selected row, no task
   action is admitted. Verify labels fit at launch and after window resize/Fit;
   inspect screenshots, not just build success.
2. RunPod negative: an explicitly seeded retained synthetic RunPod row with no
   controller credential must show the fixed missing-credential error on Check
   retained task. Verify no network/provider access and no new local panel.
   Missing-profile/key/pin and foreign-owner cases remain deterministic tests if
   not represented by separate native fixtures; label that distinction honestly.
3. Genuine loopback SSH: use a task-owned LocalDocker worker with a retained key
   and host pin. Check one running task, one exited task (known exit status 7),
   and one unavailable marker. Verify exact selected panel ID and UTC check time.
   No shell/task execution may result from inspection. Preserve a separate
   independently progressing task and a dirty-file sentinel throughout.
4. A second explicit check may update the timestamp. Idle and pointer-only
   frames must not re-query, animate or request continuous repaints. Checking
   must not create/reconnect a local view, dirty session runtime or restart an
   exited task. Compare worker execution counts and store/key bytes.
5. After a successful check, induce a task-owned transport failure and check
   again: previous status remains visibly stale, not silently fresh. Unknown
   exit status must never display exit code zero. Retry only explicit reads.
6. Change selected row/session/provider profile while a check is pending; late
   results must be discarded. Close the overlay while pending and verify no
   stale result reopens it. No Stop/Delete buttons should be activated.
7. Close the exact app normally. Verify worker task continuity, unchanged dirty
   data and zero unintended lifecycle actions. Clean up only journaled task
   fixtures under separately reviewed exact-ownership guards.

## Actual RunPod acceptance and receipt

The positive RunPod adapter → pinned SSH → GUI status path remains pending until
performed against the exact approved cloud worker. LocalDocker plus synthetic
RunPod admission proves shared UI/SSH behavior, not real provider integration.
If available, repeat running/exited/no-replay checks on that retained worker,
including its exact storage binding; never interpret metadata as filesystem
durability, full-volume-loss recovery or cross-session ownership proof.

Record candidate/source/image bindings, commands, actual test counts, screenshots
and independent visual review, before/after data/process/store evidence, cleanup
and every unexecuted lane. Run the required full eight-gate local matrix on the
final commit before push. Keep completed evidence private outside the repository.
