---
procedure: native-app-automate
candidate_commit: 1f0c933e3ab2ac7deef5f3788dfbc115150e57e2
candidate_sha256: 72f9b4c0f7fdcd597290bcf2ce30530302bcb151560d66c3381675da70bdfed5
date: 2026-10-07
lanes: [ios-phone-current, ios-phone-older, ios-tablet, android-phone]
issue: https://github.com/peters/horizon/issues/1255
---

# Native app final qualification, 7 October 2026

The frozen Linux executable above completed the physical-device checks below. The app source was `21cfdf1f4a39bdcd4a82107ccd8a78a4311fb07f`; each temporary synthetic backend used source `4b80558644c8bc862a73543955611218661bafcb`. The operator approved private app/evidence uploads and paid allocations. At most two provider devices ran concurrently. No release or deployment is covered.

## Results

| Check | Measured result |
|---|---|
| Required local validation | Formatting, maintainability, 5,283 workspace tests, speech tests, blocking and strict Clippy, and packaged build passed in the task worktree with Rust 1.99. |
| Candidate CI | All 14 applicable required checks passed on the recorded implementation commit. Final PR-head review and CI remain distinct merge gates. |
| Packaged GUI | The actual child matched the frozen executable hash. Three public native Device inspections showed advancing displayed frames, with an advancing terminal heartbeat. The viewer closed and all recorded fixture processes exited. |
| Packaged MCP preflight | Initialization exposed all 13 tools. The recipe-only screenshot action returned `app_screenshot_requires_capture`; normal empty-parent EOF exited 0. |
| Closed CLI progress pipe | Exit 2 in 14.2 seconds, `app_run_cancelled`, no build-file changes, zero paid allocations and empty reconciliation. |
| Startup parent EOF | Injected after a backend writer started and before readiness. Two creations already in flight subsequently allocated and closed. Host exit 0, matching guardian closure, all four lane cleanup results confirmed, no upload cleanup errors, and empty reconciliation twice. This is cancellation proof, not a successful device matrix. |
| Native matrix | Two runs each passed 44/44 steps across the four targets below. Every lane had confirmed cleanup and each run released its uploads. |
| Live matrix presentation | The first run's four task-owned public Device panels each had three advancing inspections at least two seconds apart and multiple distinct displayed frames. Private captures were retained before closing the viewers. |
| Matrix recordings | The repeat produced four available, fully decoded recordings. Two required bounded post-run `app_video get` calls through the same live MCP host. Extracted frames were inspected and showed guest UI, menu, orientation and terminate/relaunch transitions. |
| Live parent EOF | Actual Android menu, rotation, termination and relaunch passed; eight successful backend HTTP responses, advancing displayed frames, host exit 0, retired backend/tunnel tasks and empty reconciliation. |
| Exact host crash | The same Android interactions and eight successful backend responses passed before killing only the task host. Exit -9; the first provider reconciliation retained uncertainty. One bounded exact-owner retry confirmed all four resources complete, followed by an empty reconciliation. No allocation was replayed. |
| CLI interruption | Two live iOS lanes had three advancing displayed-frame inspections before the cancellation marker. SIGINT produced exit 2 and a cancelled report, both allocated lanes cleaned up, both remaining lanes never allocated, no upload cleanup errors and empty reconciliation twice. |
| Interactive tools | All 13 tools exercised across the matrix and interactive runs. Actual Android shell/menu screenshots, advancing displayed frames, menu/rotation/terminate/relaunch, eight successful backend responses, available provider video fully decoded and visually inspected, available device and Appium logs, explicit unavailable crash/network logs, idempotent close, host exit 0 and empty reconciliation twice. |

| Matrix target | OS | Recipe steps per run |
|---|---|---|
| iPhone 15 | iOS 27 | 11/11 |
| iPhone 12 Pro | iOS 18 | 11/11 |
| iPad 10th | iOS 27 | 11/11 |
| Google Pixel 11 | Android 17.0 | 11/11 |

