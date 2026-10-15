---
procedure: publication-records
feature: Retained image publisher persistence prerequisite
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: synthetic metadata only
owner: peters
---

# Publication record test procedure

## 1. Purpose

Check retained image destinations and publisher metadata.
Check that version guards prevent accidental loss during migration or downgrade.

## 2. Applicability

Use the publication record prerequisite of #1597.
It does not activate automatic publishing, image omission or new UI controls.
No native GIF applies because existing visible behavior is unchanged.
Metadata checks run on all supported desktop platforms.
Write and migration checks need Unix directory durability.
Windows compiles these checks but skips unsupported runs with a stated reason.

## 3. Safety

Use only synthetic records in temporary directories.
Do not edit a user's deployment version or migrate their cloud records for this procedure.
No credential, registry request or compute allocation is required.

## 4. Equipment and preconditions

- The candidate source and supported Rust toolchain.
- Linux or macOS for durable write and migration checks.

## 5. Setup

1. Record the candidate and the inputs consumed by each check.

   Result: The evidence identifies the checked code and any valid reused results.

## 6. Tasks

### 6.1 PR-01 — Validate persisted metadata and version guards

1. Run the publication record tests.

   ```sh
   RUSTFLAGS="-D warnings" cargo test -p horizon-core --lib cloud_runtime::state::publication_tests
   ```

   Result: A valid publication uses deployment format 5.
   Missing evidence and downgraded versions are refused.
   Invalid account, namespace or profile metadata cannot replace a valid saved record.
   Bad input is refused without an authentication request.
   The saved record remains unchanged after a refused write.

### 6.2 PR-02 — Require the retained grant only for new images

1. Run the initial-image and rebuild compatibility tests.

   ```sh
   RUSTFLAGS="-D warnings" cargo test -p horizon-core --lib publishing_fence
   ```

   Result: Initial image and rebuild factories refuse a missing retained publishing grant
   before loading compute credentials. An existing image specification needs no
   publishing login.
   No Docker configuration is created and saved records remain unchanged.

### 6.3 PR-03 — Preserve paired allocation and neighboring ownership

1. Run the paired conversion and migration regressions.

   ```sh
   RUSTFLAGS="-D warnings" cargo test -p horizon-core --lib cloud_runtime::allocation::legacy::tests
   RUSTFLAGS="-D warnings" cargo test -p horizon-core --lib cloud_runtime::state::migration::tests
   ```

   Result: Publication records use paired allocation format 3.
   Enrollment and publisher metadata survive conversion and reopening.
   A neighboring publication record retains its original bytes.
   A duplicate worker still prevents migration.
   Existing older records remain supported.

2. Run the applicable full local matrix.

   Result: Blocking and strict checks pass. Record advisory results separately.
   Hosted CI checks the supported platforms on the exact head.

## 7. Pass criteria

- All applicable tasks pass.
- Invalid records cannot erase or replace a retained publisher.
- Paired migration retains account, destination, enrollment and worker ownership evidence.

## 8. Cleanup

1. Let the tests remove their temporary directories.

   Result: No synthetic deployment record remains.

## 9. Record of results

Record the candidate, commands and results in the pull request.
State that no live account, publication or worker allocation was tested.
