---
procedure: native-process-diagnostics.md
candidate_base: dc3152532038608d983858758ec83eef14a383d0
candidate_patch_sha256: 5fd3ff30eb501f2fa5350c54d08c8572cb50e08fc6db839404684172092e3c53
date: 2026-10-10
lanes: [linux-unit, linux-guardian]
issue: https://github.com/peters/horizon/issues/1373
---

# Native process failure log test report, 2026-10-10

## 1. Summary

All 25 unit tests and 21 actual guardian integration tests passed on the recorded source.
The actual private backend logs retained the finite host cause after closure and after startup journal failure.
The concurrent output test kept stdout, stderr and the host cause within the shared 4 MiB limit.
This report tests the native-process prerequisite. It does not claim host adapter integration or paid device qualification.

## 2. Results

| Task ID | Result | Note |
|---|---|---|
| PROCESS-LOG-01 | pass | All 46 process tests passed. The standard candidate also built with warnings denied. |
| PROCESS-LOG-02 | pass | The normal close log contained `app_host_unavailable: Guardian: I/O BrokenPipe` once in 279 bytes. |
| PROCESS-LOG-02 | pass | The startup failure log contained `app_host_unavailable: Guardian: MissingState` once in 282 bytes. The declared child did not start. |
| PROCESS-LOG-02 | pass | The concurrent log retained 4,193,438 bytes, including the exact first cause once. Its private marker matched the cause record. |
| PROCESS-LOG-02 | pass | All three guardian receipts recorded complete cleanup. All copied evidence files had private permissions. |
| PROCESS-LOG-03 | pass | Cloned and independent handles preserved one first cause. Full child output could not use the 1 KiB host reserve. |
| PROCESS-LOG-03 | pass | Replaced roots, state, logs, symlinks, hard links and changed receipts could not claim retained diagnostics. |
| PROCESS-LOG-03 | pass | Partial markers did not permit truncation or acknowledgement. A failed fsync required new successful durability checks. |
| PROCESS-LOG-03 | pass | Finite archive and lifecycle causes kept their exact messages. Unknown error text was refused. |
| PROCESS-LOG-03 | pass | The guardian used the trusted temporary directory. Child temporary variables used its separate owned task directory. |

All required local validation commands passed in the final candidate worktree.
The workspace run passed 5,720 unfiltered tests and four subprocess fixture tests, with 47 existing ignores.
The speech run passed 1,846 tests. Format, maintainability, skill coverage, the default build and both required Clippy tiers passed.
The native-process advisory Clippy check passed with no dependencies.
The workspace advisory check stopped on six unchanged warnings in other packages.
Their source files were identical to the recorded base.

## 3. Defects and limits

The first concurrency fixture contained control characters in its command argument.
The existing declared-command guard refused that fixture before child execution.
The corrected fixture used the same flood, flush, cleanup and size assertions in one command line.
The final focused run passed without a production guard change.

Guardian termination records use the ordinary log budget.
They do not claim the host's first cause or its reserved space.
The host adapter must capture its held log before expiry or cleanup.
That adapter integration remains a separate change.

## 4. Cleanup

All actual guardian fixtures closed their owned children and recorded complete cleanup.
The fixtures removed their temporary directories.
The private copies of synthetic evidence remained for review.
No paid device, real provider credential or live backend was used.

## 5. Evidence

Private evidence contains the source hashes, actual logs, exact cause markers and terminal receipts.
The reviewed summary excludes process IDs, local paths, operation identities and unrelated content.
