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

Use the candidate with the NodeApi SDK, backend cleanup and final UI activation.
Core `cloud-workspaces` enables `tailnet-node-cleanup` for both default UI and owner CLI builds.
Existing owner Delete and cloud_deploy delete use this controller.
No public companion MCP cloud-delete lane exists.
Use the production candidate with cloud-workspaces for native retry and card retention checks.
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
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-core --lib cloud_runtime::tailnet::cleanup
   ```

   Result: Confirmation persists before the next request. Retry skips confirmed exact groups.
   Missing, denied, offline and 404 results retain exact IDs.

2. Run the existing tailnet and allocation tests.

   ```bash
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-core --lib cloud_runtime::tailnet
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-core --lib cloud_runtime::allocation
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-core --lib cloud_runtime::deployment::redeploy
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

### 6.4 C04 — Card retention and explicit owner discard

1. Run the actual close and removal regressions.

   ```bash
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-ui --features cloud-workspaces node_cleanup
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-ui --features cloud-workspaces cleanup_retry
   ```

   Result: Normal Remove and automatic close retain the original pending card and exact-ID failure.
   Stale confirmed snapshots cannot override the locked record. Unknown legacy identity stays manual.
   Initial and final checks remain asynchronous. Periodic display advice retains its settled controls.
   Cancel, context changes and retained Ready watches keep their existing lock and generation checks.
   Only a matching displayed saved-tailnet/device pair retains its previous API detail.
   A delayed Retry settings read leaves UI frames responsive and keeps the operation busy.
   Cancellation or malformed settings prevent provider/node dispatch.
   Normal event polling retains the Retry display context through every failure.
   Recorded and legacy cases retain Deleted, the error and Retry after settings failure, cancellation or lifecycle lock contention.
   Invalid launch identity also retains these controls without a settings read or dispatch.
   A later generic error does not hide the legacy missing-ID explanation.
   An empty Deleted snapshot does not settle legacy cleanup.
   A second delayed Retry stays responsive. Both attempts preserve the saved record.
   Authoritative Stage, Ready and settled recorded snapshots clear obsolete display context.
   The locked Store still refuses removal of its unresolved record after the display context clears.
   Opening Close directly after a legacy restore still shows the explicit-discard warning.

2. In the isolated desktop, open Manage for the original pending cloud.

   Result: Retry node cleanup is available. The card and existing cloud_list read show attention without a new MCP delete grant.
   The isolated settings-failure case starts with Manage open and valid owned settings.
   A settings error retains Deleted and Retry. Exact settings restoration permits another asynchronous Retry.
   During the isolated lock-contention case, only the original task record lock stays held during Retry.
   The busy error retains Deleted and Retry. A later Retry starts after that exact lock releases.
   Recorded pending cleanup and legacy missing identity have the same failure-retention checks.
   The evidence preserves all original deployment and selection bytes before fixture cleanup.

> **CAUTION:** RETRY ONLY THE EXACT TASK-OWNED NODE CLEANUP.
> Retry can remove its recorded node. Keep the original cloud record and unrelated devices.

3. Select Retry node cleanup after the authorized desktop API binding becomes available.

   Result: The original controller retries pending groups without a new rental or successor lookup.
   Only confirmed saved groups release normal removal.

4. In the isolated synthetic fixture, inspect Remove from Horizon anyway with a pending ledger.

   Result: The explicit owner warning explains that removal preserves the record but hides retry controls.

> **CAUTION:** DISCARD ONLY THE EXACT SYNTHETIC FIXTURE CARD.
> This action hides its retry controls while resources or node cleanup can remain unresolved.

5. Select Remove from Horizon anyway on that synthetic card.

   Result: Its original locked resource record and pending ledger remain unchanged. No node API request occurs.

6. Capture actual native frames and a short GIF for the first and final candidate heads.

   Result: Evidence shows pending status, retry and retained card behavior on exact frozen artifacts.

## 7. Pass criteria

- Only captured exact IDs reach the SDK after original provider absence.
- Confirmation persists and deduplicates only the same saved-tailnet/device group.
- Unresolved attempts, other bindings and uncertain results remain retained.
- Credentials remain desktop-only and failure messages contain no raw API body.
- Required matrix and applicable first-head/final-head live lanes pass.
- Native pending/retry/card-retention evidence passes on first and final heads.
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
