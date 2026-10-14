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

1. Record the exact controller source/binary and immutable marker-4 worker image.

   Result: The running controller contains this capture path and the worker meets its private path/ownership requirements.

2. Use an authorized dedicated tailnet and one task-owned cloud. Select that saved tailnet through the existing controller before allocation. Keep its short-lived tagged auth key in Horizon's desktop OS store. Record baseline device IDs and exact task resource IDs.

   Result: No existing user cloud or daemon changes. No API credential reaches the worker.

3. Deploy through the existing UI, CLI or MCP lifecycle and wait for Ready.

   Result: The shared core saves an attempt before enrollment, then a receipt before readiness. A storage/isolation failure refuses Ready; record the fixed stage and stop without a workaround.

4. Read the task-owned cloud's record through the public core Store API and compare its receipt with the actual worker Self.ID and the owned node in the authorized tailnet.

   Result: Saved tailnet, provider, worker, allocation request and device IDs match the actual allocation. Record only nonsecret identities. A displayed name or peer ID cannot qualify this check.

5. Stop and resume through the existing controller. Read the task record again.

   Result: The stateful daemon retains its actual Self.ID and no key is retransferred for an already logged-in daemon. Hetzner resume creates a new server on the retained volume. Its new worker-bound receipt coexists with the previous receipt even when Self.ID stays the same. Deduplication applies only when the complete receipt matches. If the actual node changes, retain both device IDs and report the change.

6. Delete only the task-owned compute, storage and credential binding through their owning controllers. Revoke only the task key and remove only the proven task node through the separately authorized admin cleanup path. Reconcile exact IDs and baseline devices.

   Result: Task resources are absent and baseline devices remain. This is test cleanup, not a production automatic node-delete claim. The cloud's saved ledger remains intact after provider deletion.

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
