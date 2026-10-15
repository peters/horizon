---
procedure: tailnet-api-removal
candidate_base: 5d44a7416ed5caa54f3fa6a81803c9e3a0afeb8e
candidate_head_at_build: 8636873150f05c71b69f194358fd50c754fea5ec
candidate_runtime_tree: abb5291da54d306b7e6c344917540e3234b6b8a1
candidate_sha256: f863ad5b20a35da82b1e7cb41ab7f316f0431d37f1fe923906c1ff060090d21b
qualification_scope: review2-native-and-current-local-matrix-pass
date: 2026-10-15
lanes: [linux-isolated-synthetic]
issue: https://github.com/peters/horizon/issues/1335
---

# Tailnet API credential removal test report, 2026-10-15

## 1. Summary

The fresh round-two native run passed the Connections gate and R01–R03; presentation exercised resize and Fit on the runtime tree above.
Native approval removed only the selected API binding; actual CLI plans and ordinary AGENT public MCP agreed.
All three saved networks and enrollment-key bindings and contents survived until deliberate fixture teardown.
No provider allocation, Tailscale API request or host credential change occurred.
The applicable required local matrix passed through new affected checks and valid reused package results.
The fixed-head GitHub checks and independent Copilot review are separate final gates.

## 2. Executable identity

The build records published HEAD `8636873150f05c71b69f194358fd50c754fea5ec` plus staged review fixes.
It does not claim a clean committed build of the later fixed head.
The 1,731-input inventory SHA256 is `6f72228b992d5f86c49ea618707a74cd1acdafcd0e96833ba3d189c14340a042`.
Two UI Rust inputs and two embedded cloud references changed from the prior checked runtime.

| Product | Qualification | SHA256 |
|---|---|---|
| Horizon | Newly built and frozen | `f863ad5b20a35da82b1e7cb41ab7f316f0431d37f1fe923906c1ff060090d21b` |
| cloud_deploy | Original build reused; 826 matching closure inputs | `e921aa16a1370ef65260d7eb89d07c56f7e030f5966ee08b619b37fbc1977787` |
| horizon-device | Original build reused; 39 matching closure inputs | `fe2adf0a1b710ba11d680b34bddefc244decc956131d1ef06024de81a9af68af` |
| horizon-browser | Original build reused; 264 matching closure inputs | `3be55702cdb1c2be94c282d1cf6b5d5690b29a37e72d23f49acc0c3d3253104d` |

The reused private native helper SHA256 is `e52c3a60cc9daab63b836057891c67e78ab39fcbcf8e76738dee0624644191fc`.
Its 114 public SDK inputs and three reviewed cfg(test) fixture leaves match; dependencies, flags and features are unchanged.
The helper retains its original build identity. Its private setup/cleanup is separate from public MCP proof.
Root verified all current input hashes, schema and executable hashes before launch.

## 3. Fresh native results

| Task | Result | Actual observation |
|---|---|---|
| Connections gate | pass | The first provider-free synthetic card's picker was disabled while Settings owned tailnet state. Closing Settings enabled an ordinary selection of A; reopening disabled a subsequent change to B and retained the selection. |
| R01 | pass | Native approval of B removal waited on an owned 45-second Store lock. Close/reopen retained pending work; actual owner MCP reported running before release and applied afterward. Other-caller CLI/MCP status and cancel were refused. |
| R02 | pass | Original-owner Cancel and a fresh native Decline preserved stored OAuth C. Five malformed/unsupported cases each refused through actual CLI and MCP. |
| R03 | pass | Direct native C removal waited on a separate 45-second lock. Settings closed, the private service stopped, and Settings reopened before release. Pending became a retained write failure. |
| R03 recovery | pass | Restoring the service and Refresh reported missing while retaining the write error. Another close/reopen retained it; a fresh request/native approval then produced applied and cleared its own failure. |
| R04 presentation | observed | The actual window shrank to 1,220 × 780, restored to 1,480 × 900 and used workspace Fit after restoration. The smaller-window Fit step was not repeated in this run. Public Device frames showed the controls. |

The malformed cases were `set_personal_token`, `set_oauth`, `token`, `root` and `credential_reference`.
These operations/extra fields were refused; no secret import or Set route was enabled.
A receipt file named b-applied-mcp actually held running; the later actual applied receipt supplies completion proof.
All three credential observations were unavailable during the unfinished journal/service outage.
Read recovery reported all three missing. Missing did not prove the original failed write succeeded or rolled back.
The fresh retry had a distinct request ID and its own applied receipt.

