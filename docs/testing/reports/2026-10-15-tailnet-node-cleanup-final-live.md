# Core recorded-node cleanup — final candidate live report

On 2026-10-15, C03 passed on candidate `4551f56ea5149720fa3a1b24b8f3167541eba329`, tree `519a28e837ff4744bdc59af18cc5f79735ef1913`, based on actual SDK squash `01d6463fc02aab1327624ebddd4f875b17285cb0`.
The backend remains default OFF. This report does not qualify UI activation.

## Candidate and result reuse

The explicitly enabled Core CLI had SHA-256 `371344af8eb41ef3c366fbbcc87f51e3d3db9e7a8b60c231693228a778da8927`.
Its original build used tree `4dc32869142ecfab92913fa0e2cb393178f2468a`, with `cloud-workspaces,default,tailnet-node-cleanup` and Rust 1.98.1.
It was reused after comparing the final candidate with all 1,749 recorded inputs and the additional consumed schema.
Exactly one recorded input differed: the merged SDK's test-only accepted-socket fixture correction.
The normal CLI dependency excludes that leaf; its production inputs and feature configuration matched.
This is artifact reuse with recorded provenance, not a new build from the final checkout.

The required default and speech results, enabled Core's 2,060 passing tests with 21 ignored, and blocking/strict results retain their original matching-input evidence.
The corrected SDK cloud suite passed 471 tests, one ignored, including all ten Node API units.
Its actual 421-node workspace feature graph matched the backend's recorded graph without mismatches; the corresponding 104 cloud input files matched the final candidate.
Fresh changed-file formatting, maintainability, bundled-skill coverage and diff checks passed in the final checkout.
The advisory result remains exit 101 on two inherited findings and is not reported as passing.

## Actual worker and original authority

One authorized Hetzner cpx22 worker in fsn1 used two CPUs, 4 GB memory and the declared 20 GB storage choice, at the observed EUR 0.0312 hourly compute rate.
It reached Ready in 132.7 seconds using the immutable worker image `ghcr.io/peters/horizon-worker-base@sha256:a3abdf1800d95adccdf93438208a206d2353f9a6fe69c4529faea791f73230f7`.
The live probe verified UID 10001, marker 4, private isolation state, selected-tailnet agreement and the sole worker tag.
One original Self.ID receipt matched that saved tailnet, provider allocation and worker, with no unresolved capture attempts.
An independent exact-node read matched the original receipt before deletion.

The task enrollment key was revoked before deletion. The original cloud record, provider journals and captured receipt remained the controller's authority.
No fixture-supplied provider attestation or direct SDK delete replaced the actual Core delete path.

## Missing credential, explicit retry and durable repeat

Before the first delete, the actual CLI reported Missing API access for the exact task saved tailnet.
The enabled Core delete exited 1 with the precise missing-desktop-credential and manual/retry message.
Its original record was Deleted, its worker was absent, and its receipt remained unconfirmed.
Independent provider reads returned 404 for all three original server, volume and SSH-key IDs; all original-operation collections were empty.
An independent exact-node GET still found the same captured task node.

The existing credential adapter then bound Personal API access only to that task saved-tailnet ID.
The identical Core delete command exited 0 and persisted `node_deleted: true` on the original exact receipt group.
Other receipt fields were unchanged.
The task API binding was removed and the actual CLI again reported Missing.
The identical Core delete command then exited 0 with empty stdout/stderr and byte-identical original deployment data.

Independent API inventory subsequently contained only the two original protected nodes.
Their exact node/API ID pairs matched the pre-run baseline.
This is inventory and ID-pair evidence, not equality of volatile response fields or an additional exact-node HTTP-status claim.

## Cleanup and limits

Final independent provider reads reconfirmed all three original IDs absent and all original-operation collections empty.
Task API access was removed before the task enrollment Auth/catalog binding was removed last.
The task enrollment key was already revoked and invalid; retained remote revocation metadata is not an active credential.
The original deployment, confirmed receipt, journals and private evidence were preserved.
No paid task worker or task node remained at completion.

This final lane and the [first live lane](2026-10-15-tailnet-node-cleanup-first-live.md) qualify Missing access followed by Personal access through Core on Hetzner.
Live OAuth through Core, actual offline/network failures, RunPod and DockerHost deletion remain unqualified.
Synthetic controller tests cover retained transport/authorization/404 uncertainty separately.
SDK-only OAuth qualification does not substitute for a Core OAuth run.
The final report and its index entry add prose only; they do not change the qualified executable inputs.
Exact-head GitHub checks and Copilot review remain separate PR gates.

Procedure: [Core recorded-node cleanup](../procedures/tailnet-node-cleanup.md).
