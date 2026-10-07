---
procedure: native-app-automate
candidate_commit: bc7819d21949faf3203af90d9a17ab5154b9c694
candidate_sha256: 74a83f58514793644af2d0f24f9b615f44b9bedcb8d9de929ed3040dc6b942a3
matrix_candidate_commit: 17f36d9ba8e587951078afb396a2309f7dac4710
matrix_candidate_sha256: 3c330c3172a1fc8121776771841b86e74fb52558131604faca85d54e780a4720
date: 2026-10-06
lanes: [ios-phone-current, ios-phone-older, ios-tablet, android-phone]
issue: https://github.com/peters/horizon/issues/1255
---

# Native app test report, 6 October 2026

## 1. Summary

This report records the earlier candidates named in its metadata and Section 7.
It does not qualify later changes to the native host.
Section 7 records the limits of the last live-view check.
The controller reboot removed the temporary raw evidence directory after these runs.
The committed results remain historical records, not fresh device evidence.

The packaged MCP run passed all 44 recipe steps on four physical devices.
The run used two concurrent lanes and four separate synthetic backends.
Every device showed advancing frames, received successful backend responses and confirmed cleanup.
All four retained videos decoded without errors.

The final candidate passed CLI cancellation, normal MCP parent closure and typed setup-error checks.
Exact crash recovery completed the same four recorded operations on a bounded second attempt.
Local source review and all blocking validation passed.
Public review and CI remained separate merge gates.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| NATIVE-MATRIX | pass | Four devices, 44/44 steps, two concurrent lanes and four separate namespaces. | — |
| NATIVE-MATRIX: network | pass | Each app reached its assigned loopback backend. Successful request counts were 9, 3, 3 and 3. | — |
| NATIVE-MATRIX: live view | pass | Every public Device panel displayed advancing frames in four timestamped inspections. | — |
| NATIVE-MATRIX: evidence | pass | All four videos decoded. Each requested recipe screenshot had a successful evidence result. | — |
| NATIVE-CANCEL | pass | SIGINT produced exit code 2, a cancelled report, confirmed cleanup and no upload cleanup errors. | — |
| NATIVE-EOF | pass | Normal parent EOF produced exit code 0 and two complete guardian receipts. Exact recovery found no pending operations. | — |
| NATIVE-CRASH | pass | SIGKILL stopped both local services. The second exact recovery attempt completed the session, tunnel, run and upload. | — |
| NATIVE-REMOTE-BUILD | pass | Parent SIGKILL stopped a real Xcode build group and removed its unique remote source directory. | — |
| CLI setup errors | pass | The packaged archive-limit check returned a typed NDJSON error before allocation. The missing-client regression exposed no private path. | — |
| Local validation | pass | Formatting, maintainability, workspace tests, speech tests, blocking Clippy and strict Clippy passed. | — |
| Cleanup test portability | pass | Provider tests passed after a test-only correction. A Mac probe distinguished an exact zombie PID, a live PID and a reaped PID. | — |

The matrix used these provider-resolved targets:

| Device | OS | Steps |
|---|---|---|
| iPhone 15 | iOS 27 | 11/11 |
| iPhone 12 Pro | iOS 18 | 11/11 |
| iPad 10th generation | iOS 27 | 11/11 |
| Google Pixel 11 | Android 17.0 | 11/11 |

## 3. Defects

No in-scope code blocker remained after independent local review.
The advisory pedantic tier returned 101 in unchanged cloud code.
This report does not mark that advisory tier as passed.

## 4. Deviations from the procedure

The full matrix and SIGKILL checks used the matrix candidate named above.
The later source change added typed CLI setup-error output only.
Successful MCP execution and resource ownership did not change.
The final candidate repeated CLI cancellation and normal MCP EOF checks.
The final review then corrected platform-specific observation in a cleanup regression test.
That test-only change passed provider tests and the applicable lint tiers; packaged runtime behavior did not change.

The first crash recovery attempt returned `app_reconciliation_required` and retained the exact session as uncertain.
The second attempt completed the same operations without creation replay.

An additional isolated desktop check tested packaged GUI startup.
Three public inspections showed displayed frames and an advancing terminal heartbeat.
A private video contained 12 direct captures across 13.11 seconds and decoded successfully.
The task closed its exact candidate window normally and expired its target.
This feature did not change the GUI renderer.

## 5. Cleanup

All owned device, upload and local-service cleanup results were positive.
The run closed its four public Device panels.
The selected remote build tests confirmed their exact process groups stopped before directory removal.
Shared development services and unrelated desktops remained intact.

One completed prior cancellation archive moved to a separate private evidence directory.
File hashes matched before and after the move.
A private export receipt preserved the original and destination paths.
The journal, registry, owner and state roots did not move.

## 6. Evidence

Task-private evidence lives under `/tmp/issue-1255-preflight/` on the controller.
It includes candidate hashes, per-step reports, public panel inspections, videos, backend requests and cleanup receipts.
App-specific content and companion source commits remain private.
The source report contains no credentials, private endpoints or customer data.


## 7. Final source check and matrix repeat

The final runtime package used source `8e0ff3a1adadf6b52eadd7a4442452b617b89aa3`.
Its SHA-256 was `8dc2b8ff53da4264c4e0d5f4138a5b3742cafff2815464bd0d336491e713b072`.
The frozen build source matched the independently reviewed file hashes.
All blocking local validation passed. The unchanged cloud pedantic warning remained advisory.

The final matrix again passed 44 steps on four physical devices with two concurrent lanes.
Each device used its own synthetic loopback backend.
The four logs recorded successful app requests. All four videos fully decoded.
Each step retained its screenshot. All 136 owned journal records were complete.
The task closed its four public Device panels.

The unchanged viewing path passed all four panels on earlier package `c0e63237b2e24295f1126f5475d3e999397a0254`.
Three panels each passed three displayed-frame inspections during the final run.
Panel 0 displayed frame 1 before viewport navigation clipped and moved it away.
Connected frames advanced through 3, 5 and 7. The task preserved navigation.
This report does not claim a fresh four-panel display pass.
Independent review accepted the prior proof for unchanged viewing and provider code.

New tests rejected oversized evidence requests before resource operations.
They filled the file limit and simulated a full byte budget.
Both tests then saved the separately reserved terminal report.
They also refused oversized reports without consuming the report slot.
The documented limits are 1,024 evidence files, 120 MiB of evidence and an 8 MiB report reserve.
