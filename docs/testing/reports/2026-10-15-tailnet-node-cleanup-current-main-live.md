---
procedure: tailnet-node-cleanup
candidate_commit: bbfa90971fae780c9ee97f6b61815961083979a7
candidate_tree: 1afd5b6c5e8606a484087ffb6115bbffa007f713
candidate_sha256: 03698440ad45d47fcd2c09b27a61053f8a9c974f273d9b1bd5fc9faa51912afa
date: 2026-10-15
lanes: [local-linux, hetzner-cpu]
issue: https://github.com/peters/horizon/issues/1335
---

# Tailnet node cleanup test report, 2026-10-15

## 1. Summary

The required local matrix and the fresh real-input C03 lane passed. The backend stayed OFF by default. Current UI tests and required lints ran on the actual `bbfa9097` base plus Backend tree `1afd5b6c`; unchanged Core and non-UI results were reused after input comparisons. Default activation, Core OAuth and real network-failure lanes were outside this report.

The normal enabled `cloud_deploy` artifact retained its original build identity: Backend tree `b0e7aad10e10fd778ac1f89893baaa1c8867df6c`, based on `7a275a95` and the exact #1640 test change. Its SHA-256 is in the header. Actual Core features were `cloud-workspaces,default,tailnet-node-cleanup`; `profile.test` was false. This artifact performed Core deletion. The credential fixture and independent observers did not replace it with direct SDK deletion.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| C01/C02 | pass | Enabled Core passed 2,129 tests, with 21 intentional ignores. Fifteen focused redeploy/history/Hetzner-transition regressions passed. These were automated component tests. | — |
| C03 initial deployment | pass | One approved CPU worker reached Ready in 126.60 seconds, with initial exit 0 and no recovery or replacement. Actual UID 10001, marker 4, private state, sole worker tag and original receipt bindings matched. | — |
| C03 missing API | pass | The task Auth key was revoked first. Core delete returned exit 1 after 13.03 seconds, with the precise missing-credential/manual-recovery message. The original receipt stayed unchanged and unconfirmed. | — |
| C03 provider proof | pass | Independent observations found all three exact provider resources absent by HTTP 404 and all matching operation collections empty. A separate API read found the same original node present. | — |
| C03 explicit retry | pass | With a task Personal API binding, the identical Core delete command returned exit 0 in 0.1055 seconds. The exact original group confirmation was durable. | — |
| C03 repeat without API | pass | After task API removal, status was Missing. The same command returned silent exit 0 and original deployment bytes stayed unchanged. | — |
| C03 independent preservation | pass | Full inventory contained the two original devices and no task node. The original protected node/API ID pairs matched the fresh baseline exactly. | — |
| C04 OFF/ON custody | pass | Required suites covered the locked pending-record check, missing identity, original history and redeploy refusal. Native activation was separate. | — |

Fresh current-source UI tests passed: default 2,251 and speech 2,297, each with two intentional ignores; font and home integration added five passes in each configuration. Both OFF and ON blocking/strict Clippy tiers passed. Formatting, maintainability, bundled skill coverage and diff checks passed.

Reused results covered all default targets and doctests of 23 non-UI workspace packages and the vendored workspace member, including OFF Core 2,098, SDK 471 and worker 210. Enabled Core 2,129, its 15 focused regressions and the normal CLI were also reused. All relevant source, fixtures, 29 manifests/configuration files, lockfile, toolchain, flags, environment and workflows matched. All 560 actual current UI dependency feature/profile entries matched the prior complete 644-entry workspace graph. The six changed compiler inputs were four setup UI files and two UI-embedded references; their consumers received new checks. Core/SDK had no Git-identity consumer. The one test-only manifest-directory fixture selected identical payloads.

## 3. Defects

No blocking or strict failure remained. Advisory Clippy exited 101 for the same two inherited `struct_excessive_bools` findings in cloud-provider `Description` and browser `GroupState`. Their source/configuration/toolchain baseline proof matched. This was not a pedantic-clean result.

## 4. Deviations from the procedure

The first workspace attempt stopped at the capacity guard. A subsequent complete workspace run passed after bounded task-artifact cleanup. That original stop remained in the private evidence; it was not classified as a product failure.

After #1640 merged, its actual base also contained #1636. All Core/SDK and CLI inputs matched the tested b0 candidate. Current UI/default/speech and both feature configurations of required lints ran again. Reused results retained their original source and artifact identities. All 1,785 current compiler inputs matched after qualification and after this report-only update.

Task-node absence used full inventory. No post-confirmation exact-node GET 404 was claimed. Core OAuth, actual offline/network failures, RunPod/DockerHost deletion and native activation were not qualified. Earlier 6c/default58 and other live results stayed historical; this report replaced the prior current-main summary without changing their preserved evidence.

## 5. Cleanup

The task server, workspace volume and provider SSH key were deleted and independently absent. The task tailnet node was absent from full inventory. The task API alias was removed. The task Auth key was revoked and invalid; its local Auth/catalog cleanup ran last. The task catalog was empty and original Auth remained accessible. No paid task resource, node or task credential alias remained active.

## 6. Evidence

Private evidence retained exact source inventories, commands, flags, compiler feature/profile records, build/freeze provenance, original deployment bytes, independent provider/tailnet observations and cleanup proofs. The normal CLI retained its b0 build identity; current-source reuse was separately proved. Operational IDs, private hosts and credentials were excluded from this report.

The final public-head GitHub checks, independent review and merge gates remained separate from these completed local and runtime results. No visible backend UI change occurred while the feature stayed OFF; activation required its own native evidence and GIF.
