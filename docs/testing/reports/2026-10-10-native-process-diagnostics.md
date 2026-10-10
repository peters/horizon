---
procedure: native-process-diagnostics.md
candidate_commit: 3a6af043bb987f26ce594ee162a78af57e55f203
candidate_sha256: aa32e25a86ba78c9ea6c3faa8e5ed7e6e3dbf481b0312869c3475eff5496bf4a
candidate_kind: source-patch
candidate_base: dc3152532038608d983858758ec83eef14a383d0
date: 2026-10-10
lanes: [linux-unit, linux-guardian]
issue: https://github.com/peters/horizon/issues/1373
---

# Native process failure log test report, 2026-10-10

## 1. Summary

All 26 unit tests and 21 actual guardian integration tests passed on the recorded source.
The actual private backend logs retained the finite host cause after closure and startup journal failure.
The concurrent output test kept stdout, stderr and the host cause within the shared 4 MiB limit.
This report tested the native-process prerequisite; it did not qualify host adapter integration or paid devices.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| PROCESS-LOG-01 | pass | All 47 process tests passed with warnings denied. The required Clippy tiers and the native-process advisory check passed. | — |
| PROCESS-LOG-02 | pass | The normal close log contained `app_host_unavailable: Guardian: I/O BrokenPipe` once in 279 bytes. | — |
| PROCESS-LOG-02 | pass | The startup failure log contained `app_host_unavailable: Guardian: MissingState` once in 282 bytes. The declared child did not start. | — |
| PROCESS-LOG-02 | pass | The concurrent log retained 4,193,438 bytes, including the exact first cause once. Its private marker matched the cause record. | — |
| PROCESS-LOG-02 | pass | All three guardian receipts recorded complete cleanup. All copied evidence files had private permissions. | — |
| PROCESS-LOG-03 | pass | Cloned and independent handles preserved one first cause. Full child output could not use the 1 KiB host reserve. | — |
| PROCESS-LOG-03 | pass | Replaced roots, state, logs, symlinks, hard links and changed receipt identities could not acknowledge retention. | — |
| PROCESS-LOG-03 | pass | Partial markers did not permit truncation or acknowledgement. Failed fsync calls required new successful durability checks. | — |
| PROCESS-LOG-03 | pass | Finite archive and lifecycle causes kept their exact messages. Unknown error text was refused. | — |
| PROCESS-LOG-03 | pass | The guardian used the trusted temporary directory. Child temporary variables used its separate owned task directory. | — |

The preceding source qualification used commit `5c7e8bdd1e811d607d39bf8f9cfcd9987d79f31c` and source-patch hash `5fd3ff30eb501f2fa5350c54d08c8572cb50e08fc6db839404684172092e3c53`.
Its focused run passed 25 unit tests and 21 guardian integration tests.
All required local validation commands passed in that candidate worktree.
That workspace run passed 5,720 unfiltered tests and four subprocess fixture tests, with 47 existing ignores.
That speech run passed 1,846 tests. Format, maintainability, skill coverage, the default build and both required Clippy tiers passed.
The preceding and current native-process advisory Clippy checks passed with no dependencies.
The workspace advisory check stopped on six unchanged warnings in other packages.
Their source files were identical to the recorded base.

## 3. Defects

No in-scope defect remained after the fixture and lint corrections.
The workspace advisory check reported four existing empty-value assertions and two existing excessive-boolean warnings.
The native-process package had no advisory warning.

Guardian termination records use the ordinary log budget.
They do not claim the host's first cause or its reserved space.
The host adapter must capture its held log before expiry or cleanup.
That adapter integration remained a separate change.

## 4. Deviations from the procedure

The first concurrency fixture contained control characters in its command argument.
The existing declared-command guard refused that fixture before child execution.
The corrected fixture used the same flood, flush, cleanup and size assertions in one command line.
The focused run passed without a production guard change.

The first macOS CI run refused non-canonical test directories before logging tests started.
The corrected fixture used the canonical trusted temporary parent.
A new regression kept the production refusal of an aliased state directory.
The current Linux focused run passed all 47 tests.
The corrected source had not yet run in macOS CI when this report was recorded.

The recorded candidate hash identified the frozen source patch, including its guidance changes.
It did not identify a released binary.
Additional full-workspace, speech and Clippy checks extended the procedure's focused library test.

## 5. Cleanup

All actual guardian fixtures closed their owned children and recorded complete cleanup.
The fixtures removed their temporary directories.
The private copies of synthetic evidence remained for review.
No paid device, real provider credential or live backend was used.

## 6. Evidence

Private evidence contained the source hashes, actual logs, exact cause markers and terminal receipts.
The reviewed summary excluded process IDs, local paths, operation identities and unrelated content.
