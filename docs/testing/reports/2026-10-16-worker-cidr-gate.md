---
procedure: worker-exposure-gate
candidate_commit: ff3214dd1990b96ff6119223a675e091b48a157a
candidate_sha256: 26647988ccfabecded17e780acaa20f223cd1c871bcba27354f77961aa695a4a
date: 2026-10-16
lanes: [linux-offline, linux-root-cidr-component]
issue: https://github.com/peters/horizon/issues/1335
---

# Worker CIDR gate test report, 2026-10-16

## 1. Summary

The current run passed G01–G05 for the default-off Linux component.
The candidate hash identifies the feature-enabled Worker test ELF, not a normal Worker executable.
The actual root constructor used the original accepted peer for CIDR admission.
The separate client completed PING, PONG and DONE on one connection.
The run did not qualify provider source preservation, named/tag identity, TailnetOnly, public interfaces or complete M2.
Exclusive ownership ends when the listener closes; separate root-only ingress and backend custody proof is required before provider/public activation.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| G01 | pass, reused | The unchanged enabled protocol passed 49 tests. Its documentation test passed again on the current inputs. | — |
| G02 | pass, new | The enabled Worker passed 235 tests, including 24 gate passes; four tests stayed explicitly ignored in the general lane. Five prior reservation/cancellation regressions and seven new real-socket grace regressions passed. The separate root lane ran the remaining ignored gate case. | — |
| G03 | pass, new | Accepted reservations reached the kernel before bounded half-close and input drain. Slow readers received every allowed byte and clean EOF after actual flow/socket-owner release despite unread reverse input. Idle siblings and completed-copy races entered grace. Cancellation, deadline expiry and peer resets remained immediate, with no refunds or deadline extension. | — |
| G04 | pass, new and reused | The current default Worker passed 210 tests; three stayed ignored. Enabled and default blocking/strict checks, formatting, maintainability and whitespace checks passed again. Unchanged non-Worker results from the prior full matrix were reused after input comparison. | — |
| G05 | pass, new | The current enabled test ELF ran the exact ignored actual-worker case as root. The wrong source was refused; the admitted client exchanged bytes, reached EOF and observed refusal after the close marker and test exit. | — |

The prior full default workspace result was 7,003 passes and 55 ignored tests.
The prior speech result was 2,411 UI and five integration passes, with three ignored tests.
The current candidate reuses the unchanged non-Worker source, dependency, feature, toolchain and embedded-guidance inputs of those results; it did not repeat that full matrix.
The Worker is a binary with no reverse workspace package dependencies. All changed Rust is inside its default-off gate module.
The private input comparison records the exact closure and distinguishes the fresh package graphs from the prior workspace graph.
One custom live WebAuthn harness remained outside the offline matrix.

The pedantic tier retained two unchanged dependency structures with too many Boolean fields.
The current Worker passed a separate pedantic check without dependency diagnostics.
No suppression was added.
The general unit lane kept G05 ignored; the separate root lane ran that exact case once on the current test ELF.

## 3. Defects

Review first found that budget exhaustion could truncate outstanding reservations and that listener shutdown could race the cancellation notification.
A second review identified that kernel write acceptance was being treated as peer receipt: shutdown and socket release with unread reverse input could reset the connection and discard queued output.
The preserved current-code reproducer delivered only 1,152 of 8,192 response bytes before reset.
The candidate separates immediate failure closure from budget grace, drains inputs to EOF under the original deadlines and a fixed three-second/64 KiB-per-direction bound, then releases sockets normally.
All five prior and seven new regressions passed; the decisive slow-reader cases received all 8,192 bytes and clean EOF after the actual owners released.
Conservative charging, admission refusal, bounded buffers and immediate failure cancellation remain in place.

## 4. Deviations from the procedure

The passing current G05 run had no cleanup deviation.
An initial current-ELF attempt stopped before start because its orchestration required a populated Docker attachment NetworkID before containers ran.
Both created containers and their network were positively removed; no gate test ran in that attempt.
After correcting that phase-specific assertion, one newly authorized nonce completed the fresh G05 run and cleanup.
Earlier pre-review evidence retained a cleanup-summary error caused by Docker's network-not-found wording.
Its original removal receipts and independent read-only reconciliation proved that earlier cleanup; the raw summary remains historical evidence.
The current changed Rust used a fresh G05 run and did not reuse that earlier runtime result.

## 5. Cleanup

Both exact current test containers and their internal network were deleted.
Healthy fixed-daemon exhaustive container and network inventories showed that all three original resource IDs were absent.
The fixture used no host ports, provider resources, credentials or paid capacity.

## 6. Evidence

The report metadata identifies the reviewed fix commit and the current enabled test ELF.
The subsequent report-only commit changes prose; its executable-input comparison permits reuse of the recorded checks.

Current private evidence stays under `.task-1335-work/worker-gate-round2-fixes/`:

- `final-handoff.json`, `current-check-coverage.json` and the final source patch record the checked inputs and affected results.
- `input-closure-reuse.json` records the exact reuse comparison for the previous full matrix.
- `enabled-tests-final/`, `default-tests/` and the required lint directories record fresh affected checks.
- `products/enabled-manifest.json` binds the actual feature-enabled test ELF and compiler graph.
- `G05-actual-final-r2/command-effect-ledger.json`, `test-output.log` and `result.json` record the fresh actual component run and positive exact-resource cleanup.
- `failing-slow-reader/` preserves the expected pre-fix truncation failure; `G05-actual-final/` preserves the cleaned prestart orchestration failure.
- `final-prose-reuse-proof.json` records the comparison after this report update.

The earlier full-matrix and pre-review runtime evidence remains under `.task-1335-work/worker-gate-delivery-qualification/`.
