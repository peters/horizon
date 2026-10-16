---
procedure: worker-exposure-gate
feature: Default-off bounded worker TCP gate
platforms: [linux]
cost: none locally; approved worker component lane may rent compute
destructive: no
secrets: none in gate inputs
owner: peters
---

# Worker TCP gate test procedure

## 1. Purpose

Test actual-peer CIDRs, bounded raw-TCP relay and listener/flow closure.
Keep unsupported identity modes and default activation closed.

## 2. Applicability

- Use this procedure when protocol policy or worker gate code changes.
- Keep `exposure-gate` off in default features and every public dispatcher.
- This procedure does not qualify named/tag identity, provider mapping, owner control or interface parity.

## 3. Safety

Local tests use isolated loopback sockets and synthetic payloads.
The real-worker case is ignored and needs the approved task-owned root-worker lane.
Use only synthetic PING, PONG and DONE bytes.
Do not run that case on a user's machine or expose an existing service.
The gate adds no TLS or authentication.

## 4. Equipment and preconditions

- Exact candidate source, locked dependencies and recorded Rust toolchain.
- Private HOME, TMP and XDG directories.
- The established isolated test container with its recorded UID, root-owned temporary directory and no external network.
- For the ignored case, a task-owned Linux worker and a separate approved test client.
- A recorded resource cost cap and cleanup owner before any rental.

## 5. Setup

1. Record source, features, toolchain, environment and artifact hashes.

   Result: Each result has an exact consumed-input identity.

2. Compare local producer inputs before reusing a shared target.

   Result: Changed local packages rebuild without stale artifacts from another worktree.

3. Keep the default feature graph and public dispatch unchanged.

   Result: Compiling the engine does not open a listener.

## 6. Tasks

### 6.1 G01 — Policy boundary

1. Run the protocol tests with the feature explicit.

   ```sh
   RUSTFLAGS="-D warnings" cargo test -p horizon-cloud-protocol --features exposure-gate exposure::
   ```

   Result: Default Off, empty restrictions, malformed bounds and protected targets fail closed.
   Shared syntax accepts valid DNS/tag/TailnetOnly comparison facts; G02 must prove this worker still refuses those modes before binding, including mixed source lists.
   IPv4, IPv6 and mapped-address CIDR boundaries remain exact.

### 6.2 G02 — Actual socket relay and bounds

1. Run the gate tests in the isolated environment.

   ```sh
   RUSTFLAGS="-D warnings" cargo test -p horizon-cloud-worker --features exposure-gate exposure_gate::
   ```

   Result: The original accepted peer controls CIDR admission before backend dialing.
   A forged PROXY line grants no access.
   Raw bytes, request half-close, connection/source/byte limits and monotonic timers pass.
   The actual-worker case stays ignored.

2. Examine the source-history and pending-admission cases.

   Result: The 257th new source is refused without forgetting old accounting.
   Cancellation before a task's first poll releases its admitted slot.

### 6.3 G03 — Close and drop

1. Examine explicit close and drop cases from G02.

   Result: Existing client and backend streams end, and new connections fail.
   Explicit close drains tracked tasks and retires the exact owned listener descriptor.
   TCP TIME_WAIT is not mistaken for retained listener custody.

### 6.4 G04 — Applicable matrix

1. Run the full local matrix in AGENTS.md on the stable candidate.

   Result: Default, speech, blocking and strict checks cover the current source and embedded guidance.

2. Run enabled protocol/worker tests and enabled blocking/strict Clippy with `exposure-gate` explicit.

   Result: The feature's Rust surface is checked while defaults remain off.
   Record advisory diagnostics separately.

### 6.5 G05 — Approved actual-worker component

1. Place the exact frozen enabled worker test artifact on the approved task-owned worker.

   Result: Its source, compiler, features and hash match the reviewed candidate.

2. Set `HORIZON_GATE_COMPONENT=task-owned-worker` and `HORIZON_GATE_SOURCE_CIDR` to the approved client's exact CIDR.
   Run only `exposure_gate::tests::actual_task_owned_worker_gate` with `--exact --ignored --nocapture` as root.

   Result: The production constructor binds port 20000, with a synthetic loopback backend and a 30-second cap.
   The ready marker appears without any owner, API or provider credential in gate input.

3. From a separate wrong source, attempt a connection before the admitted client sends data.

   Result: That source cannot reach the synthetic backend.

4. From the admitted client, send PING, read PONG, then send DONE on the same connection.

   Result: Both directions carry the expected bytes, and explicit close prints its closed marker.

5. Observe stream closure and listener refusal independently from the client.

   Result: No accepted stream or listener remains after the close acknowledgement.
   Record the actual TCP peer and any provider translation; infer no original-source preservation from a configured CIDR alone.

## 7. Pass criteria

- G01–G04 pass on the stable candidate.
- G05 passes on first-reviewable and final inputs when a real-worker check is required.
- No result claims named/tag admission, provider source preservation or completed M2.
- Default features and all public dispatch remain off.

## 8. Cleanup

1. Remove only exact task-owned test containers, artifacts and worker resources from the approved lane.

   Result: Their recorded identities are absent, and user state is unchanged.

## 9. Record of results

Record new, reused and non-applicable results separately.
Keep raw logs, hashes, ignored cases and cleanup evidence private.
Use the [report template](../reports/TEMPLATE.md) for a permanent real-worker report.
