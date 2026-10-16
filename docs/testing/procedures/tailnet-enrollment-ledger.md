---
procedure: tailnet-enrollment-ledger
feature: persisted tailnet enrollment evidence
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Tailnet enrollment ledger test procedure

## 1. Purpose

This procedure tests the storage of receipts and capture attempts.
It tests bounds, conversion and retention after deletion and redeploy.

## 2. Applicability

- Candidate: the recorded-device capture outcome; this procedure covers its persistence tests.
- Platforms: Linux and macOS hosts with durable cloud records.
- The tests use synthetic identities and temporary records.
- This procedure does not test live capture, provider requests, node removal or UI behavior.

## 3. Equipment and preconditions

- Use the repository Rust toolchain and declared native build prerequisites.
- Use the candidate checkout.
- No tailnet account or credential is necessary.

## 4. Setup

1. Record the candidate commit and uncommitted inputs.

   Result: The evidence identifies the exact source files.

## 5. Tasks

### 5.1 LEDGER — Bounds and state transitions

1. Run the ledger tests.

   ```bash
   RUSTFLAGS="-D warnings" cargo test -p horizon-core cloud_runtime::tailnet::receipt --lib
   ```

   Result: All ledger tests pass.

2. Examine the test results.

   Result: Exact receipt retries do not add entries. Different device IDs retain the old evidence.

3. Examine the refusal cases.

   Result: Unknown fields, invalid identities, duplicate entries and bounds are refused.
   The ledger stays intact after a failed mutation.

### 5.2 CONVERSION — Allocation ownership

1. Run the legacy conversion tests.

   ```bash
   RUSTFLAGS="-D warnings" cargo test -p horizon-core cloud_runtime::allocation::legacy --lib
   ```

   Result: All legacy conversion tests pass.

2. Examine the enrollment conversion test.

   Result: The allocation retains receipts and attempts when the worker field is absent.
   The project record contains no enrollment ledger.

3. Examine the historical-record test.

   Result: Absent or empty metadata supplies no device ID. Unknown metadata is refused.

### 5.3 RETENTION — Failed save and redeploy

1. Run the redeploy tests.

   ```bash
   RUSTFLAGS="-D warnings" cargo test -p horizon-core cloud_runtime::deployment::redeploy --lib
   ```

   Result: All redeploy tests pass.

2. Examine the enrollment retention test.

   Result: A failed save keeps the durable ledger.
   Confirmed deletion and redeploy keep its receipts and unresolved attempts.

### 5.4 FENCE — Refuse older readers

1. Run the deployment store tests.

   ```bash
   RUSTFLAGS="-D warnings" cargo test -p horizon-core cloud_runtime::state::tests --lib
   ```

   Result: A nonempty ledger uses version 4, including records with siblings.
   The previous reader rejects this version.
   Malformed version combinations keep the original file and return an error.

## 6. Pass criteria

- Every command passes on the candidate inputs.
- No test uses a real provider or OS credential.
- An invalid mutation cannot drop recorded evidence.
- Legacy conversion preserves all nonempty enrollment metadata.
- Tests do not claim that enrollment captures a live device ID.

## 7. Cleanup

The test fixtures remove their temporary records.
No cloud resource or credential needs cleanup.

## 8. Record of results

Put commands, toolchain, input hashes and results in the pull request.
Keep private build logs outside the repository.
