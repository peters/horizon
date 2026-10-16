---
procedure: cloud-tailnet-selection
candidate_commit: 556536a5c3cc92b373b1a22190180abc2edbba69
candidate_sha256: b4a538db32f1635f268c6660984ac628c4b5bd6efe433eff463bb52335c39f90
date: 2026-10-16
lanes: [linux-default, linux-speech, linux-example, linux-original-catalog]
issue: https://github.com/peters/horizon/issues/1335
---

# Cloud tailnet selection test report, 2026-10-16

## 1. Summary

The current normal CLI selected the original saved tailnet for one fresh task-owned cloud. The original catalog bytes and recorded metadata stayed unchanged. Only the new selection record and lifecycle lock were created; both were removed after verification. No deployment, enrollment, provider operation or credential lookup ran in this current S02 check.

The checked commit was `556536a5c3cc92b373b1a22190180abc2edbba69`, rebased onto actual Main `da74fc9d75181dfd8af412566ea241e53e0e2a0f`. Its 2,358 source inputs and eleven required executable products were recorded. Cargo identified the CLI as `profile.test=false`, with `cloud-workspaces`, `default` and `tailnet-node-cleanup` features. The header identifies the actual checked commit and normal executable, before this report-only update.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| S01 | pass | The full Unix example suite passed 21 cases; the declared per-platform filter passed eight cases locally on Unix. Argument, lock, saved-network and allocated-cloud refusal checks passed. | — |
| S02 | pass | The current normal CLI selected the original saved ID. The catalog hash, file metadata and existing root-entry metadata stayed unchanged. No deployment was created. | — |
| S03 | pass | Current default coverage totalled 7,056 passes including thirteen host complements. Fresh speech coverage totalled 2,417 passes. Required lint tiers and repository checks passed. | — |

Sixty-two default roots ran freshly. Only the cursor root, representing thirty passes, reused an identical executable with matching consumed source, build, feature, environment and runtime inputs. Canonical package-and-target mappings retained all 63 current roots, their hashes and raw results. All 63 test executables were frozen separately.

Fresh enabled checks passed 531 SDK cases and two doctests, 113 gateway cases and three owner-custody cases. The fresh worker Python suite passed 730 cases; eight root-only cases were local N/A for GitHub Linux coverage. Both normal UI products contained all four complete current cloud guidance assets. The builds used locked offline dependencies, six jobs, disabled incremental compilation and `RUSTFLAGS="-D warnings"`.

Blocking and strict tiers passed. Both pedantic advisory commands returned 101 after stopping at unchanged Main boolean-struct findings in the provider description and browser group state. These were incomplete advisory scans, not clean completed scans. The unchanged Cargo configuration also reported an unknown `clippy::assert_is_empty` lint. No warning suppression or unrelated source fix was added.

## 3. Defects

No current selection product defect was found. An enabled fixture first refused before container creation because an unrelated SDK example had overwritten an unused mutable companion alias. The corrected fixture omitted that unused companion and passed. A host-complement command first refused before test spawn because it used a relative evidence path; the unchanged test then passed once through its absolute frozen executable. Both equipment failures and corrections were retained.

## 4. Deviations from the procedure

The current S02 check followed the procedure without deployment or enrollment.

Historical additional evidence used the earlier normal CLI with SHA-256 `7d27cf021510fab0a52c7d34aa9c86dca6977c0cca3aa0724932d9bfd50f8a24` on base `fa25ff490bc35252ded528ed7e770e873c442e4d` plus the then-uncommitted selection patch. That earlier cloud reached Ready on the pinned ordinary A3 image with its actual selected-network node receipt. Normal deletion recorded node deletion; independent checks confirmed all five provider resources absent, all seven resource ID sets preserved and the protected devices, policy and catalog unchanged. Ready took about 136 seconds; deletion took about 13.21 seconds after its request. Those results are historical evidence, not an enrollment result for the current binary. A separate Producer equipment attempt refused before SSH and triggered normal cleanup. No Worker overlay ran in that earlier window.

This selection outcome supplies no userspace Producer, Native, public-sharing or complete M2 qualification.

## 5. Cleanup

The current S02 check removed only its exact new selection record, lifecycle lock and empty cloud directory. The original catalog comparison remained equal after removal. Each of the four current local test fixtures was removed; fresh inspection and an exhaustive inventory on the healthy original daemon confirmed their exact absence. No current paid resource was created.

The earlier authorized enrollment window also cleaned its task nodes, Auth key, private binding and guest processes. Its original receipts, uncertainties and cleanup corrections remain retained with the historical report.

## 6. Evidence

Private evidence contains the exact current and historical source inventories, compiler artifacts, normal executable hashes, all canonical runtime mappings, commands, logs, fixture identities, original-catalog comparisons and exact cleanup observations. Current results and reused results remain separate. No private provider ID, host address, credential payload or credential path is included here.

See the [procedure](../procedures/cloud-tailnet-selection.md) and [trust boundary](../../architecture/tailnet-enrollment.md).
