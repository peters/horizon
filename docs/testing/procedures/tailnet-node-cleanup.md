---
procedure: tailnet-node-cleanup
feature: Core exact recorded-node cleanup after provider deletion
platforms: [linux, macos]
cost: none
destructive: yes
secrets: [desktop tailnet API credential]
owner: peters
---

# Core recorded-node cleanup test procedure

## 1. Purpose

Test Core cleanup of exact recorded node IDs after provider deletion.
Make sure uncertainty preserves original receipts and unresolved attempts.

## 2. Applicability

Use the candidate with the NodeApi SDK and default-off Core cleanup addition.
For enabled checks, build with both `cloud-workspaces,tailnet-node-cleanup`.
Default UI and CLI builds retain prior behavior until the final UI/activation outcome.
Existing owner Delete and cloud_deploy delete use this controller.
No public companion MCP cloud-delete lane exists.
Native retry and precise card-retention presentation require the later UI/activation candidate.
Set credential entry is inherited from its separate prerequisite and supplies no node-deletion authority.

## 3. Safety

> **CAUTION:** DELETE ONLY THE EXACT TASK-OWNED CLOUD AND RECORDED NODE.
> Deletion removes compute, storage and tailnet access. Preserve protected baseline resources and devices.

Live setup needs separate authorization and a declared resource budget.
This procedure permits no new rental or policy change.
Keep credentials in Horizon and the desktop OS store.
Never pass a secret through an argument, environment variable, worker or log.

## 4. Equipment and preconditions

- Exact frozen candidate, source hashes, dependency lock and toolchain.
- The existing SDK, receipt, cleanup and migration tests.
- A Root-authorized task cloud with actual Self.ID receipts.
- Original allocation and provider resource records.
- Protected baseline node IDs and provider resources.
- An exact desktop API binding for the recorded saved tailnet.
- Existing owner CLI equipment and public status tools.

## 5. Setup

1. Record the candidate inputs and declared task ownership.

   Result: The evidence includes verified Git LFS content and original saved-tailnet, worker and allocation bindings.

2. Examine the current controller and provider records.

   Result: Unknown ownership or provider absence blocks live API dispatch.

## 6. Tasks

### 6.1 C01 — Offline controller behavior

1. Run the focused cleanup tests.

   ```bash
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-core --features tailnet-node-cleanup --lib cloud_runtime::tailnet::cleanup
   ```

   Result: Confirmation persists before the next request. Retry skips confirmed exact groups.
   Missing, denied, offline and 404 results retain exact IDs.

2. Run the existing tailnet and allocation tests.

   ```bash
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-core --features tailnet-node-cleanup --lib cloud_runtime::tailnet
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-core --features tailnet-node-cleanup --lib cloud_runtime::allocation
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-core --features tailnet-node-cleanup --lib cloud_runtime::deployment::redeploy
   ```

   Result: Confirmed paired records survive reopen. Duplicate keys, unknown fields and unsupported owner versions remain refused.
   Delete followed by redeploy refuses selected legacy missing identity, unresolved attempts and unconfirmed receipts before key validation, journal removal or history reset.
   Confirmed receipts survive replacement; genuinely fresh selection and None without enrollment still reopen.
   The feature-OFF ledger-retention regression keeps its original redeploy behavior.

3. Run the stable candidate's required repository matrix.

   Result: Blocking and strict checks pass. Advisory results and provider qualifications remain separate.

### 6.2 C02 — Independent authority and uncertainty

1. Run the existing original-provider journal cases.

   Result: Required missing, foreign, malformed or unsettled journals prevent every node request.
   Already-Deleted retries need no new provider credentials or GETs.

2. Run the unresolved-attempt preservation case.

   Result: Only the known receipt supplies an API ID.
   Its success preserves the unresolved attempt exactly and leaves a manual outcome.

3. Run the independent saved-tailnet binding case.

   Result: The same node ID under another saved binding remains separate.
   Confirmation never infers another tailnet as an alias.

