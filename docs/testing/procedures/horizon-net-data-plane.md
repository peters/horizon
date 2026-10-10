---
procedure: horizon-net-data-plane
feature: Horizon Net
platforms: [linux, macos, windows]
cost: rents compute
destructive: yes
secrets: [provider credential references, private endpoint keys]
owner: peters
---

# Horizon Net data plane test procedure

## 1. Purpose

This procedure tests TCP service access between authenticated workspace nodes.
It separates local tests from real provider and TLS evidence.

## 2. Applicability

- Candidate: the exact `horizon-net` library and agent from the pull request.
- Local portable lanes: in-memory policy, TCP relay and denied access.
- Persistent revocation, expiry and restart lanes: Unix directory durability.
- Windows and other non-Unix platforms: persistent startup returns I/O
  `Unsupported` before state ownership or mutation. In-memory APIs stay usable.
- Windows configuration: user-only ACLs are the caller's responsibility. The
  crate does not validate these ACLs.
- Remote lanes: RunPod, Hetzner, Linux and macOS where an approved test host exists.
- UI confirmation and deployment require their separate procedures.

## 3. Safety

Use synthetic network names, services, nodes and application content.
Keep private endpoint keys and provider credentials outside public evidence.
Record each test resource in a private resource ledger.

## 4. Equipment and preconditions

- Rust 1.95 or later.
- The exact candidate checkout with its dependency lockfile.
- Upstream `iroh-relay` 1.3 on an approved test host.
- A test domain and operator-controlled TLS configuration for the relay.
- Approved RunPod and Hetzner resources for the remote lanes.
- A user-owned, private agent configuration and state directory on each node.

## 5. Setup

1. Record the candidate commit and agent executable SHA-256.

   Result: The report identifies the exact candidate.

2. Run `cargo test -p horizon-net`.

   Result: All local policy and transport tests pass.

3. Run `cargo clippy -p horizon-net --all-targets -- -D warnings -D clippy::unwrap_used -D clippy::expect_used`.

   Result: The crate has no blocking warnings.

## 6. Tasks

### 6.1 NET-LOCAL — TCP relay and denial

1. Examine the local transport test results.

   Result: Actual echo bytes pass through the upstream relay with UDP transports disabled.

2. Examine the denied request test results.

   Result: Unknown keys, foreign networks, undeclared services and incorrect destination nodes cannot open a backend connection.

3. Examine the service target race test result.

   Result: A target change between authorization and registration rejects the previous TCP port.

4. Examine `caller_addresses_cannot_introduce_an_unconfigured_relay`.

   Result: Probe, update and forwarding calls reject the extra relay. No connection reaches that relay.
   Bytes still pass through the configured relay.

### 6.2 NET-STATE — Persistent revocation

Run this lane on Unix. Persistent Windows operation is unsupported.
On Windows, examine `unsupported_persistent_state_preserves_existing_files_and_never_takes_ownership`
and `unsupported_persistence_preserves_live_in_memory_transport_and_policy`.
Result: Startup returns I/O `Unsupported`. Existing files and live in-memory
policy stay unchanged. No state directory or writer ownership is created.
Actual in-memory TCP bytes pass, and a local revocation closes the socket.
The persistent restart tests have explicit Windows durability ignores.

1. Examine the revocation, expiry and key rotation test results.

   Result: Existing sockets close and subsequent connections fail.

2. Examine the restart test result for each transport test.

   Result: The old configuration cannot restore revoked or expired access.

3. Examine the private state test results.

   Result: Conflicting writers, corrupt snapshots and oversized documents fail closed.

4. On Unix, examine the private file test result.

   Result: Files with mode `0644` fail; files with mode `0600` pass.

5. Examine `withdrawn_persistent_identity_starts_denied_and_reenrolls_only_by_new_authority_update`.

   Result: An agent restarts with its withdrawn identity and denies probes and service access. The original enrollment file cannot restore membership. Only a newer authenticated authority update can restore the same identity. A changed local key cannot reuse the old state directory.

6. Examine the persistent public controller tests.

   Result: Applied changes and revocations survive restart. Failed writes preserve policy and existing sessions.
   Controller clones retain exclusive writer ownership until all owners end.

