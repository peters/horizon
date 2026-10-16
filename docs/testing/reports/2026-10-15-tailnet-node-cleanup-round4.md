---
procedure: tailnet-node-cleanup
candidate_commit: 554580e0d32a78f851296826ac40c972af76742b
candidate_tree: 8c46398b424cab424d185fa5bad8033e8404ca86
candidate_sha256: 76db10866cf5c295b6d75fd08727a459b47c8e6791bb3f20d28fe6d1ac4fd4e0
date: 2026-10-15
lanes: [local-linux, dockerhost-capacity, hetzner-cpu]
issue: https://github.com/peters/horizon/issues/1335
---

# Tailnet cleanup observation-history report, 2026-10-15

## 1. Summary

The full applicable local matrix is covered by new affected checks and proven reuse of independent results. Required blocking and strict tiers passed with the backend both ON and OFF. Fresh DockerHost capacity smoke and Hetzner CPU C03 passed and cleaned up using the normal enabled CLI in the header. Advisory Clippy exited 101 on inherited findings; this is not a pedantic-clean result.

The tested tree contains uncommitted corrections on the recorded commit. The backend remains OFF by default. The normal CLI was 134,264,656 bytes, built with Core `cloud-workspaces,default,tailnet-node-cleanup`, SDK features `[]`, and `profile.test=false`. All 1,786 recorded compiler inputs matched. Report-only changes retain the tested tree and executable identities rather than relabeling them as a later build.

## 2. Correction boundary

Every positive owned-container observation must preserve Requested history, even when the container disappears before retry. Core's preliminary lookup already persisted that state. Both SDK lookups now call the same `find_for_delete` helper, saving Requested before removal or a final-verification error.

The new controller regression observes absence at the first two lookups and a container only at final verification. Requested must be durable. After external container disappearance, retry, normal removal and redeploy must stay manual. The save-failure case exercises all three lookup positions and forbids removal when the Requested save fails.

Original provider absence, receipt groups and Store custody remain authoritative. A name, selected saved tailnet, missing API binding or later container absence grants no cleanup authority. Genuine capacity refusal before worker intent remains distinct from prior worker, enrollment, request and uncertain history.

## 3. New local results

| Check | Result | Executed coverage |
|---|---|---|
| Docker controller / redeploy / SDK Docker | pass | Five controller regressions, fifteen selected redeploy tests and fourteen SDK Docker tests. |
| Full enabled Core | pass | 2,134 passes, 21 intentional ignores, including all 28 actual tailnet cleanup tests. |
| Full disabled Core | pass | 2,098 passes, 21 intentional ignores. |
| Full SDK | pass | 471 passes, one intentional ignore; includes ten NodeApi unit tests. |
| Other affected default libraries | pass | Host 105, runtime 48, protocol 35 and worker 210 passes; worker had three intentional ignores. |
| Default UI | pass | 2,251 passes, two intentional ignores, plus font 3 and home-isolation 2. |
| Speech UI | pass | 2,297 passes, two intentional ignores, plus font 3 and home-isolation 2. |
| Other affected targets | pass | Default integration/native-binary targets, normal example compilation and fresh complete workspace doctests. Ignored live targets were not executed. |
| Required tiers | pass | Fresh blocking and strict Clippy with cleanup ON and OFF. |
| Repository checks | pass | Formatting, maintainability, both bundled skill coverage and diff checks. |

Checks used Rust/Clippy 1.98.1, two build jobs, no incremental compilation, line-table debug information and warnings denied. Feature/profile comparisons covered the actual default library, UI and integration graphs. Normal examples were compiled with `test=false`; no example runtime or libtest execution is claimed. Fresh complete doctests retain their own actual host profiles rather than asserting equivalence to the older combined job.

## 4. Reused results and advisory status

Passing standard target-class results for 17 independent workspace crates and the vendored member were reused. Their complete sources, fixtures, readmes, build scripts, manifests, lockfile, configuration, toolchain, flags and relevant environment matched. Inverse dependency proof excluded every changed Core/SDK consumer; both changed embedded references are consumed by UI. All 424 emitted current default dependency feature/profile nodes matched the complete prior 644-node witness. No independent build/test path consumed changed Git identity. Fresh private home and absent-secret-service paths retained the same isolated fixture semantics.

