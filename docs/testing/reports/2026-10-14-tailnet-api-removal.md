---
procedure: tailnet-api-removal
candidate_base: c5e513508ca03f31d4d80677ff13ccca4094a501
candidate_runtime_tree: ff1c7e11dfaf80404cf1bdc3d337b53e3f372592
qualification_scope: first-old-base-and-final-current-main
candidate_sha256: 8746a8c685213be1d8917d9d0a403e42e962deed125dae15e0bb41df6410ed24
date: 2026-10-14
lanes: [linux-isolated-synthetic]
issue: https://github.com/peters/horizon/issues/1335
---

# Tailnet API credential removal test report, 2026-10-14

## 1. Summary

The [removal procedure](../procedures/tailnet-api-removal.md) passed the first Linux run and the final current-main run.
Native owner approval removed only the requested API credential.
Actual CLI plans and normal AGENT public MCP responses agreed.
Saved networks and all three enrollment-key bindings and contents survived.
Neither run used real user credentials, provider allocations or remote API requests.

The first run used Capture base `8a41` and 1,716 inputs.
The final run used current main `c5e51350` plus the staged Remove tree above, with 1,726 hydrated inputs.
The final executable was built from those uncommitted inputs, not a later clean commit.

## 2. Final native results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| R01 | pass | CLI Remove for personal-token B returned pending_owner. Original-caller MCP observed pending, then applied after native approval. Another caller's status/cancel requests were refused. | — |
| R02 | pass | Cancellation and native Decline each preserved stored OAuth C. Independent native removal then made C missing. Five malformed cases were refused through actual CLI and MCP. | — |
| R03 | pass | Explicit Refresh with Settings open produced matching outage and recovery maps. A later Settings-closed CLI/MCP pair also matched. | — |
| R04 | pass | The owned native candidate window shrank and returned to its original size, followed by workspace Fit. Public viewer frames showed the real controls. | — |

The outage and recovery CLI/MCP maps matched exactly.
Their observation skews were 0.920043 and 0.140132 seconds.
After Settings closed, the maps still matched; the observation skew was 1.657921 seconds.
These are observation-time differences, not latency or concurrency guarantees.

Trusted private-bus checks compared exact enrollment service/username slots, item identities, catalog bindings and content hashes before and after removal and recovery.
All three bindings and values matched through final cleanup preparation.
Ordinary reads added the standard `xdg:schema` annotation; this did not change the enrollment bindings or contents.
The service owner was pinned again after restart.
These checks proved enrollment preservation; they did not substitute for public MCP results.

## 3. Local validation and input reuse

Fresh tests covered the eight affected package closures, including unit, integration and documentation tests.
Sixteen unchanged package closures reused passing results after individual dependency and consumed-input comparisons.
Together, these results covered the applicable workspace tests; no old whole-workspace job was reused without that comparison.
The actual CLI integration and explicit example arity tests passed on current inputs.

Fresh full UI speech, blocking Clippy, strict Clippy and formatting passed.
Maintainability, skill coverage and diff checks passed.
All four current production builds passed.
The qualification record contains the new commands, logs and sixteen reuse proofs.

Advisory pedantic Clippy exited 101 on two inherited `struct_excessive_bools` findings in `horizon-cloud::Description` and `horizon-browser::GroupState`.
Capacity-stopped attempts stayed recorded and did not count as passes.
No product failure remained in the executed scenarios.
Stored kind did not prove remote validity, scope or expiry.
Set/replacement, automatic node cleanup and non-Linux native behavior remain outside this qualification.

## 4. First-run differences

The first run declined a new request for already-missing B.
The final run declined stored OAuth C and proved that its stored state survived.
Both runs separately cancelled a request for C before its independent native removal.

An earlier first-run Refresh input missed after the error layout changed.
That equipment attempt did not qualify recovery and was not a product failure.
The preserved first-run evidence and final current-main evidence remain separate.

## 5. Cleanup

The final native candidate exited normally with code zero.
The owned synthetic service was empty; collectors and children stopped, and the native target expired.
The public viewer closed and the owned relay exited zero.
The healthy original Docker daemon confirmed exact container cleanup; the ownership record reports cleanup_confirmed.
No host secret, provider resource or remote access changed.

## 6. Evidence and executable identity

Final native evidence: `/tmp/horizon-1335-evidence/api-remove-native-final-mainc5e513-9521eb06`.
It retains public RPC records, CLI results, native frames, enrollment-preservation proofs, recordings and cleanup receipts.
The final pairs are recorded in `paired-unavailable-result.json`, `paired-recovered-result.json` and `paired-final-settings-closed.json`.

Current local results: `/tmp/horizon-1335-evidence/api-credential-remove-mainc5e513/qualification/qualification.json`.
The 1,726-input manifest digest is `69ac865ec37b04c03bbaa85c06b2420f8a40ddf1e72f111f74f110d34947d936`.
The four frozen binaries and flags are in `qualification/frozen-remove-currentmain-c5e513508ca0-6ae4678c/build.json`.
The metadata records current main plus staged Remove inputs; it does not relabel them as a clean committed build.

Native setup used current-SDK helper SHA256 `76bf0fd66002d5becca0f397601b893baf85a40881012410d4383630ba68586a`.
Only the three reviewed `cfg(test)` fixture leaves differed from the public SDK.
Its compile-only result and exact overlay comparison remain separate from the executed native evidence.
The older public F05 and native helpers were not reused as current-SDK binaries.

First-run native evidence remains in `/tmp/horizon-1335-evidence/api-remove-native-current-8a41-e196b55f`.
Its matrix and input records remain under `/tmp/horizon-1335-evidence/api-credential-remove-capture8a41/qualification`.
The [first-run GIF and evidence](https://github.com/peters/horizon/issues/1335#issuecomment-6177600086) remain historical evidence for that source.
Final run and cleanup results are indexed by `root-native-qualification.json`.
The inspected final GIF uses 22 seconds starting at 187 seconds, at 6 fps and 1,000-pixel width.
Its size is 707,691 bytes; SHA256 is `d34061c64c2cf381c6da7b45e0fc246925ba395e086a9da6afe0ef853a9f0826`.
The PR body contains the final GIF attachment.
No GIF is committed.
