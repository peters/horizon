---
procedure: image-destination-management
feature: shared local image destination management and bounded operation receipts
platforms: [linux, macos, windows]
cost: none
destructive: no
---

# Image destination management

## 1. Purpose

Verify that host adapters share local destination policy and retain operation ownership.
This prerequisite wires no public adapter and does not activate automatic publishing.

## 2. Applicability

Use a source checkout and the repository Rust toolchain. Metadata and receipt tests
run on all listed platforms. Durable preference writes require Unix and are ignored
with an explicit reason on Windows; hosted Windows compilation remains required.

## 3. Safety

Use temporary synthetic settings and Git repositories. No installed account, secret,
compute provider, sign-in service or real registry is required by the normal tests.

## 4. Prerequisites

1. Read the changed boundary in `docs/architecture/image-publishing.md`.

   Result: The host owns receipt scope; repository text supplies no authority.

## 5. Test equipment

Use `cargo test`, local Git and a temporary directory. Keep root-only worker skips
separate from these tests. Do not use a shared account or settings directory.

## 6. Tasks

### 6.1 DM-01 — Metadata and preference fence

1. Run `cargo test -p horizon-core --lib cloud_runtime::publishing::management`.

   Result: Listing needs no registry secret or compute key. Select/reset retain
   canonical repository scope and refuse stale shown state. Missing profiles,
   stale authentication requests and arbitrary personal targets refuse before sign-in.
   A receipt bound to a subdirectory cannot widen its scope to the Git root;
   direct native select/reset still normalizes checkout subdirectories. Cancellation
   during a failed or successful metadata lookup remains a cancelled result.
   Each authentication attempt receives a private key distinct from a valid cloud ID
   and every other attempt, even when the caller reuses its receipt ID.

2. Run `cargo test -p horizon-core --lib metadata_loading`.

   Result: Missing settings return standard defaults without creating files.
   Malformed existing bytes are refused and preserved.

### 6.2 DM-02 — Receipt ownership and lifecycle

1. Run `cargo test -p horizon-core --lib cloud_runtime::publishing::operations`.

   Result: Another actor, host, root or repository cannot access a receipt. The
   same ID cannot name another request. Active work and retained results are bounded;
   abandoned scopes, unobserved jobs and deadlines request cancellation. A sign-in
   prompt stays with its original owner and clears when the operation finishes.

Recipe metadata matches saved pull bindings for bare, tagged and digest-pinned
images, including registry ports. Other paths and unconfirmed bindings remain
unmatched. The metadata lookup reads no registry secret.

### 6.3 DM-03 — Read-only real checkout

1. Set `HORIZON_TEST_REPOSITORY` to this task checkout and `HORIZON_TEST_PROFILE`
   to one of its existing build profiles, then run the ignored
   test with the command below.

   ```sh
   cargo test -p horizon-core --lib real_repository_destination_listing_preserves_source_and_machine_settings -- --ignored
   ```

   Result: Actual Git/configuration lookup succeeds. Repository YAML and synthetic
   settings bytes are unchanged; no Docker login or registry journal is created.
   Run this on the first and final reviewable heads.

## 7. Pass criteria

All applicable checks pass, including the full local matrix and current-head hosted
CI. Keep metadata, synthetic sign-in, real source and live registry evidence separate.
No publication, allocation or public-interface proof is implied by this procedure.

## 8. Cleanup

1. Let temporary fixtures drop and remove only task-owned evidence resources.

   Result: Installed settings and account stores remain unchanged.

## 9. Record of results

Record head, tree, toolchain, commands, outcomes, expected skips and source checks.
Never record credentials or raw sign-in codes in public PR evidence.
