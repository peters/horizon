---
procedure: exposure-management
candidate_commit: c8cc26d3ef10ffc0b0c6adcd8950f658e74d086f
candidate_sha256: 9d9f9b0fdd14b61dc540c35ecde821d1fd057eaca1437c30941c299e1e6450dd
executed_source_patch_sha256: ef25d3c331841fc9a5de7c2a157f074b9c823fe81036af1a0fa20b2ab276488c
tested_source_inventory_sha256: f340fd631a3206c1f7f747929d1c933ac999370d61b898c74ed7428a0c3533c7
date: 2026-10-16
lanes: [local-linux, actual-hetzner-management]
issue: https://github.com/peters/horizon/issues/1335
---

# Original SDK exposure management first test report, 2026-10-16

## 1. Summary

The first reviewable SDK Manager passed its actual provider test. The same
original client preserved management SSH access, sibling policy contributions
and the firewall identity across removal and reuse. Original-only cleanup
completed, and fresh provider reads showed no owned resource remained.

The header commit identified the base of the uncommitted candidate. The patch
hash identified its executed source change. The inventory hash identified the
JSON snapshot of 2,333 source files before this report and its index entry.
Every recorded source hash matched before and after the real run. The candidate
hash identified the frozen enabled Core test executable, not a normal worker.

This test qualified the SDK management component. It did not qualify Gate bytes,
provider source preservation, RunPod, Tailnet, Funnel, Native approval or public
UI/CLI/MCP exposure operations. The SDK feature and application activation stayed
off by default. A final-head provider check remained a separate requirement.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| M01 | pass | The enabled SDK passed 515 tests and two doc tests. The default workspace graph passed 7,033 tests and 13 host complements, with 55 intentional ignores. Speech passed 2,416 tests, with three intentional ignores. | — |
| M01 | pass | Enabled Core passed 2,207 fixture tests and 12 host complements, with 24 intentional ignores. Required blocking/strict tiers, docs, formatting, maintainability, skill and API checks passed. | — |
| M02 | pass | One actual ignored consumer passed in 36.37 seconds. Original SSH access, sibling retention, same-firewall reuse, exact retirement and refusal to reopen the retired Manager passed. | — |
| M02 cleanup | pass | Normal deletion returned zero at about 343 seconds after deployment started. All five paid resource IDs and the original firewall returned exact 404 responses. The seven-kind inventory equalled the baseline. | — |

The real run used one CPX22 in fsn1, with two CPUs, 4 GB RAM and a 10 GB volume.
The original normal deployment created its Store, provider journal, worker and
SSH pins. It reached Ready after 126.8 seconds. No Ready state or ownership
record was copied into the deployment.

The worker used this immutable image without an executable overlay:

`ghcr.io/peters/horizon-worker-base@sha256:a3abdf1800d95adccdf93438208a206d2353f9a6fe69c4529faea791f73230f7`.

The normal `cloud_deploy` executable had `profile.test=false` and SHA-256
`ffe36142c9951050c020a9aa9b0dfaf5cbdfcdaaad83fca73a4d92e2b36f6b49`.
The normal profile kept Tailnet absent and published only SSH 22. It requested no
agent, browser, desktop or protected exposure ingress capability.

The consumer locked that original Store. It checked the actual allocation,
server, volume, registered public key, immutable image, Settings reference and
original pinned SSH endpoint. A nonce and the actual SSH connection tuple
established the management source. Credential and private-key comparison used
metadata; credential values were not copied into the repository or worker.

The same SDK client created one scoped firewall and applied the approved
management policy. The consumer added two service contributions, removed one
while retaining the other, removed the last while retaining management access,
then reopened a contribution on the same firewall. Fresh policy readback and
actual pinned SSH checks passed throughout. The consumer removed and deleted
the original firewall, confirmed absence and refused a later reopen attempt.

The exclusive, fsynced SDK journal retained 23 rows. Its intent order was Create,
Apply, four Rules changes, Remove and Delete. Returned firewall and action IDs
were recorded before action waits. The original child exited zero and was
joined before the supervisor's original-only reconciliation.

The quote was EUR 0.0312 per server hour, with volume and address charges
separate. The run used a USD 3 cap, a 20-minute cleanup deadline and a 30-minute
hard bound. The independent outer watch completed after about 346 seconds.
The run did not retrieve an invoice or certify the remaining epic budget.

The offline matrix used Rust 1.98.1, locked offline dependencies,
`RUSTFLAGS=-D warnings`, six build jobs and no incremental compilation.
The SDK's 515-test result reused an explicit comparison of 118 SDK/configuration
inputs after the Main update; the SDK had no dependency on the changed Core
command path. The Core/default/speech graphs ran on the current base.

A later ignored-consumer-only lint correction changed one feature-gated test
leaf. Its enabled Core graph, full Core test root, host complements and required
tiers ran again. Actual default Core dependency information excluded that leaf;
production code, guidance and ordinary graph inputs stayed unchanged. Both
normal UI graphs embedded the complete paired cloud skills and references.
The worker Python suite passed 730 of 738 unique tests. Eight root-only cases
were local non-applicable cases and still required exact-head GitHub root CI.

## 3. Defects

No runtime failure occurred in M02. Introduced lint findings in the ignored
consumer were corrected before execution. The initial lint failure and an
intermediate unsupported digest-formatting attempt remained in private evidence.

Pedantic advisory checks exited 101 on unchanged Main excessive-boolean
declarations. A Core-only check reached the new consumer without dependency
linting and reported only six unchanged Main declarations. The existing
unknown-lint warning also remained. No lint suppression was added, and this was
not recorded as a full pedantic pass.

## 4. Deviations from the procedure

The direct SDK consumer opened no service socket or Gate. Its service rules
were policy inputs, not data-plane or source-attribution evidence. The actual
server started without foreign firewall attachments; foreign-policy retention,
lost responses, failed saves and refusal paths remained offline regression
coverage. The successful live run did not exercise orphan removal after process
loss. Its supervisor observed the already retired original firewall's exact 404.

An initial local plan omitted a required worker-choice field and refused before
provider mutation. The complete choice then passed the normal plan. One local
Board launcher used a relative executable path and failed before spawning its
test; its corrected absolute-path check passed. These equipment failures were
retained separately from product results.

## 5. Cleanup

- The original Manager deleted its exact firewall. The supervisor confirmed its absence.
- The normal controller deleted the task server, volume, registered SSH key and two primary addresses.
- Fresh exact-ID reads returned 404 for all six resources. All seven paginated baseline ID sets matched, with no unclassified additions or loss.
- Both original normal image-preflight containers were absent on the same healthy Docker daemon.
- The original SDK child, normal deployment, supervisor and outer-watch processes were absent. Their original process groups had no remaining member.
- No Auth, enrollment, separate guest, overlay or Native operation ran in this window. Private original journals remained evidence; they did not authorize a restored controller.

## 6. Evidence

Private task evidence retained the original normal plan and quote, source and
executable inventories, actual Ready record, provider journal, Settings/pin
digests, metadata-only key bindings, pinned SSH observations, SDK intent/action
journal, raw test output, supervisor result, exact absence reads and baseline
comparison. This public report omitted account details, resource IDs, addresses,
private paths and key material.
