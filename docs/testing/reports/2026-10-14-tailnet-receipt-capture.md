# Tailnet enrollment receipt qualification — 2026-10-14

Issue: #1335. Procedures: [ledger](../procedures/tailnet-enrollment-ledger.md)
and [capture](../procedures/tailnet-receipt-capture.md).

## Candidate and inputs

The first real-input candidate is `46f1cfe3526463449bff8eab7a07b09a28142f0e`,
based on `d1a675a64b24b013306e97e2d70b4c1046489fe8`.
Its tree is `6f3aa48151434d91b4c55d7299368d7def77d2d6`.
All 1,702 consumed source/build inputs match the qualified builds;
their inventory SHA-256 is
`022f11bb0af393ad66e6b1da311d6564a9877f9700edfabc075e141d16e0dec9`.

The builds ran on `e5be1a4c4f5b5f2a0a85b4d51e49381021544894` with three
uncommitted integration files. Those exact bytes were committed as `46f1cfe3`.
The original build manifests retain that build identity. The source comparison
does not relabel a binary as rebuilt after the commits.
The later report-only commit changes no executable inputs.

Linux x86-64 qualification used Rust 1.98.1, the locked dependencies, two build
jobs, no incremental compilation and line-table debug information. Rust tests
used `RUSTFLAGS="-D warnings"`. Runtime tests used an isolated home and a
deliberately absent Secret Service. Builds and logs are retained under
`/tmp/horizon-1335-evidence/captured-id-validation/newbase-e5be1a4c/`.

The actual controller binary has SHA-256
`110a8f51f617407d3daecf6d7fe739b8ac93edfd4cd87a8360fa45c16e41e62d`.
The bounded Store/probe adapter has SHA-256
`21a9d4d6ddb053e0ed3457ac5f4a7affddabb44b3dc7b18af68880637ed5b660`.
It links the candidate's public core APIs; it does not edit worker or deployment
state to manufacture a receipt.

## First candidate local results

- Formatting, maintainability, MCP skill coverage and whitespace checks passed.
- Workspace tests and the separate `horizon-ui --features speech` tests passed.
- Blocking Clippy with `speech,trace-profiling` passed.
- Strict library/binary/example Clippy with `speech`, `unwrap_used` and
  `expect_used` denied passed.
- Forty-five focused tailnet/capture/migration tests passed; one existing test
  was ignored. A separate worker filter selected zero tests and is not evidence.
- Five fresh root-projection fixture tests passed in a network-disabled,
  read-only container on the current projection inputs. They use synthetic
  daemon responses and do not qualify real enrollment.
- The advisory pedantic command returned 101 for existing unrelated warnings.
  A diagnostic run with warning caps found no warnings in changed capture files;
  that diagnostic run is not an advisory-tier pass. The existing Cargo lint
  `clippy::assert_is_empty` is unknown on this toolchain.

The old-base Rust results were not reused after the DockerHost and working-copy
changes reached main. The full required matrix above ran on the rebased inputs.
Results may be reused after the commits only because the recorded consumed
inputs match, including the three integration fixes and unchanged dependencies.

## First real-input result

LIVE passed on 2026-10-14 using one authorized Hetzner `cpx22` CPU worker in
`fsn1`, with 2 vCPU, 4 GB memory and a 20 GB retained workspace volume.
Its actual reported running rate was EUR 0.0312/hour. The run had a USD 3 cap,
one paid worker at a time, within the authorized USD 200 task budget.
Prices and timing are observations, not an invoice.

