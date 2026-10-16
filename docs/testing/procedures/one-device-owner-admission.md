---
procedure: one-device-owner-admission
feature: Optional Core one-device owner controller prerequisite
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# One-device Core owner controller test procedure

## 1. Purpose

Test Armed-before-dispatch, original Store custody, current-recipient checks and
physical cancellation/retirement in the optional Core controller.

## 2. Applicability

Use a candidate changing the Core controller or its private record transitions.
Enable `one-service-gateway` explicitly. Enable the already existing `cloud-exposure`
pure-record feature for the passive sibling-field tests. Defaults remain off.
This procedure qualifies component behavior only; it does not qualify native
owner approval, actual LAN/tailnet input, worker SSH or a public interface.
Read [the boundary](../../architecture/one-device-owner-admission.md).

## 3. Safety

Use synthetic records and task-owned temporary paths. The declared tests need no
provider, credentials, Tailscale client, enrollment, policy change or paid equipment.
Do not pass `--ignored` or substitute a host worker for the synthetic helper.

## 4. Equipment and preconditions

- Exact candidate source, locked dependencies, toolchain and enabled features.
- Repository build prerequisites and task-private HOME, TMP and XDG paths.
- Trusted root-owned sticky `/tmp` for the private Unix ingress tests. If the host
  maps its ownership differently, use the established isolated exact-binary recipe;
  preserve the refusal instead of weakening the production guard. Physical private
  fixtures are Linux-only. Store-backed admission and sibling-field tests require
  Unix directory durability. Pure generation/state checks remain portable; non-Unix
  hosts must retain the existing fail-closed Store entry coverage.

## 5. Setup

1. Record all consumed source/configuration and dependency inputs, flags and environment.

   Result: Each check identifies its exact candidate; historical combined source is not proof.

2. Compare local producers before reusing the task build cache.

   Result: Only changed task-owned package products are invalidated; third-party cache remains.

## 6. Tasks

### 6.1 O01 — Durable admission and original identity

1. Run focused controller regressions.

   ```sh
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-core --lib --features one-service-gateway cloud_runtime::local_network::endpoint::admission::
   ```

   Result: Armed is durably saved while the original Store lock is held before
   dispatch. Failed startup preserves it. Unresolved attempts refuse replacement.
   Worker, endpoint, image, identity-path, full launch or owner drift refuses or revokes.
   Stale generation/foreign owner cannot close or alter the retained attempt,
   including `finish`: the Ready handle, active source, original lock and durable
   Attached bytes remain unchanged until a valid finish or separate owner Drop.

### 6.2 O02 — Physical cancellation and retirement

1. Examine the focused cancellation and exact-finish results from O01.

   Result: Already-cancelled jobs dispatch nothing. Shared cancellation revokes the
   source without the owner loop. A deliberately blocked record read cannot delay
   physical close; its original Store remains held until that observer retires.
   Drop preserves Attached uncertainty. Exact finish reaps first, then saves Closed.
   Only a Closed record permits a fresh explicit admission.

### 6.3 O03 — Enabled composition and default behavior

1. Run the full enabled Core library and existing metadata/bridge regressions.

   ```sh
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-core --lib --features one-service-gateway
   ```

   Result: Current Engine DNS/source checks, private transport, retained validation,
   lifecycle and migration behavior pass together. Live ignored cases remain unexecuted.

2. Run the applicable repository matrix in AGENTS.md, plus enabled Core required tiers.

   ```sh
   cargo clippy --locked --offline -p horizon-core --all-targets --features one-service-gateway -- -D warnings
   cargo clippy --locked --offline -p horizon-core --lib --examples --features one-service-gateway -- -D warnings -D clippy::unwrap_used -D clippy::expect_used
   ```

   Result: Blocking/strict checks pass for both applicable default inputs and the
   new opt-in surface. Advisory results are recorded separately. Matching input
   closures support reuse; a cache flag or old combined result alone does not.

3. Inspect feature defaults and caller paths.

   Result: The feature is still off by default. No UI/CLI/MCP route invokes this
   controller. Complete M1 still requires its combined public owner-flow qualification.
   A decoded scope or record cannot restore access.

### 6.4 O04 — Original aliases and passive sibling fields

1. Run the original-custody regressions with the existing passive format feature.

   ```sh
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-core --lib --features one-service-gateway,cloud-exposure cloud_runtime::owner_custody::
   ```

   Result: Fresh admission and exposure-field transactions preserve sibling fields
   and every other deployment fact. Stale fields, foreign owner, missing record and
   worker drift cannot save. The original lock outlives every live alias. On Linux,
   physical cancellation closes the exact source while a record read is blocked.
   These Store-backed cases run on Unix; the physical case runs on Linux only.
   On non-Unix, enabled production still compiles while Store acquisition refuses.
   These tests activate no exposure runtime, keeper or provider operation.

2. Include `one-service-gateway,cloud-exposure` in the enabled blocking and strict
   checks so the passive sibling transaction surface is checked too.

   Result: Required tiers cover both private admission and original field custody.

### 6.5 O05 — Unsupported original Store acquisition

1. On a non-Unix host, run the existing Store entry refusal with the enabled surface.

   ```sh
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-core --lib --features one-service-gateway,cloud-exposure cloud_runtime::state::tests::unsupported_cloud_control_does_not_create_or_modify_state -- --exact
   ```

   Result: No new state directory or operation lock is created, and existing record
   bytes remain unchanged. The same regression also runs in default Windows CI.
   Without a Store, no live OriginalCloud or admitted owner controller can be made.
   Keep the pure exact-generation admission test in the portable gateway CI command.
   Do not bypass the production guard or label unsupported live custody as passing.

## 7. Pass criteria

All applicable component checks pass on the exact candidate, with raw environment
refusals and exact-binary complements distinguished. No record, Attached status,
ignored test or synthetic helper result is reported as actual owner/worker Ready.

Completed M1 still requires the native owner path, actual LAN and tailnet checks,
original worker SSH, UI/CLI/MCP parity, failure/cleanup and first/final native GIF.
Those are later implementation and separately authorized qualification lanes;
this component procedure does not authorize their equipment or imply they passed.

## 8. Cleanup

1. Let each synthetic original controller retire and remove only its owned test paths.

   Result: Source relays and owned helper processes stop. Isolated equipment has
   positive cleanup evidence; unresolved retained attempts are not rewritten as Closed.

## 9. Record of results

Keep exact source/artifact hashes, commands, enabled features, raw logs, reused
input comparisons, advisory findings and cleanup proofs. Component evidence is
not transferred to later native/public activation without the required input and
behavior comparison. No runtime result is recorded by adding this procedure.
