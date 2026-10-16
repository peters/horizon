---
procedure: tailnet-receipt-capture
feature: persisted worker enrollment device identity
platforms: [linux]
cost: none offline; live task rents authorized compute
destructive: yes, task-owned live resources only
secrets: [desktop tailnet auth key, provider credential]
owner: peters
---

# Tailnet receipt capture test procedure

## 1. Purpose

The offline tasks test host ordering and the fixed root projection with synthetic inputs.
The live task separately qualifies actual capture and stop/resume.
This procedure does not qualify API node removal.

## 2. Preconditions

- Use the candidate checkout and its declared Rust build prerequisites.
- Use Docker with the existing task-owned worker-base image.
- Capture is enabled in the candidate.
- Use no real daemon, credential or provider for the offline tasks.
- The live task requires an explicitly authorized dedicated tailnet and task-owned worker.

## 3. Tasks

### 3.1 HOST — Durable ordering

1. Run the host tests.

   ```bash
   RUSTFLAGS="-D warnings" cargo test -p horizon-core tailnet::capture --lib
   ```

   Result: All host tests pass.
   A failed initial save prevents configure.
   Capture or receipt-save failure preserves prior evidence and unresolved attempts.

### 3.2 ROOT — Offline authority and bounds

1. Run the root fixture in an isolated container.

   ```bash
   docker run --rm --init --network none --read-only \
     --tmpfs /run:rw --tmpfs /workspace:rw \
     --tmpfs /usr/local/bin:rw,exec --tmpfs /tmp:rw,exec \
     --mount "type=bind,source=$PWD/crates/horizon-core/src/cloud_runtime/tailnet/capture,target=/source,readonly" \
     --entrypoint /usr/bin/python3 \
     ghcr.io/peters/horizon-worker-base@sha256:6550aacb6c60187cd346a795beb3c8bffd998fe9b95ec74f5fb844cf78870bdc \
     /source/projection_test.py
   ```

   Result: All root projection tests pass.
   The fixture writes a fake CLI to a temporary mount.
   It uses a fake socket and synthetic status only.
   Network access stays disabled.

2. Examine the refusal results.

   Result: Wrong selection, tags, IDs, ownership, modes and symlinks are refused.
   Duplicate fields, oversized output, lock delay and child delay are refused.
   Raw status and stderr stay private.

### 3.3 LIVE — Captured identity and stop/resume

Run this task on both the first reviewable and final source heads. Offline results do not replace it.

1. Record the exact controller source, binary and immutable marker-4 worker image.

   Result: The controller contains this capture path.
   The worker meets its private path and ownership requirements.

> **CAUTION:** SELECT ONLY THE AUTHORIZED TASK TAILNET.
> A wrong selection can give the worker access to other devices.

2. Select the authorized saved tailnet for one task-owned cloud before allocation.

   Result: No existing user cloud or daemon changes.
   API credentials stay on the desktop.

> **CAUTION:** USE ONLY THE APPROVED DESKTOP API CREDENTIAL.
> Keep its value in the desktop store and private authenticated channel.

3. Record the protected baseline device IDs and the task allocation request.

   Result: The baseline identifies the devices that this task must not change.

> **CAUTION:** CREATE ONLY THE APPROVED TASK KEY.
> The key grants access to the dedicated tailnet.

4. Create a short-lived tagged auth key for only the task.

   Result: Only the task uses this key.
   The original auth key remains unchanged.

> **CAUTION:** STORE THE TASK KEY ONLY IN ITS EXACT DESKTOP BINDING.
> Never put its value in command arguments, logs or a different worker.

5. Store the task key in Horizon's exact desktop binding.

   Result: Only the task binding changes.
   No API credential reaches the worker.

> **CAUTION:** DEPLOY ONLY THE ONE APPROVED TASK WORKER.
> Deployment rents compute and sends the task auth key.
> Keep the run within its approved budget.

6. Deploy the task worker through the existing UI, CLI or MCP lifecycle.

   Result: The shared core saves an attempt before enrollment.

7. Wait for Ready.

   Result: The core saves a receipt before readiness.
   A storage or isolation failure refuses Ready.
   Preserve the fixed failure stage for exact reconciliation; do not bypass the checks.

8. Record the exact task resource IDs from the owning controller.

   Result: The record identifies the task compute, storage and provider SSH key.

> **CAUTION:** USE ONLY THE APPROVED TASK READ CHANNELS.
> The node lookup uses an API credential.
> Do not expose credential values or raw daemon output.

9. Compare the public core Store receipt with the actual worker Self.ID and its proven task node.

   Result: Saved tailnet, provider, worker, allocation request and device IDs match the actual allocation.
   Record only nonsecret identities.
   A displayed name or peer ID cannot qualify this check.

> **CAUTION:** REVOKE ONLY THE RECORDED TASK KEY.
> Revocation stops new enrollment with that key.
> Preserve the original user binding.

10. Revoke the task auth key through the separately authorized admin cleanup path.

    Result: The exact task key is invalid.
    Record the revocation time even if the API retains revoked metadata.

> **CAUTION:** REMOVE ONLY THE EXACT TASK AUTH-KEY OS VALUE.
> Removal deletes its saved secret value.
> Retain the task catalog entry and selection; keep original credential bindings unchanged.

