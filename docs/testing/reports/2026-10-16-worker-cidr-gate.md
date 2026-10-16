---
procedure: worker-exposure-gate
candidate_commit: bafeb16aeaf1f080b0ecdfd66013b90fc5c9b134
candidate_sha256: f3ff90c2997a7b728687722e2b5d846e73939d2ce027136cc24ad00252899ce1
date: 2026-10-16
lanes: [linux-offline, linux-root-cidr-component]
issue: https://github.com/peters/horizon/issues/1335
---

# Worker CIDR gate test report, 2026-10-16

## 1. Summary

The run passed G01–G05 for the default-off Linux component.
The actual root worker used the original accepted peer for CIDR admission.
The separate client completed PING, PONG and DONE on one connection.
The run did not qualify provider source preservation, named/tag identity, TailnetOnly, public interfaces or complete M2.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| G01 | pass | The enabled protocol passed 49 tests and one documentation test. | — |
| G02 | pass | The enabled gate passed 13 socket, policy and resource-bound tests. | — |
| G03 | pass | The close and drop tests ended the original flows and stopped admission. | — |
| G04 | pass | The default workspace passed 7,003 tests; 55 tests stayed ignored. Speech passed 2,411 UI and five integration tests; three stayed ignored. Required lint, documentation and static checks passed. | — |
| G05 | pass | The actual root constructor refused the wrong source. The separate admitted client exchanged bytes, reached EOF and observed refusal after close. | — |

The pedantic tier reported two unchanged Main structures with too many Boolean fields.
The required blocking and strict tiers passed.
The enabled component passed a separate pedantic check without dependency diagnostics.
The default unit lane kept the explicit G05 case ignored; the separate root lane ran that exact case.
One custom live WebAuthn harness was outside the offline matrix.

## 3. Defects

No product defect was found in this component run.

## 4. Deviations from the procedure

The first cleanup summary rejected Docker's network-not-found wording.
The original removal receipts and a separate read-only inspection proved cleanup.
The raw summary stayed in the private evidence.
No product source changed, and the run did not repeat.

## 5. Cleanup

Both exact test containers and their internal network were deleted.
Separate inspections and complete inventories showed that all three resources were absent.
The original Docker daemon remained healthy.
The fixture used no host ports, provider resources, credentials or paid capacity.

## 6. Evidence

The frozen candidate hash is in the report metadata.
The feature commit contained the same Rust, dependency and embedded-asset inputs that the run used.
The later report and index update changed prose only.

Private evidence stayed under `.task-1335-work/worker-gate-delivery-qualification/`:

- `full-matrix-handoff.json` and `strict-parent-root-accounting.json` recorded the exact matrix coverage.
- `G05-actual-final/command-effect-ledger.json` recorded the original commands and resource receipts.
- `G05-actual-final/result.json` retained the raw cleanup-summary error.
- `G05-actual-final/reconciled-result.json` recorded the successful result and positive cleanup proof.
- `G05-actual-final/readonly-cleanup-reconciliation.json` recorded the independent final inspections.
- `prose-result-reuse-proof.json` recorded the comparison of unchanged executable inputs.