The controller used its current immutable quick-start index,
`ghcr.io/peters/horizon-worker-base@sha256:f28a0ceb76b559b299000b440a26951bc70a820003d3911c4e130174bef944ec`.
[Worker publication run 38540226457](https://github.com/peters/horizon/actions/runs/38540226457)
published that index from `6fdb49311a18ea2d0d9409b5b1afaf0008d16301`.
Both architecture reports match all 18 current contract markers, including
`horizon-tailnet-contract=4`. Local anonymous registry inspection returned
HTTP 403; no local raw-index verification is claimed. The real provider pull
and worker startup succeeded with that digest.

The test used the authorized dedicated tailnet, a separate one-hour tagged,
preauthorized, reusable, non-ephemeral auth key, a synthetic repository and an
isolated controller home. The original saved auth key was not used. API
credentials remained on the desktop. The protected baseline contained two
existing device IDs.

1. The worker reached Ready in 102.876 seconds. The receipt read through the
   public core Store matched the actual worker `Self.ID`, selected saved-tailnet
   ID, provider, worker ID and allocation request. The admin API independently
   mapped that exact `Self.ID` to the owned node and its sole worker tag.
2. The actual worker reported UID 10001 for agents, hidden root tailnet state,
   a root-owned mode-0600 isolation marker, the GitHub service and marker 4.
3. The task key was revoked and confirmed invalid. Its exact OS credential item
   was removed without reading its value. Catalog and selection hashes remained
   unchanged. This was scoped test administration, not a new product operation.
4. Stop removed the initial server. An independent exact GET returned 404;
   the same owned volume remained unattached. Resume/reconnect reached Ready
   in 116.801 seconds on a different server, with the OS key still absent.
5. Actual `Self.ID` stayed the same. The Store retained both complete,
   worker-bound receipts and no unresolved attempt. Deduplication did not discard
   the second worker receipt merely because its device ID matched.
6. Product Delete completed. Independent exact GETs returned 404 for both
   servers, the volume and provider SSH key. All three operation-filtered
   collections were empty. The deployment retained both enrollment receipts.
7. Separately authorized admin cleanup deleted only the proven task node and
   confirmed its exact GET returned 404. The task catalog/OS binding was removed.
   The tailnet returned to the same two protected device IDs, and the original
   saved auth key remained accessible with a valid format.

Private first-run records are retained under
`/tmp/horizon-1335-smoke/private/capture-first-46f1cfe35264-45b250d3/`.
They contain nonsecret projected identity, provider absence and cleanup evidence;
the key exchange used private pipes and OS storage.

## Earlier final real-input result and first review fix

The first review found that migration rejected an unrelated neighboring version-4
deployment. The neighboring reader now accepts only the validated version-4 shape
and preserves duplicate-worker fencing. Three new regressions cover unrelated
receipts, the same worker and malformed/future version shapes. The migration
suite passed 22 tests; capture passed 26 with one existing ignored test; paired
ledger passed 19. These are named suites, not an additive unique-test count.
The full current workspace, UI speech, blocking/strict Clippy and static checks
passed again. The ordinary advisory command still reports pre-existing findings.

Final LIVE passed on runtime commit
`4f6d83d885ce7b5904345400c340339c754a7724`, tree
`121fe0c4eea5caf9bd19be8c3bb11eca6ee28d00`, on the same `d1a675a6` base.
All 1,703 consumed inputs match the fresh builds made on `0642626a` with the
three migration-fix paths staged. The original manifests retain that actual
dirty build identity. The controller SHA-256 is
`4922e2abc0942c0ca768bd1eb98b314319582bd85103aa098b47f4e5e80b19a7`.
The current Store/probe adapter SHA-256 is
`7abf3101f623ea59ed7da0e520663b1919e229cb8c4fe3dbeaabd8979106584a`.
No old controller or old public-core adapter qualified this final run.

The separate final lane again used one authorized Hetzner cpx22 in fsn1,
2 vCPU, 4 GB memory, 20 GB volume and the same immutable f28a0ceb worker image.
The fresh offer listed EUR 0.0312/hour, within the same USD 3 run cap and USD 200
task budget. One paid worker ran at a time. The original two protected tailnet
devices were verified before allocation.

1. Initial Ready took 117.910 seconds. The actual Self ID and projected receipt
   matched the saved tailnet, provider, worker and allocation request. Independent
   API lookup verified the exact owned node and sole worker tag.
2. The separate tagged test key was revoked and invalid. Its exact OS value was
   removed without reading it, while catalog and selection hashes stayed equal.
3. Stop completed in 8.890 seconds. Exact provider GET confirmed the initial
   server absent and its original volume retained unattached.
4. Resume/reconnect reached Ready in 127.031 seconds on a different server.
   The OS key value remained absent after resume. Actual Self ID stayed equal;
   both distinct worker-bound receipts survived with no unresolved attempt.
5. Product Delete completed. Exact GETs returned 404 for both servers, volume
   and SSH key; all operation-filtered collections were empty. The Deleted
   deployment retained both receipts.
6. Separate authorized administration deleted only the proven task node after
   provider absence and confirmed its GET returned 404. The task catalog/OS
   binding was removed. The original two protected device IDs and original
   accessible auth-key binding remained intact.

Private final records are under
`/tmp/horizon-1335-smoke/private/capture-final-0642626a-8d1e7e7e/`.
The directory/cloud name predates the fix; its candidate and build manifests
record the actual final source and binary identities above. Fresh production
and adapter manifests, checks and full input inventories are under
`/tmp/horizon-1335-evidence/captured-id-validation/review-round1-0642626a/`.
This later report-only update changes no consumed runtime/build input.
These runs are historical evidence after the later base update and second
review fix. They do not qualify changed controller inputs by themselves.

## Current main qualification and second review fix

The second review found that decoding a neighboring record through a JSON value
collapsed duplicate fields. The version probe and supported deployment reader now
decode directly from the original bytes. The regression refuses duplicate version,
ledger, worker and device fields, preserving both deployment files.

The current runtime commit is `d3ea370e495a6e70cebd8cfe851fd8c63322243d`, tree
`af7c4b5e6749aa278b31a0361502203df222af67`, based on
`2eb32c12f48e13985e2315de91ec14fb327d3f3a`. The base includes merged API status,
development-image helpers and current browser changes. All 1,711 consumed inputs
match the fresh builds on `88cbd3b2` with the two migration fixes staged. Their
inventory SHA-256 is `03bcb350c58baf1f2a1cf6e76dfa0caed79fd9bf31e18293dbfe5fbda33ab239`.
Original manifests retain the actual dirty build identity.

Fresh workspace tests, UI speech tests, formatting, maintainability, skill coverage,
blocking Clippy and strict Clippy passed. Neighbor tests passed 4, migration 23,
the tailnet/capture filter 27 with one existing ignored test, and paired ledger 19.
These overlapping suites are not summed as unique tests. The worker filter selected
zero tests and is not evidence. Ordinary advisory Clippy returned 101 for inherited
findings. The five offline projection tests are reused only after comparing the
exact script, test, immutable fixture image, arguments, environment and mounts.

All six production/adaptor builds passed. The first adaptor attempt hit the local
per-user disk quota; it made no code change. Its log and truncated result remain
preserved. A retry passed after completed, regenerable test outputs were retired
under process, file-descriptor, mount and ownership checks. This is build-equipment
evidence, not a product failure or an advisory-tier pass.

The new controller SHA-256 is
`73579fe40665218b750345c69f4e5463f375f2a5f3d2821956e4551136c45d1c`.
The new public-core Store/probe adaptor SHA-256 is
`f4ee1af1961147184bb7c2465102f92770f9654d8cb20188d4dce6f5b07cf24b`.
Both execution bytes changed, so the previous LIVE result was not reused.

Fresh final LIVE passed with the same authorized cpx22/fsn1 CPU shape, 20 GB
volume, immutable f28a0ceb worker image, EUR 0.0312/hour listing, USD 3 run cap,
USD 200 task budget and one paid worker at a time.

1. Initial Ready took 119.247 seconds. The public Store receipt, actual worker
   Self.ID and independently observed API node matched. Agent UID, private state,
   root isolation marker, GitHub service and marker 4 checks passed.
2. The task key was revoked and invalid. Its exact OS item was removed without
   reading its value; catalog and selection hashes remained unchanged.
3. Stop took 8.843 seconds. The initial server returned 404. The provider briefly
   reported its old volume attachment; a later read confirmed the volume unattached.
   Resume began only after that independent confirmation.
4. CLI Resume recorded intent in 0.004 seconds. Reconnect reached Ready in
   135.040 seconds on a replacement server. Self.ID stayed equal, both distinct
   worker-bound receipts survived and no unresolved attempt remained. The OS key
   value remained absent through the same Secret Service owner after Ready.
5. Product Delete took 13.043 seconds. Exact GETs returned 404 for both servers,
   volume and provider SSH key; all three task-filtered collections were empty.
   The Deleted record retained both receipts.
6. Separate authorized administration then deleted the exact proven task node
   and confirmed 404. The task catalog/OS binding was removed. The tailnet returned
   to its same two protected devices and one owner; the original auth-key binding
   remained accessible with a valid format.

Current local manifests and logs are under
`/tmp/horizon-1335-evidence/captured-id-validation/status-main-2eb32c12/`.
Current final LIVE records are under
`/tmp/horizon-1335-smoke/private/capture-final-main2eb-c2ab6ca2/`.
The testing index and immediate hazardous-step cautions were corrected. The LIVE
procedure distinguishes CLI Resume intent from reconnect allocation and Ready.
This report and procedure update changes no consumed build/runtime input.
The third review corrected the written key-removal order: revocation and exact
OS-value removal now precede Stop, with catalog/selection retained; the procedure
rechecks absence after resumed Ready. The current final LIVE above already ran
that sequence. Its recorded key, OS-item, catalog and selection proofs remain
valid; no new execution is claimed for this documentation correction.
Exact-head GitHub checks and independent review still gate merging.

## Limits

This result qualifies Linux CPU enrollment receipt capture and stateful
stop/resume. It does not qualify GPU, RunPod storage, Docker-host enrollment or
production automatic node deletion. Docker transport binding and provider-ID
roundtrip are regression-tested; their live qualification is separate.
No visible UI behavior changed: the two UI edits only initialize existing test
fixtures with the new deployment field. No UI GIF applies to this PR.
