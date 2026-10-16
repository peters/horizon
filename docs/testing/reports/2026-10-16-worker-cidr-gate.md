---
procedure: worker-exposure-gate
candidate_commit: cbfd117f2b0885e12a26faf50db646b8df42d0a8
candidate_sha256: 92274c30e0f5bcaaca825e0ecd1eff933030da3f5c0a60e481e06eb5f3fa12d8
date: 2026-10-16
lanes: [linux-offline, linux-root-cidr-component]
issue: https://github.com/peters/horizon/issues/1335
---

# Worker CIDR gate test report, 2026-10-16

## 1. Summary

The current candidate passed G01–G05 for the default-off Linux component after rebasing onto Main `5e868c76`.
The candidate hash identifies the feature-enabled Worker test ELF, not a normal Worker executable.
The current Worker source and manifest match the previously reviewed component; Main changed shared dependencies, Protocol and embedded guidance, so the applicable matrix and actual smoke were run freshly.
The run did not qualify provider source preservation, named/tag identity, TailnetOnly, public interfaces or complete M2.
Exclusive ownership ends when the listener closes; separate root-only ingress and backend custody proof is required before provider/public activation.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| G01 | pass, new | The current enabled Protocol passed 57 tests and one documentation test. | — |
| G02 | pass, new | Enabled Worker: 235 passes and four general-lane ignores, comprising 234 container passes and the separate host-only lease case. Gate: 24 general passes and one general-lane ignore; the actual root lane ran that ignored gate case. | — |
| G03 | pass, new | Five prior reservation/cancellation and seven real-socket grace regressions passed. Slow readers received all allowed bytes and clean EOF after actual flow/socket-owner release despite unread reverse input. Idle siblings and completed-copy races entered grace. Cancellation, deadlines and peer resets remained immediate. | — |
| G04 | pass, new | Fresh default workspace: 7,012 passes and 55 ignores across 63 offline roots plus 13 host complements. Fresh speech: 2,411 UI and five integration passes, three ignores. Required blocking/strict lints, formatting, maintainability, skill coverage and whitespace checks passed. The Board complement needed the diagnostic sequence described below. | — |
| G05 | pass, new | The exact current enabled test ELF ran the ignored actual-worker case as root. Wrong-source refusal, admitted PING/PONG/DONE, EOF, close marker plus test exit, then independent listener refusal all passed. | — |

The default container supplied 6,999 passes; the separate host complements supplied 13.
The default Worker supplied 209 container passes and one host lease pass, with three ignores.
The enabled Worker supplied 234 container passes and one host lease pass, with four ignores.
The enabled Protocol passed 57 tests on its current feature graph; the earlier 49-test result was not transferred.
Workspace documentation checks passed three tests across 22 groups; enabled Protocol documentation passed one test.
One custom live WebAuthn harness remained outside the offline matrix.

No earlier whole-matrix runtime or required-check result was reused after this rebase.
Cargo could cache unchanged dependency products; actual compiler JSON, frozen products, toolchain, flags, source inventories and environment records bind the current results.
Both current normal UI graphs contain the complete current paired cloud skills and references.
The broad pedantic tier reported only the same two Main dependency structures with too many Boolean fields.
The current enabled Worker/Protocol surfaces passed their separate pedantic check without dependency diagnostics.
No suppression was added.

## 3. Defects

Review first found that budget exhaustion could truncate outstanding reservations and that listener shutdown could race cancellation notification.
A later review found that kernel write acceptance was being treated as peer receipt: shutdown and release with unread reverse input could reset the connection and discard queued output.
The preserved pre-fix reproducer delivered only 1,152 of 8,192 response bytes before reset.
The candidate separates immediate failure closure from budget grace, drains inputs to EOF under original deadlines and a fixed three-second/64 KiB-per-direction bound, then releases sockets normally.
The decisive slow-reader cases received all 8,192 bytes and clean EOF after actual owners released.
Conservative charging, admission refusal, bounded buffers and immediate failure cancellation remain in place.

A subsequent review found that two fixtures could select ephemeral ports 47280 or 47281, which service policy refuses.
Both use the existing policy-validating backend helper and retain the same socket assertions.
This is a fixture correction; production behavior is unchanged.

## 4. Deviations from the procedure

The first current enabled container plan omitted the established host-only lease-test exclusion.
That case failed under the unprivileged container user; Protocol 57 and all Gate cases passed, and the exact container was positively removed.
The same lease case then passed on both current default and enabled frozen host ELFs.
A new recorded nonce reran the enabled Worker with the established exclusion and passed 234 tests, with four ignores.
No source, executable, assertion or budget changed for that correction; the original failed receipts remain preserved.

The unchanged standalone Core Board shutdown complement twice failed its two-second completion assertion.
Source and environment comparisons preceded further execution.
An own-child process/signal trace observed both Bash children receive SIGHUP and be reaped, and passed; instrumentation can perturb scheduling, so that diagnostic is not counted as the ordinary matrix result.
After other fixtures and compilers finished, one ordinary confirmation using the same frozen ELF, environment and assertion passed in 0.01 seconds.
Both original failures remain preserved. No deterministic cause, product fix or timer/assertion change is claimed.

The current G05 passed without an orchestration or cleanup deviation.
Earlier round-two evidence retains a prestart NetworkID assertion failure: both created containers and the network were removed, and no gate test ran.
Its corrected round-two run and the later round-three fixture-correction run also passed with positive cleanup; they are historical after this rebase.
An earlier pre-review cleanup-summary wording error remains preserved with original removal receipts and independent reconciliation.

## 5. Cleanup

All exact current offline containers and both actual-smoke containers were deleted.
The internal actual-smoke network was deleted.
Healthy fixed-daemon exhaustive inventories showed that every original owned resource ID was absent.
The fixtures used no host ports, provider resources, credentials or paid capacity.

## 6. Evidence

The metadata identifies the rebased fixture-fix commit and current enabled test ELF.
A subsequent report-only commit must preserve executable inputs and rerun Git-aware documentation/static/readiness checks.

Current private evidence is under `.task-1335-work/worker-gate-round3-fixes/rebase-main5e/`:

- `qualification-handoff.json`, `stable-all-check-input-proof.json` and `strict-parent-root-accounting.json` bind current source, commands, parent counts and deviations.
- `products/{default,speech,enabled}/` contains 84 frozen products and actual compiler manifests; `frozen-artifacts-rehash.json` records their hashes.
- `default-runtime/`, `speech-runtime/`, `enabled-runtime/` and `enabled-runtime-host-excluded/` preserve original create receipts, results and exact cleanup.
- `Core-host11/`, `Worker-lease-host/` and `Worker-enabled-lease-host/` record ordinary host complements.
- `Core-Board/`, `Core-Board-retry/`, `board-failure-source-environment/`, `Core-Board-process-trace/` and `Core-Board-post-diagnostic/` preserve the complete Board sequence.
- `G05-actual-final/` records the fresh actual peer/bytes/close run and positive exhaustive cleanup.
- `current-normal-embedding-proof.json`, `advisory-base-comparison.json`, `documentation-summary.json` and `report-nonconsumption-audit.json` record the corresponding comparisons.

Earlier evidence remains under `worker-gate-delivery-qualification/`, `worker-gate-round2-fixes/` and the parent `worker-gate-round3-fixes/` directories.
