# Provider API v2 migration smoke

Temporary validation plan for #956. Record the final commit, executable hashes,
image indices and embedded helper hashes before execution. Every behavior-changing
push invalidates affected smoke evidence. Keep evidence private and publish only
sanitized results. This plan qualifies the migration PR, not every outstanding item
in #813/#790. The current base includes the worker updater fix from #945. Rebuild
all worker images after rebasing; earlier image hashes do not qualify the new base.

## Preconditions and evidence

- Run the complete AGENTS.md local matrix in the final worktree, resolve independent
  review findings, and freeze the actual executables. Record all lane exit codes.
- Use only synthetic repositories with two independent worktrees, deterministic small
  test tasks, and generic display names. Never publish account/resource identifiers.
- Use task-owned private settings and explicit credential bindings. Exercise two
  separate bindings without claiming two copies of one key provide account isolation.
- Inspect the v2 catalog and account read-only. A CPU run needs an empty fully
  paginated Serverless endpoint list, compatible standard storage, and fresh stock
  for its exact CPU size. A GPU run must have actual compatible GPU capacity.
- Prepare one bounded worker operation at a time, with an independently reviewed
  cleanup plan, durable identities, deadline and cleanup service armed before create.
  Use the standing cloud authorization; do not reuse consumed operation identities.
- Run the isolated native fixture from `scripts/device-smoke/README.md` with private
  application state and `--native-view`. Attach its exact loopback endpoint through
  public `device_panel` in the current workspace. Verify displayed, advancing frames.
- Record the isolated display continuously from before launch through deletion.
  Capture screenshots after launch and resize/Fit. Verify actual child PID/hash,
  recorder health, finalized recording, full decode and representative moving frames.
  Never control or restart the developer's desktop/Horizon to repair a test viewer.
- Browser interaction uses only the horizon-browser public tools. Native input uses
  the fixture's explicit device target. Missing tool discovery or presentation blocks
  the affected lane; do not substitute browser viewers or private endpoints.

## Automated contract and failure tests

Run `cargo test -p horizon-cloud` and the workspace suite. Require coverage for:

1. v2 create bodies for CPU/GPU, resource minima, owned network mount versus Pod
   persistent storage, immutable image and registry generation, and STANDARD tier.
2. Ordered compute fallback only after definite refusal. Timeout, malformed success,
   cancellation after submission, lost persistence and server failure retain fences.
3. Requested/Bound/Terminated legacy journals: no implicit replacement, adoption of
   foreign identity, fabricated freshness, or loss of cleanup access.
4. PROVISIONING and STARTING bind identity and continue readiness on the same worker;
   ERROR/EXITED/unknown responses cannot be called Ready. Structured direct SSH is
   used; provider command strings and proxy-only connections are not executed.
5. Complete Pod pagination with cluster members, empty intermediate pages, encoded
   cursors, duplicates and cursor cycles. An incomplete list cannot prove absence.
6. Exact-size CPU capacity and allowed location/flavor preference; malformed capacity
   and insufficient memory/disk refuse allocation rather than downgrade the profile.
7. Registry creation, rotation, selected generation and revocation; unresolved POSTs
   cannot be repeated, wrong identities cannot be revoked, error bodies cannot leak
   either compute credentials or submitted registry secrets.
8. CPU volume admission/bootstrap/deletion refuses any Serverless endpoint, including
   later-page endpoints with no current mounts or active workers. Auth/schema failures
   stay fail-closed. Cluster/other Pod attachments and unknown mounts prevent deletion.
9. Image update sends only intended fields and preserves uncertainty across restart.
   Changed identity/mount and mismatching observations cannot complete a replacement.
10. Billing remains v2 and legacy journal serialization remains compatible. UI restore
    resumes readiness on the bound provisioning worker and respects explicit stop.

## Live CPU profile and interface matrix