| Exact CLI/MCP catalog/map pair | Observation skew, seconds |
|---|---:|
| B complete, Settings open | 0.414319 |
| B complete, Settings closed | 1.388450 |
| Outage, all unavailable | 1.075677 |
| Recovery, missing with write error retained | 1.900383 |
| Fresh completion, Settings open | 0.311429 |
| Final Settings closed | 0.583475 |

Skews compare CLI file modification time with actual MCP observer time; they do not prove atomicity or latency guarantees.
Every observed catalog retained the same three IDs and names.
Private-bus snapshots preserved all three Auth slots, content hashes, service/usernames and the catalog before, after recovery and finally.
The exact synthetic expected-value comparisons also passed; no value was exposed through public operations.

## 4. Local checks and reuse

Fresh checks passed: the new Connections regression; default UI 2,225 tests with two ignored; fonts three; home isolation two.
Actual workspace-feature proofs matched 494/495/495 compiler artifacts without an unknown or mismatch.
Fresh speech UI passed 2,271 tests with two ignored, plus three font and two home-isolation tests.
Fresh full blocking/strict Clippy, formatting, maintainability, skill coverage and both diff checks passed.
The other 23 non-UI package closures retain passing results through source, inverse dependency, feature and input comparison.
Together these cover the required local matrix; this does not claim a new wholesale workspace run.
The UI was rebuilt; the three unchanged CLI artifacts and helper were validly reused, without relabeling their original builds.
Recorded settings include rustc1.98.1, locked/offline dependencies, jobs2, incremental0, line-tables-only debug and RUSTFLAGS=-D warnings.
Advisory pedantic Clippy exited101 on inherited excessive-bool findings in Description and GroupState.
The existing unknown clippy::assert_is_empty lint warning is recorded separately; no advisory pass is claimed.
Earlier feature-disabled compatibility results remain historical and were not relabeled for changed embedded guides.

## 5. Cleanup and recordings

The candidate exited normally with code zero; the owned service was empty, children/collectors stopped and the target expired.
The public viewer closed, the relay exited zero, and exact container absence was confirmed against the same healthy daemon.
All three recordings finalized with encoder_failed false and file_limit_reached false.

| Recording | Duration, seconds | Encoded / dropped / repeated frames | Bytes |
|---|---:|---|---:|
| Connections | 152.697 | 1,255 / 272 / 1,001 | 11,210,646 |
| Remove | 95.654 | 778 / 179 / 466 | 8,530,115 |
| Failure/recovery | 215.819 | 1,773 / 386 / 1,160 | 22,737,574 |

Two GIFs use real finalized recordings at 6 fps and 1,000-pixel width.
Connections seconds 45–125 run at 3.2× for 25 seconds: 1,578,054 bytes, SHA256 `96062d06f98ad3c946ac4670103d46b02b1e9a627af7c54230f1ecae4337ab53`.
Removal seconds 0–95 run at 5× for 19 seconds: 1,671,788 bytes, SHA256 `07ba7c57417ba035c83d075977db8d6daab39b4c8362a7040cda17d78e2d37b8`.
Source frames and decoded GIF contact sheets were inspected. The clips show the Connections gate and actual native approval through pending to missing.
Attachments are uploaded through the PR CLI and their downloaded hashes are checked separately; no GIF is committed.
Equipment notes: the first input attempt hit EOF without interactive stdin; an unsupported Comma shortcut and a screenshot during active input were refused.
Normal-exit input dispatched successfully; its later screenshot was unavailable because the container had exited as intended.
These equipment outcomes do not supply or invalidate the separately completed behavior observations.

## 6. Evidence and historical scope

Current native: `/tmp/horizon-1335-evidence/api-remove-native-review2-main5d44-a7b20210`.
Its root-cleanup-proof.json and state/native-result.json confirm completed cleanup; the earlier public snapshot002 still says cleanup pending because it predates that proof.
Public paired observations: `/tmp/horizon-1335-evidence/api-remove-r2-public-proof/summary-001.txt` and summary-002.txt, with original RPC records retained.
Local results: `/tmp/horizon-1335-evidence/api-credential-remove-main5d44/review2-qualification/qualification.json`.
Artifact manifest: review2-qualification/frozen-remove-currentmain-8636873150f0-bc14f20d/build.json.
Source/reuse proofs: source-reconstruction.json, unchanged-three-products-and-native-helper-reuse-proof.json and root-pre-native-product-and-input-verification.json.
Earlier review-one runtime0a and Capture-8a/main-c5 runs remain historical evidence for their own inputs; their skews are not current R2 results.
