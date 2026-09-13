# Saved-connection refresh: temporary Linux UI smoke plan

Status: planned, not executed. Remove only after accepted final-head native proof.
This is one explicit configured RunPod/HPS action, not Start or automatic reconnect.
The implementation is initially stacked on the frozen Delete UI dependency; after
that dependency merges, validate the exact final source and record source parity.

## Preconditions and isolation

1. Record final worktree, clean head/tree, eight source/test path hashes, Cargo
   hashes, debug GUI and fixture hashes, and the full-matrix evidence separately.
2. Use only disposable synthetic homes and fixtures built from that exact source.
   Do not read/copy real profiles, session data, tokens, identities or saved Pods.
3. Create a fresh unused owned X11 display/WM; identify GUI by exact PID, executable
   and process start time. Never target by application name or touch existing GUIs.
4. Isolate internet networking; remove ambient cloud/Git/SSH credentials for the
   child. No provider, SSH or credential operation is authorized in this smoke.
5. Use normal WM_DELETE_WINDOW, not opaque keyboard shortcuts, within a recorded
   deadline of at most 12 minutes per theme. Preserve failures; no silent replay.
6. Persist the original scoped baseline before launch: config/DB/WAL/SHM hashes,
   permissions/link counts, semantic saved rows and process/display identities.
   Classify only expected private startup/plugin/cache additions separately.

## Deterministic tests before native

- Core admission covers exact profile/HPS/worker/public pin and allowed phases,
  private identity before lazy key lookup, concurrent drift and no-op/+1 revision.
- UI covers coarse eligibility, consumed unchecked consent, Cancel, changed
  home/config/summary, missing-store no-creation refusal and result identity checks.
- Synthetic channel completions cover receiver retention while closed, discarded
  stale/lost results, one deferred inventory refresh and no automatic retry.
- Test central dispatch guards, not only disabled paint: Delete, Start, Stop,
  observation, setup, repository and reconnect cannot overlap endpoint refresh.
- Native cannot prove these asynchronous/provider outcomes; use deterministic
  tests for them and do not present synthetic results as live cloud proof.

## Native campaign: dark Ready and light Starting

1. Launch exact debug binary with disposable config, --blank and --ephemeral.
   Record screenshot and synthetic empty-board baseline at 1200x900.
2. Open saved inventory once using the existing public-API synthetic 15-row fixture.
   Candidate dark selects RunPod Ready; candidate light selects RunPod Starting.
   Summary eligibility is not proof of actual HPS admission or running readiness.
   Negative provider/phase permutations and stale/concurrent results remain
   deterministic evidence unless separately recorded as actually exercised.
3. On that phase's selected row inspect the new action beside preserved
   Start/Stop/Delete controls. Do not claim native coverage of all fixture rows.
4. Open Refresh saved connection. Read full workspace, owning session, profile and
   Pod identity; original retained host/client key and /usr/bin/true disclosure;
   coordinate-only saving, no key discovery/rotation, no compute/task/reconnect,
   preserved Start intent, interruption/uncertainty and no durability promise.
5. Consent starts unchecked; operational button disabled. Cancel and reopen must
   reset consent. If acknowledgement is toggled for keyboard testing, never click
   Authenticate and save connection. Enter/Space must not submit that button.
6. Cancel, select a different row and reopen; identity and consent are fresh.
   Close/reopen the overview; no operation resumes automatically.
7. Resize exact window to 800x600. Scroll to inspect every wrapped identity and
   disclosure, checkbox and reachable Cancel. Capture top and lower portions.
   Cancel, restore 1200x900, refresh saved inventory once and inspect controls.
8. Passive empty-board and overview idle, pointer and resize samples must produce
   no remote dispatch beyond explicit inventory clicks. Use the matched dark
   measurement lanes below; light need not repeat the performance comparison.
9. Normal-close exact GUI. Verify successful exit, owned children/display cleanup,
   unchanged original protected files/semantic rows and absence of operational
   child/network traces. If a preexisting PID becomes unobservable, qualify it;
   never infer its fate or signal it to repair the evidence.

## Matched redraw measurements

Run a short, separately authorized dark baseline from the immutable prior Delete
debug binary, then the final candidate dark binary with identical synthetic fixture,
software GPU, 1200x900 geometry, selected Ready row, focus and scroll position.
Both use 5s empty-canvas idle and 20 alternating empty-pointer moves; then 5s
overview idle and 20 identical non-operational header/chrome pointer moves.
Record CPU ticks, monotonic elapsed time and ticks/second; screenshots follow each
sample and are excluded from timing. Each baseline/candidate needs fresh private
home, display, intent and normal-close proof; consumed old samples are not reused.
The baseline lacks the endpoint control. Compare shared surfaces, not nonexistent
baseline consent behavior. Record exact source/binary hashes and intervening main
changes: this is not a source-identical A/B experiment. Qualify short samples and
host load; software-GPU pointer CPU alone proves neither regression nor improvement.

## Acceptance and handoff

Record each lane PASS/FAIL/omitted, screenshots and hashes, actual dispatch counts,
close receipt, baseline comparison and explicitly classified added files. Preserve
original failed runs separately from any approved post-audit or fresh campaign.
No cloud readiness, durable-storage, account-visibility or PC-off claim follows.
Never execute operational confirmation, Start/Stop/Delete, SSH or cloud commands.
After native acceptance remove this temporary plan, prove source/Cargo parity,
run the exact-final-worktree matrix and obtain independent and hosted reviews.
