---
procedure: trusted-tcp-producer
feature: Default-off original TCP producer
platforms: [linux]
cost: rents compute
destructive: no
secrets: existing approved tailnet credential references only
owner: peters
---

# Original TCP producer test procedure

## 1. Purpose

Test the original daemon and stream proof without a Gate or public exposure operation.

## 2. Applicability

Use a candidate with the Linux Worker `trusted-tcp-producer` feature.
This procedure does not qualify a protected keeper, source preservation, native approval or complete M2 behavior.

## 3. Safety

> **CAUTION:** USE ONLY THE APPROVED TASK RESOURCES.
> Real-daemon tests can rent compute. Keep the exact cost cap and cleanup deadline in the approved run plan.

Use only a dedicated test tailnet and the original approved credential references.
Keep credential values and private node identities out of repository evidence.
This component does not enroll or delete devices.

## 4. Equipment and preconditions

- Use a separate checkout and private local test state.
- Use a reviewed original-daemon plan for each Worker and Kernel mode.
- Keep the original daemon, peer, process and resource handles.
- Use the fixed runtime paths and pinned CLI/daemon executables.
- Use the exact reviewed Unix Serve PROXY-v1 runtime with `--horizon-flow-cancel`. Stock v1.102.5 and ordinary Unix PROXY1 do not supply the required original cancellation descriptor.
- Keep both captured peer facts and genuine original process/socket observations.
- Include a same-daemon SOCKS and HTTP CONNECT spoof attempt. A foreign-process connection alone does not cover this path.

## 5. Setup

1. Read `docs/architecture/trusted-tcp-producer.md`.

   Result: Kernel custody and parsed comparison facts have separate roles.

2. Record the candidate source, features, toolchain and actual test executable.

   Result: The run uses one frozen candidate with its SHA-256.

## 6. Tasks

### 6.1 T01 — Test bounded local behavior

1. Run the focused component tests.

   ```bash
   cargo test --locked --offline -p horizon-cloud-worker --features trusted-tcp-producer trusted_tcp_producer::
   ```

   Result: Applicable tests pass. The real-daemon selector stays ignored.
   Foreign sockets, stale descriptors and invalid headers refuse before identity admission.
   Cancellation and capacity refusal shut payload and private control, including retained reader Arcs.
   Queued SCM_RIGHTS close after owner drop, unpolled identification and Busy refusal,
   even when opaque weak handles remain. Pending cancellation still closes both
   original endpoints after the transfer to AcceptedStream.
   Cancel also closes the legitimate first-P descriptor while the pending owner remains retained.
   Malformed later rights require final owner drop, not an unbounded cancellation drain.
   Missing/extra/truncated/late-header descriptors and wrong prefix/type/flags/credentials refuse.
   Mechanical transport fixtures do not issue an AcceptedPeer or genuine daemon identity.
   Exact DRAINED0x01 plus EOF is required, and interrupted ack progress remains with its original owner.
   Child supervisors retain capacity until the original child exits and is reaped.

2. Run the separate isolated root backend selectors.

   Use the original fixture and these exact selectors with `--ignored --exact`:

   ```text
   trusted_tcp_producer::backend::tests::original_private_listener_refuses_replacement_and_cleans_only_its_own_path
   trusted_tcp_producer::backend::tests::retired_backend_acceptance_cannot_transfer_to_a_successor_at_the_same_path
   ```

   Result: A retained acceptance refuses the successor despite the same pathname.
   The successor still accepts its own current stream. Replaced paths survive uncertain retirement; normal retirement removes the owned path.

3. Complete the applicable repository, workspace, speech and required lint checks.

   Result: Required checks pass. Record new, valid reused and non-applicable results separately.

4. Examine the two normal UI candidates that embed the bundled skills.

   Result: Both candidates contain the complete current guidance.

### 6.2 T02 — Test each genuine original daemon

1. Record the separately reviewed Worker or Kernel run plan.

   Result: The plan retains genuine original daemon, enrollment, peer and socket facts.
   It identifies only task-owned resources and declares their cleanup.

2. Start the existing ignored selector on the original owned Linux worker.

   ```text
   trusted_tcp_producer::tests::actual_task_owned_producer --ignored --exact --nocapture
   ```

   Result: The selector accepts only its explicit approved Worker or Kernel opt-in.
   It pins the fixed original daemon and owns Unix component backend `20032.sock`.
   The plan supplies original peer ID, DNS name and tags as comparison expectations.
   The Worker plan supplies the same peer's captured IP as `HORIZON_PRODUCER_EXPECTED_ADDRESS`; the selector checks its live WhoIs identity before the spoof probes.

3. Send the forged connection from the plan before the genuine tailnet connection.

   Result: A valid header from a process other than the original daemon refuses.
   The Worker selector then proves that its SOCKS and HTTP probe sockets belong to that same original daemon. SOCKS attempts to the old TCP backend and the Unix target, and HTTP CONNECT to the old backend, refuse before admission.
   The test-only ownership observer waits within its original deadline for the daemon to accept each proxy connection. A completed TCP handshake with no accepted server descriptor is not an ownership proof.

4. Send PING, receive PONG, send DONE and half-close the peer write side through the genuine original connection.

   Result: The same accepted socket retains the actual peer identity and application bytes.
   The peer ID, complete DNS name and tags match the captured original facts.
   The selector observes peer EOF, half-closes its payload response and awaits the original daemon's exact DRAINED0x01+EOF before dropping the stream.
   The peer observes all expected response bytes and EOF; the acknowledgement alone does not prove application receipt.

5. Complete the plan's half-close and cleanup observations.

   Result: With `HORIZON_PRODUCER_HALF_CLOSE=1`, the genuine peer sends PING, half-closes its write side, then receives PONG and EOF.
   The selector requires genuine identity, peer request EOF, response half-close and exact control acknowledgement; custody refusal is not a passing half-close forwarding result.
   Preserve Kernel and userspace response bytes separately, including a response larger than the frontend receive capacity in the separately reviewed runtime tests.
   Explicit retirement closes the original listener and removes only its original path.

6. Repeat T02 for the other required runtime mode.

   Result: Worker and Kernel modes have separate actual original-daemon evidence.

## 7. Pass criteria

T01 and both T02 modes pass on the first reviewable candidate and the final candidate.
The same-daemon proxy attempts must refuse attacker-supplied peer identity before admission. A passing genuine connection or foreign-process refusal cannot substitute for this regression.
Synthetic parsed facts alone do not satisfy T02.
No result claims full M2, protected publication, native approval or public activation.

## 8. Cleanup

1. Stop and wait for the exact original test processes.

   Result: Their original listeners and child processes are absent.

2. Complete the approved cleanup for task-owned equipment.

   Result: Exact owned resource IDs are absent, and the original foreign baseline is unchanged.

## 9. Record of results

Keep the raw original observations private.
Use the report template for the retained sanitized actual-daemon report.
Record exact source, executable, commands, comparison facts and positive cleanup evidence.
