---
procedure: cloud-tailnet-selection
feature: Pre-create saved tailnet selection through cloud_deploy
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Cloud tailnet selection test procedure

## 1. Purpose

Test the normal CLI selection through the existing cloud lifecycle lock.
Keep the original catalog and deployment identity.

## 2. Applicability

- Use the candidate `cloud_deploy` example and its locked dependencies.
- Linux and macOS support the existing durable cloud state.
- Windows must refuse state mutation before it creates a cloud directory.
- This procedure tests no enrollment, credential write, provider operation or native approval.

## 3. Safety

Use synthetic catalogs and task-owned paths for the automated tests.
They contain no key or OS-store binding.
For an authorized original-catalog check, preserve every existing cloud and credential.
Selection does not authorize a deployment or a network change after allocation.

## 4. Equipment and preconditions

- A separate candidate checkout and the repository build prerequisites.
- A private source manifest, test log and normal executable manifest.
- For the original-catalog check, an authorized saved ID and a fresh task cloud ID.

## 5. Setup

1. Record the source, dependency, toolchain and environment inputs.

   Result: Each result identifies the exact candidate.

2. Record the original catalog metadata before an original-catalog check.

   Result: The saved ID, catalog path and catalog bytes remain comparison facts.
   No credential value enters the evidence.

## 6. Tasks

### 6.1 S01 — CLI routing and lifecycle guards

1. Run the example tests.

   ```sh
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-core --example cloud_deploy -- tailnets:: tests::malformed_selection_arguments_never_create_cloud_paths
   ```

   Result: Missing, extra or empty-ID arguments refuse before path creation.
   The selected ID passes through the real CLI route and production selection.
   Unknown networks and unsafe cloud IDs refuse.
   A held original lock prevents a concurrent selection.
   An allocated cloud keeps its network and deployment bytes.
   Existing inventory status behavior remains unchanged.
   These filters cover selection and inventory on each platform.
   The full Unix example suite remains a separate local check.

2. Run the existing pending-operation regression on Unix.

   ```sh
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-core --lib cloud_runtime::companions::lifecycle::tests::tailnets::pending_tailnet_requests_fence_edits_and_fail_before_execution_on_drift -- --exact
   ```

   Result: A pending cloud operation retains its exact network selection.
   This existing regression supplies the shared transition check.

### 6.2 S02 — Normal CLI selection

1. Build the normal example.

   ```sh
   cargo build --locked --offline -p horizon-core --example cloud_deploy
   ```

   Result: The manifest identifies a normal executable with `profile.test=false`.

2. For an authorized fresh cloud, run the normal command with the original catalog root.

   ```sh
   cloud_deploy select-tailnet CATALOG_ROOT CLOUD_ID TAILNET_ID
   ```

   Result: Unix selects the existing saved ID at `CATALOG_ROOT/CLOUD_ID`.
   The catalog bytes remain unchanged. No deployment or enrollment starts.
   Windows refuses before path creation.
   Key validity and tags remain untested until normal enrollment.

3. Examine the original catalog comparison and the new selected record.

   Result: Only the task cloud selection and its lifecycle lock are new.
   The normal deployment must use `CATALOG_ROOT/CLOUD_ID` as its state root.
   Do not copy a catalog, credential slot or Ready record.

### 6.3 S03 — Required checks

1. Run the applicable repository matrix and the example lint checks.

   Result: Blocking and strict checks pass for the candidate.
   Record unchanged-input reuse and advisory findings separately.

2. Run the bundled skill check.

   ```sh
   python3 scripts/check-horizon-mcp-skills.py
   ```

   Result: Both cloud skill copies describe the same selection contract.

## 7. Pass criteria

- The applicable tests and required checks pass.
- The original catalog and allocated cloud identity remain unchanged.
- Unsupported hosts refuse before state writes.
- The normal command reads no secret and starts no remote operation.

## 8. Cleanup

1. Remove only the task-owned synthetic paths after all test processes exit.

   Result: Existing user state remains unchanged.

2. Keep a genuine selection with its original lifecycle if deployment follows.

   Result: Normal deletion retains the same saved ID for recorded-node cleanup.
   Never revoke a user-owned Auth key or remove its API binding for test cleanup.

## 9. Record of results

Record the exact source, executable, commands, results and log paths in the pull request.
Keep original paths and identities in private evidence.
Selection alone supplies no Worker, Producer, Native or full M2 qualification.
