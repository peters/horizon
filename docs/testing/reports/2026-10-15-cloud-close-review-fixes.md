---
procedure: cloud-close
candidate_commit: 120ea76dd65b64f5ad4edb2b50957fc881c6c69c
candidate_tree: 48624f64a6137e51a1f8501006a552b34207d6b2
candidate_sha256: 38f893b8bd993c6afc7201e4c2149f87449954caf94dcbc30d22f5e475c8302e
date: 2026-10-15
lanes: [isolated-linux-native-vnc, unix-fifo-regression]
issue: https://github.com/peters/horizon/issues/1335
---

# Cloud close review-fix native report, 2026-10-15

## 1. Summary

The review-fixed native candidate passed C01–C04 and C07. The running app and normal browser-MCP child both matched frozen UI SHA256 `38f893b8bd993c6afc7201e4c2149f87449954caf94dcbc30d22f5e475c8302e`. The candidate had base HEAD `120ea76` and staged runtime tree `48624f64`; its full build/input identity remained in the frozen five-product manifest.

The run used the declared public three-cloud fixture in an isolated NETWORK NONE Linux/X11 container. Missing provider settings were intentional. No provider request, paid resource, real enrollment, host credential or user desktop input was used. The existing synthetic Secret Service helper supplied fixture state only.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| C01 | pass | Initial Delete/Cancel dialog had no override. Cancel and reopen/Escape retained the cloud/panel. | — |
| C02 | pass | Failed Delete named the synthetic worker and storage. Cancel/reopen reset the override. Retry failure followed by explicit override removed demo-api. | — |
| C03 | pass | Ordinary Remove removed the never-started cloud/panel. deployment.json stayed absent and operation.lock existed. | — |
| C04 | pass | Failed image-push Delete warned about storage without inventing a worker name. Cancel retained it; later explicit override removed it. | — |
| C06 | pass | Automated real FIFO tests checked cancellation, stale results and lock custody. The close tests checked Ready watches, failed preflight, busy operations and held record locks. | — |
| C07 | pass | The actual window shrank to 1252×827. Fit ran while small; the Close dialog and Delete/Cancel controls were readable. Cancel retained image-push. Restore/Fit preceded its final removal. | — |

Before cleanup, both original deployment files and workspace-volume.required matched their pre-run SHA256 hashes. The original resource records stayed on disk after explicit overrides. `record-preservation-proof.json` retained these checks.

C05 Native remained UNOBSERVED; its unit/static coverage was separate. C06 consisted of actual automated FIFO, cancellation, watch, stale-owner and lock-custody regressions. This native run did not claim a blocked-filesystem or live Ready-watch reproduction. The review-fix regressions exercised cancelled AfterDelete custody and Ready watch failure/override through actual methods. The cancellation test completed both Cancel and Escape. The watch tests completed missing and unreadable settings, busy-operation and held-lock cases.

The four new regressions, 44 close tests and three worker tests passed. The full default UI suite passed 2,236 tests, with two ignored. The speech suite passed 2,282 tests, with two ignored. The font and home checks, blocking and strict Clippy, format, maintainability and skill checks passed.

The unchanged library and independent workspace results were reused after source, dependency, feature, toolchain and runtime-input comparison. Advisory Clippy returned exit 101 for two unchanged struct-layout findings. The first speech attempt stopped at the cache guard. The complete retry passed. No stopped attempt was counted as passing.

## 3. Defects

No additional defect was observed in the declared native flows. The two first-review findings had separate RED/GREEN regression evidence. Native success did not replace that automated coverage.

## 4. Deviations from the procedure

The separate ordinary AGENT observer used its own fourth workspace. Its public MCP results proved normal transport, not a global three-to-zero cloud-list transition. The public viewer displayed changing live frames before inputs, with sequences 24→28→34. The app PID and normal browser-MCP child each matched the frozen executable.

Every native action used a fresh screenshot geometry. Click was the default button action, and Escape used the declared `escape` spelling. No input was refused during this final run.

The source795f/UI11d3 run remained immutable first-reviewable-head evidence, including its full-flow GIF and complete cleanup. The older source35 run remained historical and had only partial C07 coverage. Neither was relabelled as the review-fixed run.

## 5. Cleanup

The app closed normally and returned exit 0. The owned service/catalog were empty. Children and output collectors stopped, and the target expired. Both videos finalized and were copied before the owned public viewer closed.

The explicit relay stop completed, and its original session exited 0. Qualified cleanup removed the exact owned container. Independent inspection against the same healthy daemon confirmed absence. No owned active service, viewer, relay or container remained. `cleanup-proof.json` retained separate native, relay, viewer and container results.

## 6. Evidence

Private evidence was kept under `cloud-removal-async-review1-final-main1f3d-48624-62c1`. It included live public inspections, actual executable hashes, device receipts/screenshots, original-record comparisons, finalized videos and cleanup proofs. The five-product freeze and qualified reuse of unchanged CLI/device/browser/helper artifacts remained under `review1/final-qualification`.

The public recordings finalized at 56.276 seconds/465 frames/3,816,427 bytes and 107.442 seconds/885 frames/6,353,744 bytes. Both reported encoder_failed=false and no file-limit stop.

The final GIF concatenated both complete finalized recordings in chronological order, then used 4× speed, 6 fps and 1000-pixel width. Separate palette-generation and palette-use passes produced 245 decoded frames over 40.84 seconds. The contact sheet, explicit small-modal frame and final removed-card frame were inspected. It covered Cancel/Escape, failure/reset/retry/override, ordinary Remove, storage warning/Cancel, small Close controls, restore/Fit and final override.

GIF: `cloud-close-fullflow.gif`, 5,004,654 bytes, SHA256 `f2f83ed90cabbdc870d1875c1470f07d1c59bae65cf92886976cd74014c4f089`. `gif-manifest.json` retained both original source hashes and exact whole-recording bounds. GitHub review and CI remained separate gates. The GIF hash identified the recording for this source.
