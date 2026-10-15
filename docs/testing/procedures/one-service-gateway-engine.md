---
procedure: one-service-gateway-engine
feature: One-device endpoint engine prerequisite of issue 1335 M1-1
platforms: [linux]
cost: none
destructive: yes
secrets: test-tailnet auth key references only
owner: peters
---

# One-device endpoint engine test procedure

## 1. Purpose

This procedure tests the feature-off endpoint engine and its existing TCP relay.
It records component evidence for the LAN and tailnet policies.
It does not qualify the owner controller, durable recovery or worker SSH.
M1-1 still requires those outcomes and UI/CLI/MCP parity for LAN and tailnet sharing.

## 2. Applicability

- Candidate: a change to `local_network::endpoint` or its Cargo feature.
- Platform: Linux for the real kernel-mode tailnet tasks.
- Default behavior: the current bridge remains available; the new engine stays off.
- Paid workers are not part of this procedure.

## 3. Safety

> **CAUTION:** USE ONLY THE APPROVED TEST TAILNET AND TASK-OWNED GUESTS.
> An incorrect selection can change another device's access.

> **CAUTION:** KEEP AUTH KEYS IN THE APPROVED CREDENTIAL STORE.
> A command log must not contain a key value.

> **CAUTION:** DELETE ONLY THE GUESTS AND NODES IN THE RESOURCE LEDGER.
> Other devices can contain another person's work.

## 4. Equipment and preconditions

- The exact candidate source, toolchain, dependency versions and flags.
- A frozen Core test executable with `one-service-gateway` enabled.
- Two disposable Linux guests, with separate identities in the test tailnet.
- An existing root-owned kernel-mode Tailscale daemon in each guest.
- A synthetic TCP service on the peer, on port 18080.
- A private resource ledger with the original guest and node identities.
- An actual non-Self CGNAT address with an absent or non-Self source route for R02.

The service accepts one request and replies after request EOF.
Its request is `M1-REAL-KERNEL-REQUEST\n`; its reply is `M1-REAL-KERNEL-PEER\n`.
A readiness check must not consume the service's one accepted connection.

## 5. Setup

1. Record the source, toolchain, flags and hashes in the private run directory.

   Result: The record identifies the exact candidate inputs.

2. Build the enabled Core test executable in the task checkout.

   ```sh
   RUSTFLAGS="-D warnings" cargo test -p horizon-core --features one-service-gateway --no-run
   ```

   Result: The command succeeds without a runtime test result.

3. Freeze the actual test executable with its actual feature and test-profile evidence.

   Result: The frozen file has an immutable SHA-256 and `profile.test=true`.

4. Record each owned daemon PID, UID and network namespace before a real task.

   Result: Each daemon belongs to its declared guest.

## 6. Tasks

### 6.1 E01 — Engine component tests

1. Run the focused engine tests.

   ```sh
   RUSTFLAGS="-D warnings" cargo test -p horizon-core --features one-service-gateway cloud_runtime::local_network::endpoint::
   ```

   Result: Parser, binding, relay, half-close and receipt tests pass.
   A requested name with one terminal dot matches its approved name. Another
   terminal dot, name or port is refused before lookup. The complete LAN IPv4
   set is checked before policy filters or limits it. Mixed LAN/outside answers
   and four duplicates followed by another address are refused at preflight,
   start and active admission. Active ambiguity requires new approval even after
   DNS recovers. Duplicate-only answers use one lookup. A network change during
   that lookup is still refused. Real guest tests remain ignored in this command.

2. Record each ignored test separately.

   Result: The report does not count an ignored test as executed.

### 6.2 E02 — Default behavior and LAN policy

1. Run the existing bridge tests without the engine feature.

   ```sh
   RUSTFLAGS="-D warnings" cargo test -p horizon-core cloud_runtime::local_network::
   ```

   Result: Existing LAN scope, route, Self refusal and bridge tests pass.
   The existing bridge still filters mixed answers and limits its candidates.

2. Examine the default feature graph.

   ```sh
   cargo tree -p horizon-core -e features -f '{p} {f}'
   ```

   Result: `one-service-gateway` is absent from the Core features.
   On this independent Engine base, `tailnet-node-cleanup` is also absent from
   the defaults. Cleanup activation belongs to the separate #1646 outcome.
   This Engine change preserves the base feature graph and adds no cleanup edge.

