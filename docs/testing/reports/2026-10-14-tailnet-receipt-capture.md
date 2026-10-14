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

## New local results

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

## Remaining final gate and limits

A separate final-head LIVE run is required before merge. The first run does not
replace it. GitHub checks and the independent review must cover the final head.

This result qualifies Linux CPU enrollment receipt capture and stateful
stop/resume. It does not qualify GPU, RunPod storage, Docker-host enrollment or
production automatic node deletion. Docker transport binding and provider-ID
roundtrip are regression-tested; their live qualification is separate.
No visible UI behavior changed: the two UI edits only initialize existing test
fixtures with the new deployment field. No UI GIF applies to this PR.
