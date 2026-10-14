---
procedure: tailnet-api-removal
feature: Owner-approved desktop API credential removal
platforms: [linux]
cost: none
destructive: yes
secrets: [synthetic API credential, synthetic enrollment auth key]
owner: peters
---

# Tailnet API credential removal test procedure

## 1. Purpose

Test API removal through native UI, the actual CLI plan runner and public MCP.
Make sure the exact saved network and enrollment key survive. Set, replacement
and node cleanup are outside this outcome.

## 2. Applicability

Use a production candidate with `cloud-workspaces`. Linux native evidence uses an
isolated X11 desktop and private synthetic OS store. Other desktop platforms need
separate qualification. Use the merged status adapter from the current main base.

## 3. Safety

> **CAUTION:** USE ONLY SYNTHETIC CREDENTIALS IN A TASK-OWNED PRIVATE STORE.
> Removal deletes credentials. Do not mount a host home, runtime or credential bus.

## 4. Equipment and preconditions

- Frozen production Horizon, `horizon-browser`, `cloud_deploy` and `horizon-device`.
- Exact current source hashes, dependency lock, flags, artifact hashes and ownership record.
- Fresh input comparison after a base update. Earlier Remove evidence does not qualify changed executable inputs.
- Existing trusted task equipment for a private synthetic store and isolated desktop.
- Three exact synthetic saved IDs: missing, personal-token and OAuth API bindings.
- Three synthetic enrollment-key bindings whose exact contents can be examined by the trusted fixture helper.
- A public Device viewer with actual live frames and video.

The fixture is private equipment, not a product dependency or MCP evidence.
The public viewer is read-only. Use the exact owned `horizon-device local_x11`
target for native input. Do not forge actor stamps, host identity or runtime queues.
Use at most three normal AGENT panels: an observer and two ordinary task shells.
Horizon supplies their live host/actor bootstrap. Declare frozen CLI tools as
exact read-only inputs before launch. A 30-minute native lane and 60-second CLI
observation guard bound the task; a timeout is not cancellation or rollback.

## 5. Setup

1. Run the affected source tests, full applicable matrix and static checks.

   Result: Required checks pass on the captured candidate. Advisory failures are recorded separately.

> **CAUTION:** LOAD ONLY SYNTHETIC CREDENTIALS INTO THE OWNED PRIVATE STORE.
> Host credentials and buses must stay outside this fixture.

2. Start the owned desktop and synthetic store with the frozen candidate.

   Result: The ownership record binds exact artifacts, source, accepted IDs and cleanup resources.

3. Open the public Device viewer.

   Result: Real frames show the owned native candidate.

4. Record Settings-closed CLI and public MCP status.

   Result: Public results show the three kinds. No raw secret is visible.

## 6. Tasks

### 6.1 R01 — Metadata request and owner decision

1. Run a `horizon-browser run PLAN.json` step for `tailnet_api_credential` with remove, the exact saved ID and a fresh UUID.

   Result: The actual CLI returns pending_owner. The stored credential remains. Settings does not open automatically.

2. Read the bound request through public MCP from the original normal AGENT.

   Result: The original caller can read its pending request.

3. Attempt status from the second caller.

   Result: The second caller cannot read the original caller's request.

4. Attempt cancellation from the second caller.

   Result: The second caller cannot cancel the original caller's request.

5. Open Settings > Tailnets.

   Result: The exact saved row shows its pending removal request.

> **CAUTION:** APPROVE ONLY THE EXACT SYNTHETIC API BINDING.
> Approval deletes its stored credential. Keep enrollment keys and unrelated bindings.

6. Select Approve API credential removal on the exact row.

   Result: The request becomes applied after Store completion. Kind becomes missing without closing Settings.

7. Read actual CLI and public MCP status while Settings remains open.

   Result: Both interfaces report missing. The saved network and unrelated API binding remain.

8. Close Settings.

   Result: The shared status cache retains the completed observation.

9. Repeat actual CLI and public MCP status reads.

   Result: Missing persists. Trusted fixture checks show all three enrollment bindings and contents unchanged.

### 6.2 R02 — Cancellation, native Decline and independent removal

1. Request removal for the other stored synthetic API binding.

   Result: Only a pending owner request appears. Its stored credential remains.

> **CAUTION:** CANCEL ONLY THE EXACT SYNTHETIC REQUEST.
> Cancellation prevents pending removal. Running writes cannot be cancelled.

2. Cancel that request before the owner approves.

   Result: It is cancelled and the credential remains. A cancelled request cannot start removal.

