---
procedure: azure-arm-client
feature: The Azure Resource Manager client and its read-only reconcile
platforms: [linux]
cost: none
destructive: no
secrets: synthetic fixtures only
owner: peters
---

# Azure ARM client test procedure

## 1. Purpose

This procedure makes sure that the ARM client of `horizon-cloud` sends only bounded and valid requests.
It also makes sure that the client classifies each ARM answer correctly and that reconcile only reads.

## 2. Applicability

- Candidate: a pull request of #1774 that adds to the module `horizon_cloud::azure`.
  The first of these pull requests adds the ARM transport, its error classification and the resource group read.
  The second adds the VM read and the reconcile by name (ARM-04 to ARM-06).
- Platform: Linux with the repository Rust toolchain.
- The tests use a scripted agent and synthetic data only.
  They need no network, no socket, no Azure account and no Azure credential.
- No task makes, changes or deletes an Azure resource.
- No test sleeps.
- The trust boundary of the client is in [the ARM client architecture](../../architecture/azure-arm-client.md).
- This procedure does not test these items. Later pull requests of #1774 add them:
  - The deployment and the operation poll.
  - The provisioning, stop, resume and delete of an Azure worker.
  - The Azure credential source and its token refresh.
  - The deployment template, the VM size catalog and Azure prices.
  - Azure in the UI, the CLI and the MCP tools.
- This procedure does not examine a live ARM endpoint.

## 3. Safety

No step rents compute, deletes data, sends a secret or changes access.
The tests send no request to a network address.

## 4. Equipment and preconditions

- A checkout of the candidate commit.
- The repository's supported Rust toolchain.
- A POSIX shell.
- No Azure account, Azure credential or provider key.

## 5. Setup

1. Open a terminal in the candidate checkout.

   Result: `Cargo.toml` names the Horizon workspace.

2. Build the tests of `horizon-cloud`.

   ```sh
   cargo test -p horizon-cloud --lib --no-run
   ```

   Result: The build succeeds.

## 6. Tasks

Do the tasks in order. Each task has a stable ID for the report.

### 6.1 ARM-01 — Check the request bounds and the refusal order

1. Run the request bound tests.

   ```sh
   cargo test -p horizon-cloud --lib -- azure::tests::transport::a_production_agent_pins_https_no_redirect_and_the_request_timeout azure::tests::transport::a_malformed_subscription_id_is_refused_before_any_request azure::tests::transport::a_bad_resource_group_or_name_is_refused_before_any_request azure::tests::transport::a_cancelled_call_sends_nothing azure::tests::transport::a_resource_group_body_over_the_limit_is_an_invalid_response
   ```

   Result: 5 tests pass. These tests examine the items that follow:
   - The production agent accepts HTTPS only, follows no redirect and has a request timeout of 30 seconds.
   - `loopback` refuses an address that is not a loopback address.
   - A subscription id that is not a lowercase UUID gives `Invalid` before the client exists.
   - A bad resource group or resource name gives `Invalid` and sends no request.
   - A cancelled call gives `Cancelled` and sends no request. This applies to the reads and reconcile.
   - A success body of 4 MiB or more gives `InvalidResponse`.

### 6.2 ARM-02 — Check the identity of each answer

1. Run the identity tests.

   ```sh
   cargo test -p horizon-cloud --lib -- azure::tests::transport::resource_group_reads_are_typed_bounded_and_identity_checked azure::tests::transport::vm_resource_ids_are_matched_exactly
   ```

   Result: 2 tests pass. These tests examine the items that follow:
   - A group 404 gives `Ok(None)`.
   - A group answer with the name or id of another group or subscription gives `IdentityMismatch`.
   - A group answer without a name or with a body that is not JSON gives `InvalidResponse`.
   - A redirect gives `Http(302)`. The client does not follow it. The error does not show the body.
   - A VM resource id must name the same subscription, group, provider, type and VM name.

### 6.3 ARM-03 — Check the error classification

