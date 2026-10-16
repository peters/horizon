---
procedure: exposure-records
feature: Retained exposure intent and paired record migration
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Retained exposure record test procedure

## 1. Purpose

Test bounded exposure metadata, exact phase transitions and version-safe storage.
Make sure the new highest version preserves the existing record fields.

## 2. Applicability

- Candidate: atomic retained exposure persistence after the merged policy and model prerequisites.
- Platforms: Rust hosts with the declared filesystem durability support.
- This procedure does not test live workers, provider effects, membership, owner
  approval, listeners, actual eight-hour expiry or public interface activation.

## 3. Safety

Use only the synthetic records and private temporary directories in the tests.
This procedure uses no credential, provider account or paid resource.

## 4. Equipment and preconditions

- Use the repository Rust toolchain and locked dependencies.
- Use an isolated task checkout, HOME, temporary directory and build cache.
- Apply the complete reviewed record migration before qualification.
- Include the required composite regression in the exposure test module. A run
  with only the seven pure model tests does not satisfy task R05.

## 5. Setup

1. Read `docs/architecture/exposure-records.md`.

   Result: The metadata and runtime authority boundaries are clear.

2. Run `cargo fmt --all -- --check`.

   Result: The format check passes.

## 6. Tasks

### 6.1 R01 — Test bounded transitions

1. Run `cargo test -p horizon-core --features cloud-exposure cloud_runtime::state::exposure::tests`.

   Result: All ten exposure tests pass on hosts with directory durability, including
   the Store and version-7 neighbor regressions.
   Renewal changes only the selected service. Currently retained operation IDs,
   rollback, expiry, overflow and conflicting ingress
   are refused. This tests metadata, not a physical timer or replay ledger.

### 6.2 R02 — Retain the exact close and Finish operation

1. Examine the close and Finish test results from R01.

   Result: The wrong epoch, custody, service port or operation cannot advance
   the record. Unknown startup cannot reopen. Finish uncertainty retains the
   original operation. Both Finishing and FinishUnknown reject a Finish operation
   that matches a retained service operation. The exact result survives JSON conversion.

### 6.3 R03 — Preserve strict data and version fences

1. Examine the corruption, Store and paired-conversion results from R01.

   Result: Store retains the unresolved exposure record. Paired conversion uses
   Deployment 7 and Allocation 5. A downgrade or changed workspace is refused.
   Saving an exposure record bound to a different cloud is refused and leaves
   the original deployment bytes unchanged.
   Invalid deadlines, nil IDs, unknown nested fields and duplicate nested keys
   are refused. Raw provider service/foreign map duplicates and typed key aliases
   cannot erase evidence. The ninth service and blocked service targets are refused.

### 6.4 R04 — Preserve the existing storage contracts

1. Run `cargo test -p horizon-core --features cloud-exposure cloud_runtime::state::`.

   Result: The existing publication, admission, enrollment and migration tests
   pass with the new schema. The version-7 neighbor regression invokes
   `MigratedStore::migrate`: a shared worker is refused by the duplicate-worker
   fence, while a different worker migrates. Both cases leave the original
   neighbor bytes unchanged.

2. Run `cargo test -p horizon-core --features cloud-exposure cloud_runtime::allocation::legacy::tests`.

   Result: Existing paired format and ownership tests pass. Absent exposure keeps
   the earlier version selection and encoding.

### 6.5 R05 — Retain all four fields in the highest version

1. Examine the executed composite regression from R01.

   Result: One original Deployment contains exposure, publication, one-device
   admission and enrollment together. Store save/load retains all four fields
   at version 7. Allocation/project encode/decode retains the same four values
   with Allocation version 5. A mismatched owner or workspace is refused.

2. Make sure the run includes that composite test.

   Result: Separate tests for each field cannot mark R05 as passed.

### 6.6 R06 — Test the default feature set and full matrix

1. Run `cargo test -p horizon-core cloud_runtime::state::`.

   Result: The default storage tests pass. No runtime exposure is activated.

2. Complete the repository local matrix on the stable candidate.

   Result: Blocking and strict checks pass. Record advisory findings separately.
   Treat both bundled references as UI build inputs. Use exact binaries for any
   environment complement and preserve each raw failure.

## 7. Pass criteria

- R01 through R06 pass on the recorded candidate inputs.
- All four fields survive the same Store and paired-format round trip.
- Invalid metadata cannot replace the previous saved state or grant authority.
- No runtime/provider or full epic acceptance result is inferred from these tests.

## 8. Cleanup

1. Stop the owned test processes after their results are recorded.

   Result: Temporary test state can be removed. No live resource needs cleanup.

## 9. Record of results

Record the commit, source inventory, toolchain, features, commands, logs and
actual test results in the pull request. Keep new and reused results separate.
Use `docs/testing/reports/TEMPLATE.md` if a permanent report is required.
Do not record a deferred or unexecuted command as passed.
