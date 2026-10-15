---
procedure: host-pull-journal
feature: Direct-host image pull journal prerequisite
platforms: [linux, macos]
cost: none
destructive: no
secrets: synthetic fixtures only
owner: peters
---

# Host pull journal test procedure

## 1. Purpose

Check separate direct-host and provider pull records.
Check that one generation retains the same pull identity in both records.

## 2. Applicability

Use the host-only journal prerequisite for #1597.
These checks use synthetic settings and temporary private files.
Active deployment callers do not use the new provider-aware helper yet.
No visible UI change or native GIF applies to this prerequisite.
Windows compiles these tests but skips their runs with an explicit durability reason.
Windows does not support the required durable registry journal operations.

## 3. Safety

Use only the synthetic tests in this procedure.
They do not sign in, contact a registry or allocate compute.
They do not revoke a credential installed on a worker.

## 4. Equipment and preconditions

- A checkout of the candidate and its supported Rust toolchain.
- Linux or macOS with durable local filesystem operations.

## 5. Setup

1. Record the candidate commit and the consumed source inputs.

   Result: The record identifies the code used by each check.

## 6. Tasks

### 6.1 HJ-01 — Separate journals and preserve identity

1. Run the direct-host journal tests.

   ```sh
   RUSTFLAGS="-D warnings" cargo test -p horizon-core --lib cloud_runtime::registry::
   ```

   Result: Host preparation succeeds without a compute key.
   Host and provider records use separate paths and locks.
   An actual host journal path inside the source, including an alias, is refused
   before a lock or record is created there.
   Missing private ancestors are created before the generation pin.
   Interrupted ancestor creation is recovered before a child is created.
   Existing roots work without read permission on their parent.
   A coordination symlink or hard link cannot change an existing private file.
   A host grant refuses provider registry creation.
   Changed pull material requires a new generation in either order of first use.
   Concurrent first use cannot pin two identities for one generation.
   Concurrent first use with the same identity succeeds.
   An existing pin does not wait for another scope's first-use lock.

### 6.2 HJ-02 — Complete qualification

1. Examine `registry/directory/unix.rs` or record a syscall trace of the synthetic tests.

   Result: The pending intent and its parent directory entry are synced before mkdir.
   The new directory and its parent are synced before the pending intent is cleared.
   Recovery syncs the parent before a child proceeds.
   A syscall trace checks ordering, not physical power-loss recovery.

2. Run the applicable full local matrix.

   Result: Existing scope, secret separation, immutable-image, expiry and provider checks pass.
   Blocking and strict checks pass. Record advisory results separately.
   Hosted CI covers the supported platform lanes on the exact PR head.

## 7. Pass criteria

- All applicable tasks pass.
- Host preparation uses no compute key and cannot create a provider registry ID.
- One generation cannot accept different pull identities through the two journal scopes.
- Existing provider records retain their original path and account identity.

## 8. Cleanup

1. Let the tests remove their temporary directories.

   Result: No synthetic credential or journal remains.

## 9. Record of results

Record the candidate, commands, logs and results in the pull request.
State that no live pull, worker allocation or installed-credential revocation was tested.
