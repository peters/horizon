---
procedure: cloud-close
candidate_commit: 0443dd7a20cf2381e5c1aa2370f67499063f111b
candidate_tree: 0e71a37619912b72e892afc146338727e2ed685c
candidate_sha256: 824e94ff417c1c15d38be744b15d565e455fac3e14ede0a60ce6944de33f8387
date: 2026-10-15
lanes: [linux-native, unix-fifo-regression]
issue: https://github.com/peters/horizon/issues/1335
---

# Cloud close background checks on current main

## 1. Summary

Native C01–C04 and C07 passed on the candidate above, based on main `4c021392`. C05 was not observed natively. C06 used automated real FIFO, cancellation, connection-watch and lock tests. The isolated fixture completed normal exit and exact cleanup. This report can accompany a later report-only commit when all 1,774 compiler inputs still match.

## 2. Results

| Task | Result | Observation |
|---|---|---|
| C01 | Native pass | Initial close offered Delete and Cancel. Cancel and Escape each retained the cloud. |
| C02 | Native pass | Failed Delete showed worker/storage uncertainty. Cancel retained the cloud; reopen reset the override. Retry and explicit Anyway removed only its card. |
| C03 | Native pass | Never-started close offered ordinary Remove. Cancel retained the card; Remove succeeded. No deployment appeared and its operation lock remained. |
| C04 | Native pass | Failed Delete showed storage-only uncertainty without inventing a worker. Cancel retained the card. After C07, explicit Anyway removed it while its saved record and marker remained. |
| C05 | Native unobserved | Missing and unreadable records have automated coverage; no native C05 result is claimed. |
| C06 | Automated pass | Real FIFO regressions cover blocked reads, cancellation, reset, stale results and worker lock custody. Refresh keeps settled controls; final removal still checks the current record. |
| C07 | Native pass | The actual app window became approximately 1254×832 pixels on a 1600×1000 desktop. Fit ran while small. The last cloud's Close modal had readable Delete/Cancel controls. Cancel, restored window and Fit passed before its removal. |

Retained deployment and marker hashes matched the immutable deterministic fixture seed and its recorded baseline. The exact seed bytes and guest paths matched. No fresh pre-action hash snapshot was captured in this run. The final UI showed all three synthetic cloud cards removed. The ordinary observer occupied a separate workspace; its scoped MCP results did not prove a global three-to-zero listing transition.

## 3. Local checks

A fresh standard workspace run passed, including UI 2246 tests with two ignored, Core OFF 2081 with 21 ignored, SDK 471 with one ignored and worker 210 with three ignored. The fresh speech run passed UI 2292 with two ignored, font integration three and isolated-home integration two. Full blocking and strict Clippy, format, maintainability, skill coverage and diff checks passed.

Advisory Clippy exited 101 for the two unchanged `struct_excessive_bools` findings in cloud provider and browser WebDriver state. The complete workspace coverage record retains the other crate results. No old whole-matrix result was relabelled.

The normal UI and cleanup-OFF owner CLI were newly built. Device, browser and private synthetic fixture helper were reused only after a current source/configuration/dependency/toolchain/feature-closure comparison. Both the actual app and its ordinary MCP child matched the candidate UI hash above.

## 4. Native evidence and deviations

The public Device viewer presented real live frames before input; three observations advanced 15→31→35. Input used fresh geometry on the exact isolated target. The fixture had NETWORK NONE, synthetic records and no provider settings or host credential mounts. Missing settings intentionally made Delete fail locally.

Three public recordings finalized at 97.693 seconds/812 frames, 165.560 seconds/1395 frames and 151.069 seconds/1293 frames. All had `encoder_failed=false` and no file-limit stop. Each recording was copied before viewer closure. The inspected GIF uses chronological excerpts from all three: 3× speed, 6 fps, 1000-pixel width, 45.840 seconds, 275 frames and 6,388,981 bytes. Its SHA-256 is `c9bda41c617d199f8777128922c133557dd78e49e2bae03c42fbd14cc8215e60`. Two-pass palette encoding and decoded contact, small-modal and final-frame inspection passed. All content is synthetic.

An initial viewer endpoint transcription was corrected before input. Some early clicks and window-size attempts did not achieve their intended result; the successful window-manager Resize sequence and subsequent small-modal check were recorded. These observations establish no product cause. Earlier source35,795f and review-fixed486 evidence stays historical on its original inputs.

## 5. Cleanup

The app closed normally with exit 0. The private service was empty; owned children and collectors stopped; the target expired. All original videos were copied, the exact owned viewer closed and the explicit relay stop completed with original-session exit 0. Qualified cleanup and a separate check against the same healthy daemon confirmed the exact fixture container was absent. No real provider or tailnet resource was created. Source, products, logs, recordings and record evidence remain preserved.

## 6. Publication limits

This UI prerequisite adds no Backend cleanup activation or new CLI/MCP delete authority. Final public-head CI, independent review and merge permission remain separate gates. The current GIF and exact product/input manifests accompany the private qualification evidence; the PR attachment provides the public demonstration.