This task is component evidence. It does not prove a real LAN device's bytes.
The completed M1 owner path must test actual LAN and tailnet targets before delivery.

### 6.3 E03 — Required checks

1. Run the full applicable local matrix from AGENTS.md.

   Result: Blocking and strict tiers pass for the candidate inputs.

2. Run enabled-Core blocking Clippy.

   ```sh
   cargo clippy -p horizon-core --all-targets --features one-service-gateway -- -D warnings
   ```

   Result: The command succeeds.

3. Run enabled-Core strict Clippy.

   ```sh
   cargo clippy -p horizon-core --lib --examples --features one-service-gateway -- -D warnings -D clippy::unwrap_used -D clippy::expect_used
   ```

   Result: The command succeeds.

### 6.4 R01 — Actual tailnet peer and Self refusal

1. Create `/run/horizon-m1-engine-qualification/input.json` in the owned host guest.

   Result: The file contains exactly these typed fields:
   `disposable_guest`, `daemon_pid`, `network_namespace_inode`, `self_id`,
   `peer_id`, `peer_name`, `peer_ipv4` and `wrong_route_ipv4`.

The guest fixture sets `disposable_guest` to true.
Its actual Self ID differs from its peer ID.
The peer name is the actual full lower-case MagicDNS name without a final dot.
Each path component is root-owned, non-symlink and not writable by other users.
The regular file has mode 0600, one link and at most 4096 bytes.
The daemon runs as root in the test's recorded network namespace.
These checks identify owned test equipment; they do not grant application authority.

2. Run this exact test from the frozen executable inside that guest.

   ```sh
   timeout 60s /fixture/horizon-core-tests --ignored --exact cloud_runtime::local_network::endpoint::tests::real_kernel::real_kernel_peer_uses_production_endpoint_gate --nocapture --test-threads=1
   ```

   Result: Actual name/literal preflight, relay bytes, half-close, wrong-port refusal
   and Self refusal pass through the production endpoint gate.
   The receipt reports `qualification: "engine_component"`.
   It does not qualify an application owner or a worker.

### 6.5 R02 — Actual wrong-source route refusal

1. Record the actual route observation for the declared wrong-route address.

   Result: The address is non-Self IPv4 inside `100.64.0.0/10`.
   Its route is absent or selects a source outside the actual Self addresses.

If no address meets this condition, record R02 as blocked.
A Docker bridge address or a guessed route does not meet this condition.
Do not change a host route to make the test pass.

2. Run this exact test inside the same owned guest.

   ```sh
   timeout 60s /fixture/horizon-core-tests --ignored --exact cloud_runtime::local_network::endpoint::tests::real_kernel::real_kernel_wrong_source_route_refuses_before_peer_admission --nocapture --test-threads=1
   ```

   Result: The production preflight returns UnsupportedSource before peer admission.
   Actual Self remains unchanged.

## 7. Pass criteria

- E01, E02 and the required E03 tiers pass on the exact candidate inputs.
- R01 and R02 contain real observations, successful exits and exact test names.
- An ignored, blocked or missing result does not count as a real result.
- LAN and tailnet requirements stay in the M1 delivery checklist.
- The report does not claim owner, durable, worker or public parity qualification.

## 8. Cleanup

1. Close only the gateway with its exact receipt.

   Result: The gateway listener and its active relays stop.

2. Revoke only the task auth keys with the approved credential controller.

   Result: No key from this run can enroll another guest.

3. Stop and remove only the recorded guests and their empty task network.

   Result: Their exact resource IDs are absent.

4. Remove only the recorded guest nodes from the test tailnet.

   Result: Both original protected devices remain unchanged.

5. Remove task credential aliases last.

   Result: The original saved credentials remain accessible.

## 9. Record of results

Write the result with [the report template](../reports/TEMPLATE.md).
Record new, reused, ignored and blocked results separately.
Include exact source/executable identities, feature/profile evidence and cleanup results.
Keep raw node/account data, auth keys and private evidence outside the repository.