1. Run the classification tests.

   ```sh
   cargo test -p horizon-cloud --lib -- azure::tests::transport::conflicts_are_uncertain_and_throttles_are_definite azure::tests::transport::arm_authentication_failures_are_not_rejections azure::tests::transport::capacity_codes_are_capacity_and_nothing_was_created azure::tests::transport::failure_bodies_are_bounded_and_never_echo_the_token
   ```

   Result: 4 tests pass. These tests examine the items that follow:

   | ARM answer | Result | Definite |
   |---|---|---|
   | 409 `Conflict`, `AnotherOperationInProgress`, `ResourceGroupBeingDeleted`, `ScopeLocked` or no body | `Http(409)` | no |
   | 429 | `Http(429)` with its `Retry-After` | yes |
   | 500 or 503 | `Transport` | no |
   | 401, or 403 `AuthenticationFailed`, `AuthorizationFailed` or `ExpiredAuthenticationToken` | `Unauthorized` | yes |
   | Other 403, other 400 | `Rejected` with the reason | yes |
   | 400 or 409 `SkuNotAvailable`, `QuotaExceeded`, `ZonalAllocationFailed` or `AllocationFailed` | `Capacity` | yes |
   | 400 `InvalidTemplateDeployment` with a capacity code in `details` | `Capacity` | yes |
   | 500 with a capacity code | `Transport` | no |

   The client sends each request one time. It does not repeat a request after a 409.
   A reason that contains the credential, also with characters between its letters, is removed.
   A failure body of more than 8 KiB gives no reason.

### 6.4 ARM-04 — Check the VM view

1. Run the VM view test.

   ```sh
   cargo test -p horizon-cloud --lib -- azure::tests::transport::virtual_machine_views_carry_identity_instance_power_and_disks
   ```

   Result: 1 test passes. This test examines the items that follow:
   - A VM view keeps the resource id, `vmId`, size, power state, tags and data disks.
   - A data disk without a disk id is counted but not named.
   - A `vmId` that is not a lowercase UUID gives `IdentityMismatch`. A VM answer with another name gives `IdentityMismatch`.
   - A VM 404 gives `Ok(None)`.

### 6.5 ARM-05 — Check the reconcile rules

1. Run the reconcile tests.

   ```sh
   cargo test -p horizon-cloud --lib -- azure::tests::reconcile::reconcile_proves_absence_with_one_read azure::tests::reconcile::reconcile_adopts_a_matching_group_and_vm azure::tests::reconcile::reconcile_refuses_a_foreign_group_and_a_wrong_vm_name
   ```

   Result: 3 tests pass. These tests examine the items that follow:
   - The group of operation `op-1` is `horizon-ws-op-1`.
   - A group 404 gives `Absent`. The client sends no second request.
   - An operation id that cannot name a group gives `Invalid` and sends no request.
   - A group with the tag `horizon-operation: op-1` and a VM 404 gives `GroupOnly`.
   - A group with that tag and the VM `worker` gives `Created`. The result keeps the `vmId` of the VM.
   - A group with the tag of another operation, or without the tag, gives `IdentityMismatch`. The client does not read its VM.
   - A VM with another name, or with a resource id in another group, gives `IdentityMismatch`.

### 6.6 ARM-06 — Check that reconcile only reads

1. Run the read-only test.

   ```sh
   cargo test -p horizon-cloud --lib -- azure::tests::reconcile::reconcile_never_writes
   ```

   Result: 1 test passes.
   Reconcile gives `Absent`, `GroupOnly`, `Created`, `IdentityMismatch`, `Http(409)` and `Transport` in six calls.
   Each of the 8 recorded requests is a GET.

2. Make sure that the module has no write path.

   ```sh
   grep -rn "Method::" crates/horizon-cloud/src/azure.rs crates/horizon-cloud/src/azure/*.rs
   ```

   Result: The output shows only `Method::Get`: the call in `get_json_within` and the arm of `send_within`.
   The module has no PUT, POST or DELETE request.

## 7. Pass criteria

1. Run all tests of the module.

   ```sh
   cargo test -p horizon-cloud --lib -- azure::
   ```

   Result: 16 tests pass.

- All tasks pass on the candidate commit.
- No request leaves the scripted agent. No test sleeps.
- A 409 is never definite, and the client never sends a request a second time.
- Reconcile sends GET requests only.
- No error text contains the synthetic credential.

## 8. Cleanup

No cleanup is necessary. The tests make no file outside the build directory.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
