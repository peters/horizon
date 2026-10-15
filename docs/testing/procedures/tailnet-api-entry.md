---
procedure: tailnet-api-entry
feature: Masked native owner API credential entry and replacement
platforms: [linux]
cost: none
destructive: yes
secrets: [synthetic API credential, synthetic OAuth client secret, synthetic enrollment auth key]
owner: peters
---

# Tailnet API credential entry test procedure

## 1. Purpose

Test empty masked entry, replacement and metadata-only owner requests through
native UI, the actual CLI plan runner and public MCP. Preserve the enrollment key
and other saved bindings. Stored kind proves local parsing only; this procedure
does not validate remote API authority or delete tailnet nodes.

## 2. Applicability

Use the production candidate with cloud-workspaces after the status and removal
prerequisites. Source tests or prior prototype results alone do not qualify this
outcome. Native evidence covers Linux; other desktops need separate qualification.

## 3. Safety

> **CAUTION:** USE ONLY SYNTHETIC VALUES IN AN OWNED PRIVATE OS STORE.
> Replacement retires the previous API slot. Do not mount host home, runtime,
> credential bus or provider configuration. Keep cleanup within the owned fixture.

## 4. Equipment and preconditions

- Frozen production Horizon, horizon-browser, cloud_deploy and horizon-device.
- Exact source tree, consumed hashes, dependency lock, flags and artifact hashes.
- Existing trusted isolated X11/private-store equipment with an accepted-ID ledger.
- Exact synthetic missing, personal-token and OAuth saved bindings, plus an independent enrollment key.
- A public Device viewer with live frames and video; exact owned horizon-device local_x11 target for input.

Use at most three normal AGENT panels: an observer and two task shells. Declare
frozen CLI tools as read-only launch inputs. Horizon supplies their live actor and
host identity. Do not inject actor stamps or runtime queues. The viewer is
read-only; native input uses the owned device target. Bound the lane to 30 minutes
and CLI observation to 60 seconds. Timeout does not cancel or prove rollback.
Private setup/cleanup equipment supplies synthetic state, not MCP evidence.

## 5. Setup

1. Run affected source tests, the full applicable matrix and static checks.

   Result: Required checks pass on captured inputs. Advisory findings stay separate.

> **CAUTION:** LOAD ONLY SYNTHETIC CREDENTIALS INTO THE OWNED PRIVATE STORE.
> Host credentials and buses must stay outside this fixture.

2. Start the owned desktop and private store with exact frozen artifacts.

   Result: The ownership record binds the candidate, accepted IDs and cleanup resources.

3. Open the public viewer and record initial CLI/public MCP status with Settings closed.

   Result: Actual frames and public responses show the three synthetic kinds.

## 6. Tasks

### 6.1 E01 — Personal-token request and explicit owner Save

1. From a normal AGENT, run a horizon-browser plan for tailnet_api_credential with set_personal_token, the exact missing saved ID and a fresh UUID.

   Result: The real CLI returns pending_owner. Settings does not open and no binding changes.

2. Read the same request through public MCP. Try another caller and extra token/root/reference fields.

   Result: Only the original caller can read its receipt. Secret fields and retargeting are refused.

3. Open Settings > Tailnets and choose Enter requested API credential on that exact row.

   Result: The token field is empty and masked. The requested kind cannot change.

> **CAUTION:** SAVE ONLY THE EXACT SYNTHETIC API BINDING.
> Save can replace its stored value. Keep the enrollment key and unrelated bindings.

4. Enter a synthetic API token and select Save API credential. Read the bound request and current kind through actual CLI and public MCP while Settings remains open.

   Result: Applied follows Store completion. Personal-token kind reaches discovery without closing Settings. The enrollment key and sibling bindings remain unchanged.

### 6.2 E02 — OAuth replacement and direct native entry

1. Submit set_oauth for the personal-token binding through public MCP.

   Result: Only a pending owner request appears. The old credential remains until Save.

> **CAUTION:** REPLACE ONLY THE EXACT SYNTHETIC API BINDING.
> Saving retires its previous API slot. Keep the enrollment key and unrelated bindings.

2. Choose its native requested entry and enter synthetic client ID and secret. Select Save.

   Result: Both fields are masked and start empty. Applied follows local replacement. The original caller observes OAuth kind through CLI/MCP.

> **CAUTION:** ENTER ONLY SYNTHETIC VALUES IN THE OWNED DRAFT.
> Discard deletes the unsaved draft. Do not enter or expose host credentials.

3. Use Replace API credential directly. Enter a synthetic draft, change the source and use Discard credential draft. Reopen entry and try undo.

   Result: Source change clears all fields. Discard clears the draft without writing; reopening is empty and undo restores no cleared value. Discard alone does not cancel a still-pending agent request.

### 6.3 E03 — Cancellation, expiry and target changes

> **CAUTION:** USE ONLY THE OWNED SYNTHETIC REQUEST AND DRAFT.
> Cancellation clears its unsaved values. Keep host credentials outside the fixture.

1. Open a requested draft, enter synthetic values, then cancel from the original caller. Repeat with deterministic expiry and renamed/removed catalog targets.

   Result: The draft disappears. Old owner choices cannot start Save. Changed targets fail before OS-store mutation. Kind alone never marks a request applied.

> **CAUTION:** ENTER ONLY SYNTHETIC VALUES IN THE OWNED DRAFT.
> Closing Settings deletes unsaved draft values. Do not enter host credentials.

2. Enter an unsaved draft, close Settings and reopen it.

   Result: No draft or secret survives close. Existing read/write barriers and independent errors remain correct.

3. Run running/late-result, removed-row Status/Cancel and storage-root suspension regressions.

   Result: Running writes cannot be cancelled; a late result remains readable for its bounded retention. Lookup grants no new write or another caller access. Root suspension clears unsaved drafts without rendering or dropping owned writes/errors.

### 6.4 E04 — Failure ownership and presentation

> **CAUTION:** CHANGE ONLY THE OWNED PRIVATE STORE.
> Save can replace an API binding. Keep all host stores and unrelated bindings intact.

1. Make the owned private store unavailable, attempt a synthetic Save, then restore it and refresh status.

   Result: Failure proves no rollback. A successful read clears reader failure only and preserves an unrelated Save failure. A later successful write clears its own write failure only.

2. Record native frames, actual public replies and a GIF. Resize the window and select workspace Fit.

   Result: Masked fields, owner choices and status remain readable. The GIF uses real frames and is attached to the PR, not committed. It does not replace public response or Store-outcome evidence.

## 7. Teardown

> **CAUTION:** CLEAN ONLY THE EXACT OWNED SYNTHETIC FIXTURE.
> Cleanup deletes synthetic credentials and fixture data. Preserve unresolved operations and all host bindings.

1. Stop owned observation tasks, close the exact viewer/relay and remove only the accepted synthetic bindings through trusted equipment.

   Result: The enrollment/API services contain no owned leftovers; the exact healthy Docker daemon confirms owned container absence. No host store was accessed.

2. Preserve logs, frozen manifests, public results, source hashes and exact cleanup receipt.

   Result: Uncertain cleanup retains its owner and evidence; no broad prune or unowned deletion occurs.
