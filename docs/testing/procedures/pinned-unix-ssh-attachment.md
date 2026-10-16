---
procedure: pinned-unix-ssh-attachment
feature: Default-off pinned Unix SSH attachment prerequisite of issue 1335 M1-1
platforms: [linux, macos, windows]
cost: none
destructive: yes
secrets: original task-only SSH identity reference; no provider or tailnet API keys
owner: peters
---

# Pinned Unix SSH attachment test procedure

## 1. Purpose

Test the exact private Unix target, original pinned SSH, bounded lifetime and real peer bytes.
This procedure qualifies the attachment component, not owner approval or completed M1 sharing.

## 2. Applicability

- Enable `one-service-gateway` explicitly; defaults remain off.
- Linux private qualification requires the declared non-symlink `/tmp` policy.
- A Unix host with a symlink at `/tmp` refuses private ingress; record that host as unsupported.
- Non-Unix hosts retain the LAN engine and refuse SSH attachment before process start.
- Actual LAN and tailnet tasks require Linux and an already authorized task-owned worker.
- No task in this procedure rents infrastructure or enrolls a client.

Read the [trust boundary](../../architecture/pinned-unix-ssh-attachment.md).
Keep the existing [engine tasks](one-service-gateway-engine.md) in the qualification plan.

## 3. Safety

> **CAUTION:** USE ONLY THE RECORDED TEST WORKER AND OWNED PEERS.
> The real tasks start a bridge and change access on that worker.

> **CAUTION:** KEEP SECRET CONTENT OUT OF FIXTURES AND LOGS.
> Record only the original key reference and its metadata.

> **CAUTION:** NEVER REPLACE A PREEXISTING BRIDGE.
> Another session can depend on that connection.

## 4. Equipment and preconditions

- Exact source, lockfile, toolchain, feature flags and private environment records.
- A frozen enabled Core test executable with its SHA-256 and `profile.test=true` evidence.
- Root-owned sticky `/tmp`, mode 01777, for the private socket tests.
- For real tasks: an owned disposable Linux guest and original task-owned CPU worker.
- The original ready allocation, Store lock inode, settings and pinned SSH route.
- A fresh task-only SSH key; no private key payload inspection or fixture copy.
- A synthetic service on the approved peer's port 18080; port 18081 remains unapproved.
- For tailnet: actual separate Self and peer identities in the dedicated test tailnet.
- For LAN: an actual approved LAN peer and the production route/interface observations.

The service reads `M1-REAL-KERNEL-REQUEST\n` and replies after request EOF.
Its reply is `M1-REAL-KERNEL-PEER\n`.
A readiness probe must not consume its one accepted request.
The worker needs the existing Local Network Bridge helper and Python3.

## 5. Setup

1. Record the exact source and complete executable input closure.

   Result: The record includes untracked inputs, dependencies, flags and relevant nonsecret environment.

2. Freeze the actual enabled test executable from the task checkout.

   ```sh
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-core --lib --features one-service-gateway --no-run
   ```

   Result: The executable has an exact hash and actual enabled feature/profile evidence.

3. Record the original worker, guest, peer and resource identities in the private ledger.

   Result: The fixture cannot select a successor or another person's worker.

## 6. Tasks

### 6.1 A01 — Private ingress and supervisor

1. On Linux, run the enabled component tests.

   ```sh
   RUSTFLAGS="-D warnings" cargo test --locked --offline -p horizon-core --lib --features one-service-gateway cloud_runtime::local_network::
   ```

   Result: Exact private ingress, engine policy, half-close, stale receipt and source revocation tests pass.
   A stalled child cannot block heartbeat or Drop.
   A stale target cannot reach a successor at its old numeric port.
   Live ignored tests remain unexecuted.

2. Run the enabled Core tests on macOS and a supported non-Unix machine.

   Result: The existing Engine tests retain TCP behavior on both platforms.
   Linux-only private fixture modules remain inapplicable; non-Unix attachment refuses without SSH effects.
   A macOS symlink at `/tmp` causes private-start refusal, not a private-attachment pass.

### 6.2 A02 — Default behavior and required checks

1. Run the full applicable matrix from AGENTS.md on the stable candidate.

   Result: Required blocking and strict tiers pass; advisory results remain separate.

2. Run both enabled Core required tiers.

   ```sh
   cargo clippy --locked --offline -p horizon-core --all-targets --features one-service-gateway -- -D warnings
   cargo clippy --locked --offline -p horizon-core --lib --examples --features one-service-gateway -- -D warnings -D clippy::unwrap_used -D clippy::expect_used
   ```

   Result: The optional attachment and shared session/relay changes pass both tiers.

