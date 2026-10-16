---
procedure: tailnet-node-cleanup
candidate_commit: 83f7a8e1cd558b2faa6a28ff717e7a177c11a7f6
candidate_tree: c5ce3c10cd4d9dcb4eb917f0c0f203516a11cdc1
candidate_sha256: a12fde0a909cdff1b2d2dfc3b2f03f64654d2e7a5a5a21c539e6c3013ece50e8
date: 2026-10-15
lanes: [local-linux, dockerhost-capacity, hetzner-cpu]
issue: https://github.com/peters/horizon/issues/1335
---

# Tailnet cleanup correction and live test report, 2026-10-15

## 1. Summary

The corrected backend passed a real DockerHost capacity-refusal lane and a fresh Hetzner CPU C03 lane. Both used the normal enabled Core CLI identified in the header. The backend remains OFF by default; default activation and native behavior are separate outcomes.

The tested tree includes uncommitted corrections on the recorded commit. The normal artifact used Core features `cloud-workspaces,default,tailnet-node-cleanup`, SDK features `[]`, and `profile.test=false`. All 1,786 recorded compiler inputs matched this source. Later report-only changes do not rename the tested tree or executable.

## 2. Correction boundary

A DockerHost attempt that was refused before worker creation retains its genuine Prepared history through deletion and retry. Any original worker, enrollment, request or operation uncertainty keeps the existing cleanup guard. If either SDK lookup actually finds an owned container while Prepared, the Requested transition is persisted before container removal. A later lookup cannot erase that evidence or silently turn the attempt into a never-provisioned cloud.

The same original Store custody protects provider deletion, exact receipt confirmation, ordinary removal and redeploy history. This report does not treat a synthetic saved selection, absent API binding or successful provider deletion as proof of tailnet-node deletion.

## 3. Real-input results

| Case | Result | Observed behavior |
|---|---|---|
| Docker capacity refusal | pass | An impossible CPU/memory request failed before image pull or worker intent. No worker, enrollment, container or volume was created. |
| Docker delete and repeat | pass | Both commands returned exit 0. The original record durably retained Deleted/Prepared history without a worker or enrollment. |
| Docker same-cloud redeploy | pass | The original cloud reopened into Provision, reached the same real capacity refusal, and then deleted successfully. Final owned container and volume inventories were empty. |
| Docker API status | observed | The deliberately disconnected private OS store reported Unavailable. Missing was not observed or required for this lane. |
| Hetzner initial deployment | pass | One approved CPU worker reached Ready in 133.0126 seconds with exit 0, without recovery or replacement. Actual UID 10001, marker 4, private state, sole worker tag and original receipt bindings matched. |
| C03 missing API | pass | Task Auth was revoked before Core delete. The command returned exit 1 after 13.0577 seconds. The original exact receipt remained unchanged and unconfirmed. |
| C03 independent observations | pass | All three original provider resources returned HTTP 404 and matching operation collections were empty. A separate API observation found the same original captured node still present. |
| C03 explicit Personal retry | pass | After binding the task Personal API credential, the identical Core delete command returned exit 0 in 0.10845 seconds. Confirmation of the exact original group was durable. |
| C03 repeat without API | pass | After removing the task API binding, status was Missing. The same Core command returned silent exit 0 and original deployment bytes were unchanged. |
| C03 preservation | pass | Independent full inventory contained only the two original devices and no task node. The two original protected node/API ID pairs matched the fresh baseline exactly. |

The Docker capacity request was refused on the actual task-owned local daemon; no paid capacity was used. This was not a complete enrolled Docker worker lifecycle. There is no public CLI `can_remove` operation: eligibility was covered by the separate controller regression, not inferred from CLI deletion.

The Hetzner lane used one CPU worker, no GPU, and the immutable A3 worker image. Task Auth/catalog cleanup ran last. The credential adapter and observers supplied fixture credentials or independent observations; they did not replace Core deletion with a direct SDK delete.

## 4. Local validation status

Recorded current results include enabled Core 2,133 passes with 21 intentional ignores, disabled Core 2,098 passes, SDK 471 passes with one intentional ignore, UI default 2,251 passes and UI speech 2,297 passes, each with two intentional ignores. Each UI configuration also passed three font and two home-isolation integration tests. Focused Docker transition, cleanup and redeploy checks passed. Required blocking and strict Clippy tiers passed with the backend both ON and OFF after the narrow test-only lint corrections.

Fresh results also cover all default targets in the seven affected workspace packages, including integration targets, native test binaries, normal examples and complete workspace doctests. Complete results for the seventeen independent workspace crates and the vendored member are reused only after exact consumed-input, fixture, manifest, toolchain, environment and actual feature/profile comparisons. Reused results retain their original source identities. The full applicable local matrix is covered by these new and input-verified reused results. Advisory Clippy exited 101 on the same two inherited boolean-field findings and an inherited unknown-lint warning; no new or worsened advisory finding was identified. This is not a pedantic-clean result. Exact-head GitHub checks and independent review remain separate gates.

## 5. Deviations and limitations

An additional Docker status assertion initially expected Missing; the actual disconnected-store result was Unavailable. The original deploy/delete/redeploy actions had already passed and resource absence was confirmed. No action was repeated to change that status observation.

A private Hetzner offer check initially examined the default-provider list. The actual approved Hetzner offer was selected before effects; no key, deployment or creation request was duplicated.

This report does not qualify full enrolled DockerHost cleanup, Core OAuth cleanup, offline/network-failure recovery, RunPod cleanup, default feature activation or native UI behavior. Post-confirmation task-node absence used full inventory; no exact-node GET 404 is claimed. Original protected pair equality proves those IDs were preserved, not equality of every mutable device field.

## 6. Cleanup and evidence

The Docker lane created no provider worker, tailnet node or credential alias. Its original records were preserved and final owned container/volume inventories were empty.

For Hetzner, all task paid resources and the task node were absent. The task API binding was removed. The task Auth key was revoked and invalid before final local Auth/catalog cleanup. The task catalog was empty and original Auth remained accessible. No task paid resource, node or credential alias remained active.

Private evidence preserves the exact source inventory, normal artifact identity, commands, original receipt bytes, independent provider/node observations and both completed cleanup results. Operational IDs, account details, local runtime paths and credential values are excluded here. Earlier reports retain their original tested sources and results; they are historical rather than substitutes for this corrected source.

Procedure: [tailnet node cleanup](../procedures/tailnet-node-cleanup.md). Exact-head CI, independent review and merge readiness remain separate gates.