This reuse does not include the seven affected packages. Their new checks, plus current target and doctest execution, complete the applicable matrix. Original results keep their original source identities; this is not a claim that one new whole-workspace command ran everything.

Advisory Clippy exited 101 for the same `struct_excessive_bools` findings in cloud-provider `Description` and browser `GroupState`. Their source hashes, root lint configuration, toolchain and flags matched the baseline. The same run emitted inherited warning E0602 for the unknown `clippy::assert_is_empty` allowance. No introduced or worsened advisory finding was observed, and required tiers passed without suppression changes.

## 5. Fresh real-input results

| Case | Result | Observed behavior |
|---|---|---|
| Docker capacity refusal | pass | An impossible CPU/memory request failed before image pull or worker intent. No worker, enrollment, container or volume was created. |
| Docker delete / repeat / redeploy / final delete | pass | The original cloud durably retained Deleted/Prepared history, reopened into Provision, reached the same capacity refusal and deleted successfully. Owned container/volume inventories were empty before and after. |
| Docker API status | observed | Actual status JSON was Unavailable with the disconnected private OS store. Missing is not claimed. |
| Hetzner initial Ready | pass | One CPU worker reached Ready in 154.5051 seconds, exit 0, without recovery or replacement. UID 10001, marker 4, private state, sole worker tag and original receipt bindings matched. |
| C03 missing API | pass | Task Auth was revoked before the first Core delete. Exit 1 followed after 8.93582 seconds, preserving the original unconfirmed receipt unchanged. |
| C03 independent observations | pass | All three exact provider resources returned HTTP 404 and matching operation collections were empty. The same original captured node remained present. |
| C03 Personal retry | pass | The identical Core command returned exit 0 in 0.107089 seconds after task Personal API binding. Exact-group confirmation was durable. |
| C03 repeat without API | pass | After API removal, status was Missing. The same Core command returned silent exit 0 with unchanged original deployment bytes. |
| C03 preservation | pass | Full inventory contained only the two original devices and no task node. The two protected node/API ID pairs matched the fresh baseline. |

The Docker lane used no paid capacity. Hetzner used one approved CPU worker and the immutable A3 image, with no GPU. Original-version adapters supplied fixture credentials or independent observations; no direct SDK delete replaced the actual normal Core CLI. Post-confirmation task-node absence used full inventory, not an inferred exact-node GET 404.

## 6. Retained deviations

The first five-test execution on tree `17d1073a` returned three passes and two failures: the fixture counted a global capacity list before the deletion lookups. The exact-owned-filter counter correction was confined to the test-only Python literal; production was unchanged. Its original failure remains separate from the current five passing tests.

The earlier cleanup selector exited 0 but selected zero tests. It is not coverage; the later current full Core run executed all 28 actual cleanup tests. An earlier separate Activation UI run had 2,257 passes and one worker-completion timeout. That failure keeps its original inputs; current fresh Backend UI passed the common test without claiming a proven race cause or relabeling the old run.

A read-only provider observer initially treated a saved key name as a numeric API ID and refused before effects. The original-operation collection and exact provider-key GET established the actual ID before Core deletion. No resource action was repeated.

## 7. Cleanup, limits and publication gates

Docker created no provider worker, enrolled node or real credential alias. Its final owned container/volume inventories were empty. For Hetzner, all task paid resources and the task node were independently absent. The task API binding was removed; task Auth was revoked and invalid. Local Auth/catalog cleanup ran last, leaving an empty task catalog and original Auth accessible. No task paid resource, node or credential alias remained active. Current compiler inputs and CLI hash were rechecked for the completed run.

Full enrolled Docker cleanup, Core OAuth, offline/network-failure recovery, RunPod cleanup, default activation, private SSH and native/public sharing were not qualified. Default-ignored live targets remain ignored. No new visible UI behavior exists while this backend is OFF, so new native smoke, GUI/device/browser/helper builds and a GIF were not applicable to this outcome. Activation requires its own current qualification.

Private evidence preserves original inventories, commands, logs, feature/profile proofs, receipt bytes, independent observations and cleanup results. Earlier reports remain historical. Operational IDs, account details, credential values and local runtime paths are excluded here. Procedure: [tailnet node cleanup](../procedures/tailnet-node-cleanup.md). Exact-head GitHub checks, independent review and merge permission remain separate gates.