3. Request removal on an exact existing synthetic row with a fresh UUID.

   Result: The new request is independent. Its API binding can already be missing.

> **CAUTION:** DECLINE ONLY THE EXACT SYNTHETIC REQUEST.
> Decline cancels its pending operation. Keep unrelated requests unchanged.

4. Select Decline request on that exact row in Settings > Tailnets.

   Result: The new request becomes cancelled. Its credential state stays unchanged and no writer starts.

> **CAUTION:** REMOVE ONLY THE EXACT SYNTHETIC API BINDING.
> Removal deletes its stored credential. Keep its saved network and enrollment key.

5. Use the native Remove API credential button on the still-stored OAuth row.

   Result: Only its API binding becomes missing. The saved network and enrollment key remain.

6. Submit Set operations without secret arguments, then unknown secret/root/reference fields.

   Result: The actual CLI/public schema refuses them before an owner request or store mutation.

### 6.3 R03 — Failure ownership and close transitions

1. Run deterministic source regressions for blocked, failed and disconnected read/write replies through Settings close.

   Result: A fresh read clears reader failures only. It cannot erase a failed removal, catalog or save error.
   Known per-ID errors remain unavailable during close/replacement reads; removed-ID errors do not return.

2. Run the late-result and removed-catalog-row receipt regressions.

   Result: The original caller reads a bounded late result. No receipt lookup grants a new write or another actor access.

> **CAUTION:** DISABLE ONLY THE OWNED PRIVATE SYNTHETIC STORE.
> Store interruption blocks its reads and writes. Keep host stores and buses intact.

3. Make the owned private store unavailable.

   Result: Only this fixture's credential service is interrupted.

4. Record actual CLI and public MCP status.

   Result: Public status reports unavailable, not missing.

> **CAUTION:** RESTORE ONLY THE OWNED PRIVATE SYNTHETIC STORE.
> Restoration reopens its stored bindings. Do not connect a host credential bus.

5. Restore the owned private store.

   Result: Only this fixture's credential service becomes available.

6. Refresh its API status in Settings.

   Result: Recovery preserves unrelated errors and needs no Settings close.

7. Record actual CLI and public MCP status again.

   Result: Both interfaces show the recovered current kinds or completed removal.

### 6.4 R04 — Presentation

1. Capture fresh screenshots before each native action.

   Result: Input uses current geometry from the exact owned native target.

2. Resize the owned native candidate window smaller.

   Result: Native controls remain usable. The viewer shows the actual resized window.

3. Select workspace Fit.

   Result: The candidate workspace fits its smaller window.

4. Resize the owned native candidate window larger.

   Result: The viewer shows the actual enlarged window.

5. Select workspace Fit again.

   Result: The candidate workspace fits its larger window.

6. Record a short GIF from real video for the PR attachment.

   Result: The GIF shows owner approval and missing status. Do not commit it in the repository.

## 7. Pass criteria

All interfaces use the same removal policy and exact saved-ID binding. Requests
cannot authorize their own mutation. Enrollment keys and sibling bindings survive.
No Set capability, secret getter, remote API validation or node deletion is claimed.
Source tests do not replace actual native owner and public-route evidence.

## 8. Cleanup

1. Reconcile any unfinished writer with its original receipt before cleanup.

   Result: An unresolved result remains preserved. No broad namespace cleanup is used.

> **CAUTION:** EXIT ONLY THE OWNED NATIVE CANDIDATE.
> Exit can hide an unresolved write result. Reconcile its original receipt first.

2. Exit the native candidate normally.

   Result: The candidate exits with code zero. A forced stop or timeout cannot qualify normal-exit cleanup.

> **CAUTION:** CLEAN ONLY THE EXACT OWNED SYNTHETIC FIXTURE.
> Cleanup deletes synthetic credentials and fixture data. Preserve unresolved operations and all host bindings.

3. Complete the existing ownership-bound fixture cleanup.

   Result: Accepted synthetic services are empty. Owned children stop, the target expires and the relay exits normally.
   The healthy original daemon reports the exact container absent.

4. Close the public viewer.

   Result: Its owned viewing session is closed.

5. Preserve manifests, public responses, CLI results, images and video.

   Result: No host credential or provider resource changed.

## 9. Record of results

Record source and artifact identity, new/reused/non-applicable checks, actual
public responses, native frames and exact cleanup in a report under
`docs/testing/reports/`. Mark pending or failed lanes accurately. Private fixture
state is setup/cleanup evidence, not a substitute for public MCP results.