Build immutable full, minimal, selected-agent and browser-only images from the exact
candidate using their normal recipes and version pins. Verify the image contracts,
uncached/cached builds and private registry pushes/pulls. Record command durations
separately from cold transfer, provider allocation, SSH readiness and application use.

For each profile, use its supported UI/CLI/MCP entry point and inspect the same saved
operation through the other interfaces. Across the runs, exercise deployment through
all supported interfaces and record the public commands/tool names and outcomes.

1. Prepare normal first-use settings and correct a missing/invalid binding before
   allocation. Verify cancellation does not create a worker or lose submitted intent.
2. Create once. Observe PROVISIONING/STARTING/RUNNING, then actual SSH, assigned CPU,
   memory, container disk, exact owned volume/mount and runtime contract readiness.
   Record the earliest failing stage if the endpoint disappears; do not replay POST.
3. Verify the full/minimal/selected/browser capabilities match each profile. Missing
   optional capabilities should be explicit; minimal is not expected to run agents.
4. On full/selected profiles, start two real managed agent sessions in different
   synthetic worktrees. Perform independent edits and tests. Record process start
   identities, executable versions/hashes, file hashes and test results. Exercise
   configured normal authentication; never bypass agent sandbox/runtime protections.
5. On browser profiles, discover and use both supported browser engines through public
   MCP tools, navigate a synthetic page, inspect DOM, perform input, and verify output.
   Exercise the corresponding CLI path and UI status. Native desktop must display
   changing output through the actual Device panel, including resize/Fit.
6. Normally close and restart only the isolated client. Restore the same worker,
   volume, two sessions, browser and desktop panels. Compare process/worktree identities
   and rerun tests. No extra worker allocation or re-created files may substitute.
7. Exercise explicit stop/start on the same worker and reconcile from UI/CLI/MCP.
   Record expected process loss versus client-only reconnect preservation separately.
8. Check current cost and billing behavior without presenting an estimate as billed
   total. Observe the same bound worker through every supported status interface.
9. Delete through the supported lifecycle. Verify exact Pod and volume authenticated
   absence, final durable states, no task processes/tunnels/viewers, and generation
   revocation plus issuer token removal. Keep original uncertain operations untouched.

## Live GPU and replacement/recovery lanes

- Build and run the actual GPU profile on a GPU worker. Verify GPU type/count, CPU/RAM
  minima, disk/persistent mount, CUDA compiler/runtime and a small deterministic CUDA
  workload. CPU rendering or a CPU build cannot qualify the GPU lane.
- Reconnect the isolated client and verify the same GPU allocation and persisted
  workspace. Stop/start only when the test's exact bounded plan covers it; loss of
  capacity on restart is a failed lane, not permission for an automatic replacement.
- Rebuild to a second immutable image and replace on the same worker. Verify the
  actual container/process restart, new executable/image identity, preserved workspace,
  re-established capabilities and fresh sessions. A changed API image string alone
  cannot prove the new container is running.
- Exercise cancellation and failed pull with a task-specific credential/image. Retain
  update intent and distinguish definite rejection from uncertain application. Recover
  through the documented existing-worker path and verify the final image and mount.
- Exercise an interrupted client after the create fence is durable. Reconcile the
  same identity; do not intentionally create an uncontrolled ambiguous paid request.
  Fault-inject timeout/5xx/lost responses in mocks for the dangerous no-replay paths.
- Finish each run with exact resource/credential cleanup and decoded video evidence.
  Any unresolved outcome remains open and fenced; do not claim account-wide absence.

## Reporting and merge gate

Update #956 with completed checks and precise remaining lanes. Link the migration PR
and its exact candidate. Keep #813 and #790 open for their remaining qualification.
Require final-head review approval, no unresolved actionable threads, all applicable
CI and successful required smoke before the authorized squash merge. Verify the squash
commit and post-merge workflows. Remove this temporary plan after the UI validation
pass; preserve the completed private report. Merge authorization is not release approval.
