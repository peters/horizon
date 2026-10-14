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
- Trusted `timeout`, `flock` and `sleep` commands in the fixture terminal.
- The existing regular file `/fixture/state/horizon/cloud/tailnets.lock`, owned by the fixture UID.

The fixture is private equipment, not a product dependency or MCP evidence.
The public viewer is read-only. Use the exact owned `horizon-device local_x11`
target for native input. Do not forge actor stamps, host identity or runtime queues.
Use at most three normal AGENT panels: an observer and two ordinary task shells.
Horizon supplies their live host/actor bootstrap. Declare frozen CLI tools as
exact read-only inputs before launch. A 30-minute native lane and 60-second CLI
observation guard bound the task; a timeout is not cancellation or rollback.

Record the private fixture runner path, source hash and exact ownership file before launch.
The existing runner supports `keyring-off` and `keyring-on` with `--ownership`.
Use these controls only for the accepted fixture. No new helper program is required.

The Linux Store uses `File::lock` on `tailnets.lock`, which uses `flock` on this platform.
Use its existing inode, not a replacement or symlink. The command below holds it for 45 seconds.
Its outer timeout is 50 seconds. Record actual lock acquisition before native input.
If the lock expires too early, record an incomplete attempt, not a passing close/reopen check.

The bounded-write sequence below defines the qualification requirement.
Use a completed report to establish the result for the exact tested inputs.

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

> **CAUTION:** LOCK ONLY THE OWNED FIXTURE STORE.
> The lock blocks its reads and writes. Keep host stores and unrelated processes outside this lane.

6. Start the bounded lock command in the fixture terminal.

   ```sh
   timeout --signal=TERM 50s flock --verbose --exclusive --timeout 2 /fixture/state/horizon/cloud/tailnets.lock sleep 45
   ```

   Result: The command holds the exact Store lock. Its acquisition output precedes the next native action.

> **CAUTION:** APPROVE ONLY THE EXACT SYNTHETIC API BINDING.
> Approval deletes its stored credential after the lock clears. Keep enrollment keys and unrelated bindings.

7. Select **Approve API credential removal** on the personal-token row.

   Result: The writer waits for the lock. Its request reports running, not applied.

8. Close Settings before the lock expires.

   Result: The original writer and its read barriers remain retained.

9. Reopen **Settings > Tailnets** before the lock expires.

   Result: The row remains pending. No independent reader reports the old stored kind as a completed write.

10. Wait for the lock command and original removal request to finish.

   Result: Both finish within the lane guard. The original request reports applied after Store completion.

11. Read actual CLI and public MCP status while Settings remains open.

   Result: Both interfaces report missing. The saved network and unrelated API binding remain.

12. Close Settings.

   Result: The shared status cache retains the completed observation.

13. Repeat actual CLI and public MCP status reads.

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

> **CAUTION:** LOCK ONLY THE OWNED FIXTURE STORE.
> The lock blocks its reads and writes. Keep host stores and unrelated processes outside this lane.

5. Start the same bounded lock command from R01 in the fixture terminal.

   Result: Its acquisition output precedes the next native action. The OAuth API credential is still stored.

> **CAUTION:** REMOVE ONLY THE EXACT SYNTHETIC API BINDING.
> Removal can delete its stored credential after the lock clears. Keep its saved network and enrollment key.

6. Select **Remove API credential** on the still-stored OAuth row.

   Result: The native writer waits for the lock.

7. Close Settings before the lock expires.

   Result: The original writer remains retained.

> **CAUTION:** STOP ONLY THE OWNED SYNTHETIC SECRET SERVICE.
> Its API removal will fail. Keep host services and buses intact.

8. Run the declared private fixture control before the lock expires.

   ```sh
   python3 "$FIXTURE_RUNNER" keyring-off --ownership "$OWNERSHIP"
   ```

   Result: Its ownership check accepts only the exact fixture. Its recorded service process stops.
   Do not treat a control request alone as proof of service shutdown.

9. Reopen **Settings > Tailnets** before the lock expires.

   Result: The existing writer remains pending. The reopened UI starts no competing status reader.

10. Record the pending native UI and the original request state.

   Result: The retained operation still reports running before the lock clears.

11. Wait for the bounded lock command and native writer to finish.

   Result: The native UI shows the removal failure after completion. Failure does not guarantee rollback or preservation of the API credential.

12. Submit Set operations without secret arguments, then unknown secret/root/reference fields.

   Result: The actual CLI/public schema refuses them before an owner request or store mutation.

### 6.3 R03 — Failure ownership and close transitions

1. Run deterministic source regressions for blocked, failed and disconnected read/write replies through Settings close/reopen.

   Result: A fresh read clears reader failures only. It cannot erase a failed removal, catalog or save error.
   Known per-ID errors remain unavailable during close/replacement reads; removed-ID errors do not return.

2. Run the late-result and removed-catalog-row receipt regressions.

   Result: The original caller reads a bounded late result. No receipt lookup grants a new write or another actor access.

3. Record the reopened native UI and actual CLI/public MCP status after the R02 failure.

   Result: The UI retains the failed write. Public status reports unavailable, not a successful missing observation.

> **CAUTION:** RESTORE ONLY THE OWNED SYNTHETIC SECRET SERVICE.
> Restoration reopens its bindings and permits recorded cleanup. Do not connect a host credential bus.

4. Run the declared private fixture control to restore its Secret Service.

   ```sh
   python3 "$FIXTURE_RUNNER" keyring-on --ownership "$OWNERSHIP"
   ```

   Result: Only the exact fixture service becomes available.

5. Select **Refresh** in **Settings > Tailnets**.

   Result: A fresh status read clears its read failure. The separate API write error remains visible.
   Recovery can finish recorded cleanup and report missing. This does not turn the failed write into a successful receipt.

6. Record actual CLI/public MCP status and the reopened native write error.

   Result: Public kinds match the recovered store. All three enrollment bindings, their exact contents and the saved catalog remain unchanged.

7. Request removal for the same saved OAuth row with a fresh UUID.

   Result: A new pending owner request appears even if recovery reports missing. The request itself starts no writer.

> **CAUTION:** RETRY ONLY THE EXACT SYNTHETIC API REMOVAL.
> The retry can delete remaining API data. Keep enrollment keys, saved networks and unrelated bindings.

8. Select **Approve API credential removal** for that fresh request.

   Result: A new native owner decision starts the retry. It does not reuse the failed operation's authority.

9. Wait for the fresh request to report applied.

   Result: The successful native retry clears its write failure. Read recovery alone did not clear that error.

10. Record actual CLI/public MCP status and all three enrollment bindings.

   Result: The exact API binding is missing. Enrollment binding slots and contents match their original private snapshots.

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
