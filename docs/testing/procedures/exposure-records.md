---
procedure: exposure-records
feature: Default-off exposure intent model and pure transitions
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Exposure intent model test procedure

## 1. Purpose

Test bounded comparison data, strict JSON decoding and exact metadata transitions.
Keep existing saved formats and runtime activation unchanged.

## 2. Applicability

- Use this procedure for the Core model prerequisite after the merged policy prerequisite.
- This procedure does not test persistence migration, listeners, live workers or provider effects.
- It does not prove membership, owner approval, physical expiry or public interface parity.

## 3. Safety

Use only the synthetic model values in the tests.
This procedure uses no credentials, account, provider resource or paid capacity.

## 4. Equipment and preconditions

- Use the repository Rust toolchain and locked dependencies.
- Use an isolated task checkout, HOME, temporary directory and build cache.
- Keep `cloud-exposure` off in default features and public dispatch.

## 5. Setup

1. Read `docs/architecture/exposure-records.md`.

   Result: The model and live authority boundaries are clear.

2. Run `cargo fmt --all -- --check`.

   Result: The format check passes.

## 6. Tasks

### 6.1 R01 — Bounded transitions

1. Run `cargo test -p horizon-core --features cloud-exposure cloud_runtime::state::exposure::tests`.

   Result: The five model tests pass.
   Renewal changes only the selected service and requires a new operation.
   Currently retained operation IDs, rollback, expiry, overflow and conflicting ingress are refused.

### 6.2 R02 — Exact close and Finish operation

1. Examine the close and Finish results from R01.

   Result: Wrong epochs, custody, service ports and operations cannot advance the model.
   Unknown startup cannot reopen.
   Finish uncertainty retains the original operation through JSON conversion.

### 6.3 R03 — Strict data and policy limits

1. Examine the corruption and service-bound results from R01.

   Result: Invalid deadlines, nil IDs, unknown nested fields and duplicate nested keys are refused.
   The ninth service, blocked targets and unsupported route combinations are refused.
   A decoded value cannot supply live custody or approval.

### 6.4 R04 — Existing persistence contracts

1. Run `cargo test -p horizon-core cloud_runtime::state::`.

   Result: Existing storage tests pass with the default feature set.
   The model adds no saved format field or version.

2. Run `cargo test -p horizon-core --features cloud-exposure cloud_runtime::allocation::legacy::tests`.

   Result: Existing paired format and ownership tests pass.
   No exposure model is written through those existing formats.

### 6.5 R05 — Applicable matrix

1. Complete the repository local matrix on the stable candidate.

   Result: Default, speech, blocking and strict checks pass.
   Record advisory findings separately.
   Treat both bundled skills and references as UI build inputs.

2. Run the enabled Core blocking and strict tiers with `cloud-exposure`.

   Result: The pure transition surface passes the required lint tiers.
   No public operation or runtime exposure is activated.

## 7. Pass criteria

- R01 through R05 pass on the recorded inputs.
- Model validation cannot grant live authority or discard invalid nested data.
- Active storage fields, versions and fences match the base.
- No persistence migration or full epic acceptance is inferred from model tests.

## 8. Cleanup

1. Stop only the owned test processes after their results are recorded.

   Result: Temporary state can be removed without a live resource operation.

## 9. Record of results

Record the commit, source inputs, toolchain, features, commands, logs and actual results.
Keep new, reused and non-applicable results separate.
The later persistence outcome requires a separate composite Store and paired-conversion test.
That test must retain exposure, publication, admission and enrollment together.
