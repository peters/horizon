---
procedure: one-device-admission-records
feature: Validated durable one-device attempt metadata
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# One-device admission records test procedure

## 1. Purpose

Test record validation, version fences and lossless migration of one-device metadata.
Test coexistence with current publishing fields and the read-only snapshot contract.

## 2. Applicability

- Use a candidate that changes admission metadata, shared reads or paired conversion.
- Use the existing `cloud-workspaces` feature without an endpoint-engine feature.
- This procedure does not qualify an owner controller, worker, network or public interface.

## 3. Safety

The tests use synthetic records and task-owned temporary directories.
They need no credentials, provider account, kernel client or paid equipment.
They perform no gateway or worker operation.

## 4. Equipment and preconditions

- The exact candidate source and Rust toolchain.
- Locked dependencies and the repository build prerequisites.
- Private HOME, TMP and XDG directories for the task.
- An isolated test environment with the recorded UID and filesystem behavior.

## 5. Setup

1. Record the candidate inputs, flags, toolchain and environment.

   Result: Each later check has an exact input identity.

2. Compare local package inputs before reusing a shared build target.

   Result: A different worktree cannot qualify the candidate through stale local artifacts.

3. If local package inputs changed, remove only those packages' task-owned build products.

   Result: The affected local producers rebuild without removing third-party caches.

## 6. Tasks

### 6.1 D01 — Validated metadata

1. Run the focused metadata tests.

   ```sh
   RUSTFLAGS="-D warnings" cargo test -p horizon-core --lib cloud_runtime::state::service_admission::
   ```

   Result: Malformed identities, bindings, unknown fields and version downgrades fail.
   Armed, Attached and Closed records survive storage without creating runtime authority.

### 6.2 D02 — Shared reads and publishing

1. Run the state tests.

   ```sh
   RUSTFLAGS="-D warnings" cargo test -p horizon-core --lib cloud_runtime::state::
   ```

   Result: Snapshot reads create no root and reserve no lock.
   Admission and publishing retain both fences, and invalid publishing cannot replace saved bytes.

### 6.3 D03 — Paired conversion and migration

1. Run the paired conversion tests.

   ```sh
   RUSTFLAGS="-D warnings" cargo test -p horizon-core --lib cloud_runtime::allocation::legacy::
   ```

   Result: The original project binding and complete metadata survive split/join.
   Moved ownership and unsupported record versions fail.

2. Examine the migration results from D02.

   Result: Enrollment and admission survive both publication orders, duplicate worker history and reopen.
   Missing, malformed or duplicate evidence is refused.

### 6.4 D04 — Required matrix and defaults

1. Run the full applicable local matrix from AGENTS.md.

   Result: Blocking and strict tiers pass for the exact candidate inputs.
   Default and speech UI tests consume the current embedded references.

2. Examine the current feature graph and runtime call sites.

   Result: Metadata reads need no endpoint-engine feature or new runtime caller.
   The change adds no public sharing capability.

## 7. Pass criteria

- D01–D04 pass on the exact candidate inputs.
- Nonempty admission uses deployment v6 and allocation v4; absent admission keeps earlier encodings.
- Original scope, retained phases and current publishing fields remain intact.
- No result treats saved facts or a snapshot as owner authority.

## 8. Cleanup

1. Remove only the task-owned test containers and temporary fixtures.

   Result: Their exact identities are absent, and user state remains unchanged.

## 9. Record of results

Keep source hashes, compiler records, commands and cleanup evidence private.
Record reused, new and non-applicable results separately.
Use the [report template](../reports/TEMPLATE.md) if the run needs a permanent report.