7. Examine the edited enrollment test.

   Result: A higher enrollment revision cannot replace a committed withdrawal. The committed snapshot remains authoritative.

8. Examine the invalid snapshot tests.

   Result: Invalid initial enrollment and direct writes cannot publish a snapshot.
   The last valid snapshot and current policy stay unchanged.

9. Examine the generated relay configuration tests.

   Result: Parsed TOML contains only the explicit endpoint allowlist and required
   TLS settings. Escaped operator text cannot add configuration fields.

### 6.3 NET-TLS — Operator relay on TCP port 443

> **CAUTION:** USE ONLY APPROVED TEST RESOURCES. A relay host can incur charges and receive network traffic.

1. Install upstream `iroh-relay` on the approved test host.

   Result: The resource ledger contains the host ID and owner.

2. Generate relay TOML with `horizon-net relay-config --config relay.json`.

   Result: The configuration uses TLS on TCP port 443 and an explicit endpoint allowlist.

> **CAUTION:** CHANGE ACCESS ONLY FOR SYNTHETIC ENDPOINTS. An incorrect allowlist can block other relay clients.

3. Start the test relay with the generated configuration.

   Result: The relay presents a valid certificate for the test domain.

4. Set the test endpoint relay URL to the test domain.

   Result: The configuration contains no public relay, credential URL or discovery service.

### 6.4 NET-PROVIDERS — RunPod and Hetzner

> **CAUTION:** RENT ONLY APPROVED TEST COMPUTE. Record each provider ID and its deletion owner before use.

1. Start one synthetic node on RunPod and one on Hetzner.

   Result: Both resources appear in the resource ledger.

> **CAUTION:** SEND PRIVATE KEYS ONLY TO THEIR OWNED TEST NODES. Never include keys in shell logs or public evidence.

2. Install each private agent configuration on its designated node.

   Result: Unix configurations use mode `0600` and user-owned state directories use mode `0700`.

3. Set `relay_only` to `true` on both agents.

   Result: The candidate removes all UDP transports.

4. Start a loopback echo service on the Hetzner node.

   Result: The service accepts no public connection.

> **CAUTION:** GRANT ACCESS ONLY TO THE SYNTHETIC SERVICE. The grant permits TCP traffic until its absolute expiry.

5. Apply a finite grant from the RunPod node to the Hetzner echo service through the confirmed host topology.

   Result: The host records the exact change and remote acknowledgement.

6. Send a unique nonce through the named service from RunPod.

   Result: Hetzner returns the complete nonce through the relay on TCP port 443.

7. Attempt the same request with an ungranted source key.

   Result: The request fails and the backend accepts no connection.

8. Revoke the grant while a socket remains open.

   Result: The remote acknowledgement follows local socket shutdown; the socket and new connections fail.

9. Restart the receiver with its original configuration and retained state directory.

   Result: The committed revision remains active and the revoked source remains denied.

10. Repeat the nonce, denial, expiry and revocation checks in the reverse direction.

    Result: Both provider directions have current transport evidence.

## 7. Pass criteria

- Every local test passes on the exact candidate.
- The remote TLS certificate and TCP port 443 lane pass.
- Both provider directions pass actual byte transfer, denial, expiry and revocation checks.
- A remote failure does not become a successful status value.
- No private key, credential or unrelated host identifier appears in public evidence.

## 8. Cleanup

1. Stop each task-owned agent and forwarder with its normal shutdown operation.

   Result: The task listeners and sockets close.

> **CAUTION:** DELETE ONLY RESOURCES IN THE TASK LEDGER. Do not delete shared relay infrastructure or unrelated clouds.

2. Delete each disposable resource from the ledger.

   Result: Provider queries show no remaining task compute.

3. Retain the private state and sanitized evidence required for the report.

   Result: The report distinguishes local, provider, TLS and platform lanes.

## 9. Record of results

Write retained runs under `docs/testing/reports/` with the report template.
Keep executable hashes and sanitized outcome summaries in the pull request.
Keep private provider IDs, endpoint keys and configuration outside the repository.
