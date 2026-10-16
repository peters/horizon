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

## 7. Earlier post-review fix check (historical)

The fixed candidate `89a390725ccbad6958f809f24ceeeb0f16d5ebfe`, based on
merged Main `b7122fd7d38a785c1147d0b9a8e3dd5308f52862`, passed an earlier
post-review provider check. The first-run header and sections above remain historical
first-reviewable evidence. The fixed candidate retains requested create rules
before fallible action journaling, rejects fresh cleanup rule or SSH drift,
and latches failed current-state verification into cleanup-only uncertainty.

The final normal `cloud_deploy` executable had `profile.test=false` and SHA-256
`884c9156622d866ece45c575e37c80587b0a7606a38441d83efb68f2b8d82589`.
The enabled Core test executable had SHA-256
`d7b191c83f985a953bc929ea433ac798f6ef5562e8d1e1d20a4831017348091d`.
Both were frozen from the qualified current source. No executable was overlaid
on the worker. The fresh original normal deployment used the same immutable
image and resource shape as the first run, with Tailnet absent and SSH 22 only.
It reached Ready in about 120 seconds.

The actual consumer passed once in **34.44 seconds** with no stderr. Its
original-client checks covered continued pinned SSH, sibling contributions,
removal, same-firewall reuse, retirement and refusal to reopen. The exclusive
SDK journal retained 23 complete rows, including intent and returned action IDs
before waits. The original child was joined before fallback reconciliation.

Normal deletion exited zero about **276 seconds** after deployment started.
Fresh provider reads confirmed exact 404 responses for the five paid resources
and the original firewall. All seven paginated inventory ID sets equalled the
baseline. The original deployment, SDK child and supervisor were absent, with
no member left in their process groups. Both exact local image-preflight
containers were also absent. No Auth, Tailnet, separate guest, overlay, Gate or
Native operation ran. The final window used a USD 1 cap and the same 20-minute
cleanup and 30-minute hard bounds. No invoice or exact remaining budget was
claimed.

Local coverage for that check comprised 87 frozen products: default **7,048 passes**
including 13 host complements, speech **2,416**, enabled Core **2,219**,
gateway contract **113**, Owner-custody contract **three**, and freshly enabled
SDK **520 plus two doctests**. Default SDK passed 489 tests and two doctests.
Default runtime coverage combined 18 freshly executed roots with 45 roots
whose actual executable, features, consumed inputs and environment matched.
Worker Python coverage retained 730 passes and eight local root-only
non-applicable cases under an exact 387-input comparison. Required tiers and
repository checks passed; inherited pedantic findings remained advisory.

Two local equipment assertions refused while preparing this final window.
Offer selection initially inspected the empty primary-provider projection;
the retained normal output's configured-provider comparison supplied the
actual Hetzner offer without a repeated request. A later reporting assertion
incorrectly equated the SSH server's container address with the external
endpoint. The same successful pinned SSH output met the actual consumer's
valid-address and port contract; no SSH retry or product change was needed.
These records remain separate from the passing SDK result.

That earlier check qualified original-client SDK management, not public exposure,
Gate traffic, provider source preservation, RunPod, Funnel or Native approval.
The final report-only commit does not change the executed Rust, configuration,
embedded guidance or fixture inputs.


## 8. Final second-review fix qualification

The final second-review fixes passed a new actual provider check on the same
merged Main base. The checkout was at commit
`aae3d7533ee9d7e91364de8eb230c42bd63ab63e`. Its executed uncommitted
source patch had SHA-256
`e3e2bb664fb1110d12285068f0d7fa760866f97f292340df1037c0f03d264a8a`.
The complete snapshot of 2,352 executed source files had SHA-256
`3295906dca44cc1201144feb1b3a1d5ed0a2fb260e6652d2d8ebd4a88d88b30a`.
All recorded source hashes matched through the actual run and cleanup, before
this report-only update.

The enabled Core consumer executable had SHA-256
`075d8de11f6c9455f2752e2276aff91fbae0669e3a2b8aba72aded4691629237`.
The unchanged normal `cloud_deploy` executable had `profile.test=false` and
SHA-256 `884c9156622d866ece45c575e37c80587b0a7606a38441d83efb68f2b8d82589`.
The fresh original deployment used the same immutable image and resource shape
as the earlier checks, with Tailnet absent and SSH 22 only. It reached Ready in
**113.485 seconds**. No retained Ready state, executable overlay or restored
Manager was used.

The actual consumer passed **one test, with zero failures, in 40.44 seconds**;
its stderr was empty. It checked continued original pinned SSH, sibling policy
contributions, removal, same-firewall reuse, original retirement and refusal to
reopen. The supervisor retained the original process and journal custody through
cleanup. This successful run did not simulate lost replies, failed saves,
foreign-firewall drift or the refusal transitions covered by offline regressions.

Normal deletion exited zero **283.444 seconds** after deployment started.
Fresh direct provider reads returned exact 404 responses for all five paid
resources and the original firewall. All seven paginated baseline ID sets
matched. All five original process handles were absent and their original
process groups had no members. Both original image-preflight containers were
absent on the same healthy daemon. No Auth, Tailnet, separate guest, overlay,
Gate or Native operation ran.

Fresh offline qualification covered **529 SDK tests plus two doctests**,
**2,207 Core fixture tests plus 12 host complements = 2,219 passes**, and the
**three-test dual-feature Owner-custody contract**. The ordinary Board check
passed on its first attempt. SDK and affected Core blocking/strict tiers,
enabled Core docs and repository statics passed. All nine new cleanup
regressions retained their red and green evidence: six failed against the
previous implementation, one exposed the prior false confirmation on an empty
unknown-create lookup, and two failed against the intermediate implementation
that promoted an unvalidated or unsaved cleanup candidate.

Reused coverage was separately bound to unchanged consumed inputs and actual
feature graphs: default **7,048 passes** including 13 host complements, speech
**2,416**, gateway contract **113**, default SDK **489 plus two doctests**, and
worker Python **730 passes with eight root-only local non-applicable cases**.
These feature-off graphs exclude the three changed Rust leaves. Their toolchain,
dependencies, configuration, embedded assets and fixture inputs matched.
Product accounting retained **87 records: 86 current verified executable hashes
and one unneeded historical example alias** overwritten by another feature
graph. The alias was never used by a runtime or this actual provider check; its
completed ordinary compile was reused by input comparison rather than rebuilding
to restore a mutable path. Scoped pedantic checks exited 101 on inherited Main
findings; advisory results were not counted as passing required tiers.

The earlier pushed head's Linux and macOS checks passed, including SDK 520 tests
and two doctests on each platform; Linux root worker Python passed all 738 tests.
Windows passed SDK 518 tests and two doctests, but its library job failed in an
unchanged gateway relay test while waiting for upstream EOF after Revoked. The
failure remains historical evidence. No deterministic cause, source fix or
old-head retry is claimed. Review and all mandatory GitHub checks remain required
on the forthcoming fixed head.

The window used a **USD 1 cap**, a 20-minute cleanup deadline and a 30-minute
hard bound. The cap is not measured spend or an invoice. This report makes no
remaining-budget certification.

This final check qualifies original-client SDK management only. It does not
qualify public exposure, Gate traffic, provider source preservation, RunPod,
Funnel or Native approval. This report update changes no Rust, configuration,
embedded guidance, fixture input or executed artifact.