11. Remove only the task auth-key OS value through the separately authorized cleanup path.

    Result: The exact OS item is absent.
    The task catalog entry, saved tailnet ID and cloud selection remain unchanged.
    Do not delete the saved tailnet or use catalog-removing binding cleanup before Resume.

> **CAUTION:** USE ONLY THE APPROVED TASK READ CHANNELS.
> Key status uses an API credential.
> Keep secret values private.

12. Examine task-key revocation, exact OS-value absence and retained catalog selection before Stop.

    Result: The recorded key is invalid with a nonempty revocation time.
    Its exact OS item is absent.
    The catalog entry and selected saved tailnet ID still match the task.
    Stop cannot proceed without all three proofs.

> **CAUTION:** STOP ONLY THE RECORDED TASK WORKER.
> Stop interrupts task sessions; retained storage can still cost money.
> The controller uses the provider credential.

13. Stop the task worker through the owning controller.

    Result: The retained volume stays owned.
    Hetzner removes the original server.

> **CAUTION:** RESUME ONLY THE APPROVED TASK CLOUD.
> UI Resume automatically starts reconnect, which rents compute and changes access.
> A missing retained daemon login must refuse readiness; do not restore a key.

14. Resume the task cloud through the owning controller.

    Result: The controller records resume intent.
    CLI Resume returns before reconnect allocates compute.
    UI Resume automatically starts reconnect.

> **CAUTION:** RECONNECT ONLY THE APPROVED TASK CLOUD.
> Reconnect rents compute, changes worker access and uses the provider credential.
> Keep the task auth-key OS value absent; no new enrollment is allowed.

15. If reconnect has not started, connect again through UI **Reconnect** or `cloud_deploy reconnect SETTINGS STATE_ROOT`.

    Result: Hetzner creates a new server on the retained volume.
    An automatic UI reconnect needs no second request.

16. Wait for Ready.

    Result: Reconnect completes readiness and saves the worker-bound receipt.
    A daemon that needs a new login cannot qualify this key-free lane.

> **CAUTION:** READ ONLY THE EXACT TASK AUTH-KEY OS SLOT.
> Keep secret values private.
> Do not recreate the absent item.

17. Examine exact task auth-key OS-value absence after resumed Ready.

    Result: The recorded OS item remains absent.
    No task enrollment key was restored for reconnect.

> **CAUTION:** USE ONLY THE APPROVED TASK READ CHANNELS.
> The node lookup uses an API credential.
> Keep all secret values private.

18. Repeat the receipt and actual-ID comparison after resume.

    Result: A logged-in daemon keeps Self.ID without another key transfer.
    Both worker-bound receipts remain when Self.ID stays the same.
    Only identical complete receipts count as a retry.
    If the actual node changes, retain both device IDs and report the change.

> **CAUTION:** DELETE ONLY THE RECORDED TASK COMPUTE AND STORAGE.
> Deletion removes task data and uses the provider credential.

19. Delete the task compute and storage through their owning controllers.

    Result: Exact resource checks report absence.
    The cloud's saved ledger remains intact after provider deletion.

> **CAUTION:** DELETE ONLY THE NODE PROVEN BY THIS TASK.
> Node deletion changes tailnet access.
> Never delete a protected baseline device.

20. Delete the proven task node through the separately authorized admin cleanup path.

    Result: Its exact API read reports absence.
    This is test cleanup, not automatic product node deletion.

> **CAUTION:** CLEAN ONLY THE EXACT TASK CATALOG ENTRY AND OS-BINDING METADATA.
> Cleanup deletes the saved task network and its remaining binding metadata.
> Require exact provider and node absence proofs; preserve original and unrelated bindings.

21. After exact provider and task-node absence proofs, remove only the task catalog entry and remaining OS-binding metadata.

    Result: The saved task network and its exact credential-binding metadata are removed.
    The worker receipt ledger remains intact.
    The exact task auth-key OS item stays absent.

> **CAUTION:** USE ONLY THE APPROVED CLEANUP READ CHECKS.
> These checks use API credentials.
> Keep all secret values private.

22. Examine exact resource absence and the protected baseline device IDs.

    Result: Task resources are absent and baseline devices remain.
    The original user credential binding stays unchanged.

## 4. Pass criteria

- Offline host/projection checks pass on the recorded candidate inputs.
- First-reviewable and final heads have separate real captured-ID/stop-resume evidence.
- The child ends after its deadline.
- Host cancellation preserves the unresolved attempt.
- Offline tests contact no real daemon, API or provider.
- The candidate captures the device ID before readiness.

## 5. Cleanup and evidence

Docker removes the fixture container and its temporary mounts.
Record commands, input hashes, image digest and results in the pull request.
A merge candidate needs authorized first-reviewable and final-head live worker evidence.

The current synthetic projection suite also switches a subprocess to UID10001. It must emit no data and must never invoke the private daemon CLI. A host binding test refuses another endpoint/port, alias/worker or cloud root before transport. These are existing focused fixture/unit tests, not a new framework. A production candidate still requires actual captured Self ID equality against the owned node before and after stop/resume; synthetic IDs do not qualify that gate.
