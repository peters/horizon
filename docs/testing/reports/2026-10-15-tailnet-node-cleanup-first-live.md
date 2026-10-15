# Core recorded-node cleanup first live check

## Scope and inputs

On 2026-10-15, the first C03 lane in the
[Core cleanup procedure](../procedures/tailnet-node-cleanup.md) passed on a
task-owned Hetzner cpx22/fsn1 CPU worker. The checked source tree was
`4dc32869142ecfab92913fa0e2cb393178f2468a`, on the prospective `acbaf5eb`
main and SDK prerequisite tree `a2f0eb02bfe55fb68210bdb65aa500d0d69a99c9`.
The main base was already merged; the SDK combination was prospective at
qualification time.

The frozen `cloud_deploy` had cleanup explicitly enabled, SHA256
`371344af8eb41ef3c366fbbcc87f51e3d3db9e7a8b60c231693228a778da8927`.
The actual quick-start image was
`ghcr.io/peters/horizon-worker-base@sha256:a3abdf1800d95adccdf93438208a206d2353f9a6fe69c4529faea791f73230f7`.
The Core feature `tailnet-node-cleanup` remains OFF by default. This run does not
activate UI cleanup, native retry presentation or a public MCP delete operation.

## Local qualification

The applicable required default matrix and speech checks passed on 1,749 recorded
inputs, using fresh results and individually justified independent closure reuse.
The enabled Core suite passed 2,060 tests with 21 ignored. Existing storage
finalization, cancellation and confirmation regressions passed. Both feature-OFF
and feature-ON blocking and strict Clippy passed. Repository, formatting and
bundled skill checks passed. Advisory exited 101 on two unchanged inherited
findings; it is not an advisory pass.

These results belong to the recorded prospective source. Earlier failed or
capacity-stopped checks are not counted as passing. Exact-source comparison after
the actual SDK merge and final-head C03 remain required.

## Missing API credential after provider deletion

The original controller reached Ready and captured the worker's actual Self.ID
with its saved-tailnet, worker, allocation and provider bindings. Its task
enrollment key was revoked before deletion. The saved tailnet had no desktop API
credential when the enabled Core delete command ran.

The first delete exited 1 after approximately 10.9 seconds. Provider deletion
completed: the original controller record had Stage Deleted, no current worker,
and its original receipt still had no node-deletion confirmation (the false
default is omitted from its saved JSON).
Separate observations proved the original server, volume and SSH key each
returned 404 and all exact-operation collections were empty.
An independent exact-node GET still found the same recorded task node.

The error identified the exact saved-tailnet and device binding, stated that the
saved tailnet had no desktop API credential, and directed the owner to preserve
the original cloud record and remove that exact node manually or retry cleanup.
Missing access did not discard the receipt or report node deletion.

## Explicit Core retry and durable confirmation

The existing private credential adapter bound a Personal API credential to the
same task saved-tailnet ID. The identical enabled Core delete command then
returned 0. The original receipt's `node_deleted` confirmation was true in the
saved deployment record. The original record, rather than a fabricated receipt,
supplied the evidence.

Core used its original locked cloud record and durable provider-deletion
journals. No external fixture attestation supplied Core deletion authority.
The private adapter managed fixture credentials and read-only observations; it
did not perform a direct SDK delete as a substitute for this Core operation.

Confirmation was read from the original saved JSON. The older Capture reader may reject the new true confirmation field;
its decoder was not modified or treated as proof that the current record failed.

## Repeat with API binding removed

The task's managed Personal API binding was removed and its status was Missing.
The identical Core delete command again returned 0, with no progress output.
The original deployment bytes were unchanged. The repeat therefore required no
API credential once its exact receipt was durably confirmed; it did not become
an unverified second SDK deletion.

Independent inventory found the task node absent and the two original protected
node/API ID pairs unchanged. These ID-pair checks do not assert equality of whole
responses or volatile last-seen metadata.

## Reconciliation and limits

The first C03 lane completed cleanup. Original provider resources were absent,
the task node was absent, and the task API binding was removed. Task enrollment
Auth/catalog cleanup occurred last. Original protected node/API pairs remained
intact. The original controller record, receipt, journals and evidence were
retained; no name-based or broad deletion supplied authority.

This Core live lane tested a missing credential followed by a Personal credential.
It does not qualify live OAuth through Core, real offline/network failures,
RunPod, DockerHost or other provider deletion lanes. Synthetic controller tests
and the separate SDK OAuth live results remain distinct evidence.
Default activation, UI card retention/retry/discard behavior and native/public
interface qualification belong to a later outcome.

This is first-reviewable-source evidence only. The final actual public-head C03
run remains required after prerequisite reconciliation and review fixes.
The public report omits private resource IDs, credential slots, account details
and paths. Private evidence retains the exact original bindings and observations.
