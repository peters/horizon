---
procedure: terminal-close
date: 2026-10-08
base: 9699b65d9c8af76ef614b43a5b109ee68cd0879f
candidate_runtime_patch_sha256: 2ebc0ba594a4532814a4e5a9a4fd0d8b04ee6a1785b0cf25098c0bd417b14853
candidate_source_file_sha256: 628720e632fe1c3bd8664d0a1b3a698f6e5338e674295b182d5e4bd69b7773bc
candidate_sha256: 3eb26bf211aff6f2ff2a1700fc352ec313229741de57752d2a5a955ab4219d36
lanes: [linux]
---

# Terminal destruction after event loop exit

## 1. Summary

All tasks in [the terminal close procedure](../procedures/terminal-close.md) passed on Linux.
The candidate used the recorded base revision and the corrected `terminal/lifecycle.rs` file with the hash above.
The focused fixture failed with the old implementation and passed with the correction.
The native smoke used the frozen executable above.
The runtime patch hash covers the lifecycle implementation and its new unit fixture.

The Linux speech shard stopped after the Tab focus test exceeded its time limit.
[CI run 37811802758](https://github.com/peters/horizon/actions/runs/37811802758) captured thread stacks with argument and entry values disabled.
The test thread reached the end of the test and destroyed its terminal harness.
Its stack showed this sequence:

1. `Terminal::drop` dropped the event loop's `JoinHandle`.
2. The finished thread's packet destroyed its returned `EventLoop` and state.
3. `Pty::drop` sent SIGHUP, then called `Child::wait`.
4. `wait4` blocked the test thread while the PTY child stayed alive.

The test assertions completed before this sequence.
Terminal destruction now uses the existing helper thread to join the event loop and destroy its returned PTY.
The caller returns while cleanup waits for the child.
The change adds no new signal policy or child exit deadline.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| TERM-CLOSE-UNIT | Pass | The old implementation failed in 2.01 seconds. The correction passed in 0.02 seconds. | None |
| TERM-CLOSE-LIVE | Pass | Normal panel close left the gated child alive. The counter advanced and Fit responded. | None |
| TERM-CLOSE-CLEANUP | Pass | Release reaped the exact child. The candidate stayed alive. Resize, Fit and final fixture cleanup passed. | None |

Three public inspections, more than two seconds apart, reported displayed pixels and advancing frames in one connection.
Received frame counts were 946, 954 and 960. Uploaded frame counts were 826, 834 and 840.
The recording completed with 862 frames and no encoder failure.
The retained GIF contains panel close, continued counter output, resize and Fit.

## 3. Defects

No candidate defect remained in these tasks.
The old implementation waited for the live child during terminal destruction.
The fixture released that child before it reported the expected old-code failure.

## 4. Deviations and limits

The Unix shell and SIGHUP fixture does not run on Windows.
The native smoke used one isolated Linux desktop with synthetic output and no credentials.
The native smoke did not force the completed-handle condition; the deterministic unit fixture tested that condition.
This report covers the focused regression and native smoke.
The pull request records the complete validation matrix separately.

## 5. Cleanup

The test gate released the owned child, and its process disappeared.
Normal window close stopped the candidate and every recorded fixture process.
The launcher revoked the native target and removed the private configuration and home.
The owned Device viewer closed after the finalized recording was copied.
No unrelated process or viewer changed.

## 6. Evidence

Private evidence contains the old-code failure, corrected fixture result, timestamped viewer inspections and executable identity checks.
It also contains screenshots, the decoded recording, the GIF and exact owned process cleanup records.
Public evidence contains no local paths, process IDs, host names or credentials.
