# Worker runtime version stability

Candidate: record the exact branch commit and immutable image digest before testing.
Use synthetic task-owned worktrees, private credentials, an isolated native desktop
and a live Horizon native VNC Device panel. Cloud allocation requires a current
explicit spending/time authorization and cleanup armed before allocation.

1. Run the worker-script suite. The launcher regression must execute its child with
   a minimal environment and with an inherited `DISABLE_AUTOUPDATER=0`, observing
   `1` in both cases. Preserve normal permissions, MCP arguments, credential
   bindings and recorded exit status. Version-mismatch readiness tests must pass.
2. Build the selected-agent/full image from the candidate with fixed agent version
   and helper binaries. Verify its image contract and selected runtime versions.
3. Start a real managed session through the normal SSH/tmux path. Inspect only that
   child's environment and run `claude doctor` within the same environment. Confirm
   background updates are disabled and capture executable hash/version.
4. In two managed sessions, edit/test independent synthetic worktrees. Capture the
   worker ID, process IDs/start times, worktree revisions, file hashes and passing
   test results. Recheck the agent executable/version and stock readiness.
5. Normally close and restart only the isolated client. Require the same worker,
   both real agent panels, desktop and browser panels to restore without duplicate
   allocation. Compare process/worktree identities and changes; repeat tests.
6. Inspect native frames after launch, resize and Fit; record the scenario and
   decode representative frames. Save precise application/reconnect timings.
7. Clean up exact worker/storage resources and task credentials, with authenticated
   absence proof. Preserve uncertain allocation journals and their cleanup access.

The earlier live failure used a different frozen candidate. Its record does not
qualify this fix. Initial launch-fence EDQUOT and first browser-start failure remain
separate findings; do not bypass their safety fences to complete this plan.
