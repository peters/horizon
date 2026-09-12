# Check saved Stop — temporary Linux validation plan

Status: implementation draft; no candidate binary, native result or cloud proof is bound yet.
Remove this temporary plan only after the exact final candidate's UI validation and archive.

## Outcome and exclusions

An explicit **Check saved Stop** observes one existing persistent RunPod Stop intent.
It does not send Stop, allocate/recover a worker, delete a resource, or use a private
SSH key. Matching retained-stopped proof may write local `Stopping` → `Stopped`.
Pending/absent results retain the saved identity, intent and original timestamps.
Provider status remains a distinct, strictly read-only overview action.

RunPod configured checks are Linux-only in this slice. LocalDocker Stop is unchanged;
Azure and timed Stop confirmation are not offered. A saved public pin is validated,
not live-attested. No task success, billing cessation, process memory, durable bytes,
or completed #473 cloud campaign may be inferred from this UI pass.

## Ownership, source and process preconditions

- Use only the reviewed candidate in `issue-473-check-saved-stop`, at the final
  committed head. Record head/tree, clean status, build log, binary digest and inputs.
- Reuse the reviewed public-only overview harness pattern from
  `/tmp/horizon-473-runpod-observation-ui-smoke`, in a fresh task-owned namespace.
  Do not execute old consumed launch/fixture/cleanup scripts or change old evidence.
- A future helper must be independently reviewed before execution. Use one fresh
  private home and exact owned display/window manager, with a bounded GUI supervisor.
- No real provider credential, private SSH key, cloud API, Docker resource, registry
  credential, user desktop or pre-existing Horizon process belongs to this pass.
- Launch with a clean child environment, excluding `RUNPOD_API_KEY`, SSH agents and
  `HORIZON`. Do not change the user's environment or configuration.
- Seed synthetic public retained allocation/request/pin/HPS state using public store
  APIs. Save an existing `Stopping` intent; never call a provider to manufacture it.
  Keep fixture seeding separate from the actual candidate GUI check under test.
- Record exact fixture rows, original Stop time, public identity, selected profile/HPS,
  and absence of a private key. Use sanitized identities and no executable saved task.
- Capture only the candidate PID's window. Close normally; stop only the display/WM
  created by this harness after confirming their exact identities.

## Deterministic core and UI gates (no network)

1. Run all Stop core tests, including the actual generic confirmation coordinator
   behind the configured provider-construction seam. Cover ordinary/HPS selections,
   Pending, Absent, provider failure, RetainedStopped, and repeated saved Stopped.
2. Assert no credential callback/provider action on unsupported provider/platform,
   missing intent, timed lifetime, absent pin, stale row, invalid profile/storage,
   incompatible data center or unsafe timestamp.
3. Mutate the full workflow and separate immutable-selection fixture at credential
   and completion callbacks; confirm StateChanged wins and no wrong completion is saved.
4. Assert exact allocation/identity retention and no timestamp renewal. Generic
   lifecycle Stopped is not a substitute for the retained-stop observer contract.
5. Check UI single-flight, no paint/open/reopen dispatch, config/selection/close
   invalidation, no cross-kind callback success, and notice retention through its
   own saved-page refresh. Reject changed generation/revision/request time/results.
6. Missing/insecure store checks must not create or repair the fixture store. A
   successful completion intentionally requires a writer; do not claim no WAL/schema
   bookkeeping or immunity to same-user concurrent filesystem tampering.
7. Run fmt, maintainability, version-sync, default/speech workspace tests and all
   three Clippy tiers in the final candidate checkout. Preserve every failed log.

## Native public-only lane

1. Launch the exact candidate at 1200×900; save/view initial screenshot. Open Remote
   Environments and select the synthetic Stopping row. Check the exact profile,
   worker identity and saved phase; the Stop mutation button remains disabled.
2. Confirm **Check saved Stop** is enabled, with disclosure that it sends no Stop and
   may save completion. Provider status remains separate and says it changes no state.
3. Before clicking, idle and move the pointer across empty modal space. A bounded
   child-only trace must show no Stop-check background thread or SSH/provider call.
4. Click Check once. With no API key, expect the fixed credential-unavailable error;
   no success or absence is inferred. Bind the exact action, screenshot and thread
   count. Wait until this dispatch settles before testing unrelated input.
5. Repeat once explicitly: exactly one additional check, same original Stop time,
   unchanged allocation/HPS/identity and no private-key file or credential output.
6. Refresh the saved page, change selection and return, close/reopen the overview:
   no implicit check or Stop is dispatched. A discarded late result cannot attach
   to another selected record. Local synthetic tests cover controlled late callbacks.
7. Resize to 960×720 and inspect readable disclosure, button, fixed error and scroll
   behavior. Close the modal, test Fit appropriate to the actual board; empty-board
   disabled Fit is only an inert-control proof, not populated-board fitting.
8. Close the exact GUI through the normal window-manager close path. Verify candidate
   exit and exact owned display/WM cleanup, unchanged committed fixture rows and no
   private key. Retain hashes, trace, screenshots and final reviewed receipt.

## Positive proof boundary

The no-credential native lane verifies actual UI wiring/refusal and no implicit replay.
The deterministic synthetic tests verify successful local completion and three-way
presentation. They do not prove a current provider reports retained-stopped state.
Any later live lane requires separately bound exact owned worker/profile/HPS, retained
public pin, original intent and read-only provider authorization. It must call only
Check, never Stop again or automatic recovery, and may not reuse cleaned H200 resources.