4. Run the None, missing identity and cancellation cases.

   Result: None without evidence sends no API request.
   Missing identity remains manual; cancelled retry cannot dispatch.

### 6.3 C03 — First-head and final-head real inputs

1. Record actual captured Self.ID receipts from the authorized task cloud.

   Result: Worker replacement can add another worker-bound receipt for the same saved-tailnet/device group.

> **CAUTION:** DELETE ONLY THE EXACT OWNED CLOUD.
> The command deletes its resources and can remove its recorded tailnet node.

2. Use the existing owner CLI delete entrypoint for that original cloud.

   ```text
   cloud_deploy delete TASK_SETTINGS TASK_CLOUD_ROOT
   ```

   Result: Provider deletion precedes API dispatch. The returned result identifies success or retained exact-ID uncertainty.

3. Examine original provider absence and exact node evidence with authorized read-only adapters.

   Result: Compute and managed storage are absent. Protected baseline resources and nodes remain unchanged.
   A confirmed result persists in the original ledger.

4. Read the existing companion MCP contract.

   Result: Stop, resume, uncheck and read operations keep their original grants. No cloud-delete operation is added.

5. Repeat C03 on the final public head.

   Result: Both real-input runs bind exact source, artifact and task-resource identities.

### 6.4 C04 — OFF and ON removal custody

1. Run the default-feature regression without enabling node cleanup.

   ```bash
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-core --lib disabled_node_cleanup
   ```

   Result: A Deleted record without a worker specification keeps its original refusal without provider or API credentials. Original receipt bytes and prior normal-removal behavior remain unchanged.

2. Run the enabled locked-record removal regression.

   ```bash
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-core --features tailnet-node-cleanup --lib original_record_removal
   ```

   Result: Pending receipts refuse removal, confirmed groups permit it and unresolved attempts remain unchanged and prevent removal.

3. Check both feature configurations with the required blocking and strict tiers.

   Result: The backend is not activated by default. No disabled branch references an unavailable cleanup module.

Native pending/retry UI and explicit-discard warning checks belong to the final UI/activation outcome.
They must qualify its first and final actual candidate before default activation.

## 7. Pass criteria

- Only captured exact IDs reach the SDK after original provider absence.
- Confirmation persists and deduplicates only the same saved-tailnet/device group.
- Unresolved attempts, other bindings and uncertain results remain retained.
- Credentials remain desktop-only and failure messages contain no raw API body.
- Required matrix and applicable first-head/final-head live lanes pass.
- OFF/ON removal checks pass; native pending/retry/card-retention evidence is required for final activation.
- Unsupported provider lanes remain explicitly unqualified.

## 8. Cleanup

1. Reconcile uncertain results through the original owner controller.

   Result: A new root, successor, name or peer cannot replace original authority.

2. Preserve unresolved receipts and original resource records.

   Result: Manual cleanup remains possible without an inferred node ID.

Any remaining resource cleanup uses its original authorized procedure and exact owned IDs.

## 9. Record of results

Use the [report template](../reports/TEMPLATE.md) for evidence that must remain.
Record each provider and credential lane separately, including blocked or non-applicable cases.
Keep private IDs, credentials and raw responses outside the repository.

## Docker initial capacity failure regression

With `tailnet-node-cleanup`, run the focused
`cloud_runtime::deployment::redeploy::tests::docker_host` Rust tests. An owned fake
Docker executable drives the actual SDK capacity refusal and Core deletion
controller without contacting Docker or a provider. Check repeated delete/reload,
normal removal and redeploy after initial capacity failure, including volume-only
preparation. Keep legacy readiness/stop/resume, Requested, synthetic Terminated
and port-intent cases manual. An unexpected container found by any of the three lookups must save Requested.
The shared SDK observation path covers removal and final verification. A
final-only observation followed by absence before retry must still block normal
removal and redeploy. Failed saves send no rm at all three lookups; lost removal
replies remain uncertain after reload/retry. These cases check the Core adapter
and SDK transitions, not a claimed live Docker qualification.