Each matrix lane used a distinct database namespace and temporary backend worktree. The recording repeat's four retained backend logs contained 8, 2, 2 and 2 successful app HTTP responses respectively. All four worktrees were absent after cleanup. Recording sizes were 3,040,729; 5,646,023; 6,190,377; and 715,799 bytes. Full decoding passed for every file. The complete live-display proof is from the first run; the final recordings are from the repeat on the identical executable and identical four targets. These are separately retained observations, not claims that every proof came from one allocation.

All 13 MCP tools were exercised across the matrix and interactive checks: upload, session create/close, act, wait, snapshot, screenshot, view, tunnel status, video, logs, audit and matrix run.

## Preserved failures and cleanup limits

The first matrix's raw report retains two recording fetch failures: one bounded metadata/discovery failure and one `app_media_unavailable`. Its steps and resource cleanup passed. The repeat also retains its initial recording errors; successful later reads are separate receipts and do not rewrite the report. A read-only media failure is not evidence that a device remained allocated.

An earlier CLI interruption was observed too late for live viewing; three lanes allocated before its signal. It exited 2 with confirmed cleanup and empty reconciliation after host exit. An attempted reconciliation while that host still held ownership returned `app_execution_busy`. Those receipts remain intact. The subsequent two-live-lane trial supplies the acceptance proof above.

An earlier candidate's two-second backend grace period produced one failed pre-readiness guardian acknowledgement. The original failed receipt remains false. Exact identity, process-group, namespace and worktree checks established closure through the trusted owned-journal recovery API; the original worktree and task were then retired. This required explicit repair and is not counted as a successful guardian acknowledgement. The final implementation allows ten seconds of cooperative backend cleanup and still requires the matching nonce. A real cold-start parent crash on the preceding implementation passed guardian acknowledgement without repair; that guardian code is byte-identical in this candidate. The final candidate's actual hot EOF and crash trials also retired their tasks normally.

The preceding implementation also exposed an upload-handle race during startup parent EOF: the resources were deleted, but the report contained `ArtifactUnknown` errors. Normal shutdown now waits for the active matrix lease before clearing upload handles. The repeated startup-EOF trial on the final candidate produced a clean report.

The advisory pedantic Clippy tier returned 101 for existing style diagnostics in other workspace components; it is not reported as passed. Required blocking and strict tiers passed. An initial regression-test fixture attempted admission in the wrong order; the fixture was corrected, and the full matrix was rerun successfully. Interrupted or failing local logs are retained separately.

A historical `--help` invocation dispatched startup and synchronized eight managed embedded-plugin files before refusing a missing display. No user window opened. Exact prior plugin bytes were unavailable, so no speculative rollback was attempted. Subsequent GUI smokes used an isolated temporary home and owned fixture only.

## Companion and scope boundaries

The trusted Mac completed an actual remote Xcode-build parent-closure guardian test; its exact owned process group stopped and temporary source roots were absent afterward. A subsequent unsigned Debug IPA and Android Debug APK were rebuilt. The companion helper's 31 local tests passed. This tests unattended build cleanup, not authenticated app behavior.

The recipe covers the synthetic guest shell. It does not qualify sign-in, payment, customer data or production service operation. Windows native process execution remains unsupported. Public native viewing used the existing renderer, so no changed-renderer GIF applies.

Private controller evidence remains under `/var/tmp/horizon-1313-smoke/`; validation logs remain under `/var/tmp/horizon-1313-serialized-final-validation/`. Raw configuration, provider references, app images, database names, receipts and logs are not published in this report. Completed extra preflight reports were preserved outside the bounded admission store only after exact owned cleanup was confirmed. Historical [interim preflight](2026-10-06-native-app-final-preflight.md) and [earlier matrix](2026-10-06-native-app-automate.md) reports retain their original candidate and evidence limits.

A documentation-only report commit can reuse this executable proof only after confirming that production code, dependencies and executable build inputs are unchanged from the recorded implementation commit. Required review, thread disposition and CI must still pass on the final PR head before a guarded squash merge.