3. Examine the default graph and public callers.

   Result: The feature remains off; no UI, CLI, MCP or owner controller starts this attachment.
   Existing bridge commands and protocol bytes stay unchanged.

### 6.3 R01 — Original worker over actual tailnet

1. Prepare the private guest directory `/run/horizon-m1-ssh-qualification/tailnet`.

   Result: Its root-owned input.json is a canonical regular mode 0600 file with one link and at most 8192 bytes.
   All directory components are non-symlink and refuse other-user writes.

2. Record the exact typed fixture fields in input.json.

   Result: The file contains only these fields:
   `root_authorized_disposable_component`, `task_only_fresh_ssh_identity`,
   `task_ssh_identity_file`, `cloud_root`, `settings_path`, `original_owner_uid`,
   `cloud_id`, `worker_id`, `operation_id`, `image_digest`, `deployment_sha256`,
   `settings_sha256`, `known_hosts_sha256` and `target`.
   Both explicit equipment declarations are true; they do not represent owner approval.

The tailnet target contains source=`tailnet`, desktop_id, peer_id, peer_name and peer_ipv4.
Use actual production kernel status, route, Self, WhoIs and normalized full-name observations.
Every original settings, deployment, key and known-host leaf must pass the fixture's private metadata policy.
The selected settings key reference must equal the original key path.
The fixture hashes only nonsecret settings, deployment and known-host bytes.
It retains and checks the original Store lock inode throughout the task.
The original worker must already be Ready, and its bridge must be inactive.

> **CAUTION:** START ONLY THE AUTHORIZED ORIGINAL WORKER CONNECTION.
> This command uses the recorded task-only SSH key and changes bridge access.

3. Run the exact frozen test inside the declared guest.

   ```sh
   timeout 180s /fixture/horizon-core-tests --ignored --exact cloud_runtime::local_network::endpoint::attachment::tests::real_ssh::real_original_worker_private_ssh_relays_exact_peer_and_closes --nocapture --test-threads=1
   ```

   Result: Actual pinned SSH returns helper Ready and carries the peer reply after request EOF.
   Wrong port and destination requests fail; exact close retires the original bridge and private path.
   Original worker, record hashes, selected key inode and pins remain unchanged.
   The receipt states `native_owner_qualification=false`.

### 6.4 R02 — Original worker over actual LAN

1. Prepare the equivalent fixture under `/run/horizon-m1-ssh-qualification/lan`.

   Result: The same custody rules apply to the original allocation and pins.
   Its target contains source=`lan`, host, address, network, desktop and interface.
   These fields match actual production LAN preflight and the approved peer.

> **CAUTION:** USE ONLY THE APPROVED LAN TARGET AND ORIGINAL WORKER.
> A different target can expose another device's service.

2. Run the exact LAN fixture inside the declared guest.

   ```sh
   timeout 180s /fixture/horizon-core-tests --ignored --exact cloud_runtime::local_network::endpoint::attachment::tests::real_ssh::real_original_worker_private_ssh_relays_exact_lan_and_closes --nocapture --test-threads=1
   ```

   Result: Actual LAN policy, pinned SSH, peer bytes, EOF, refusals and exact close pass together.
   The result does not qualify native approval or public interface parity.

## 7. Pass criteria

- A01 and required A02 checks pass for the exact candidate inputs.
- Actual R01 and R02 pass on the first reviewable source and the final source before PR qualification.
- Ignored, synthetic, blocked and actual outcomes remain separate.
- Changed executable inputs require fresh affected evidence; a historical combined result is insufficient.
- Native owner, durable dispatch, saved membership and UI/CLI/MCP parity remain required M1 outcomes.

## 8. Cleanup

1. Close only the recorded attachment with its exact receipt.

   Result: Its original private source closes before supervisor retirement.

2. Examine the original worker's bridge status through the same pinned connection.

   Result: The original bridge is inactive and its private Unix path is absent.

3. Recheck the original allocation, settings, pins and key metadata.

   Result: The original worker remains; no successor or secret payload supplies the result.

4. Complete only the infrastructure cleanup from the separately authorized resource plan.

   Result: Exact owned resource absence is recorded without deleting unrelated resources.

## 9. Record of results

Use the [report template](../reports/TEMPLATE.md).
Record exact source/executable identities, first/final actual tasks and cleanup evidence.
Keep credentials, raw peer/account identities and private paths outside the repository.
