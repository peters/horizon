---
procedure: tailnet-api-entry
candidate_base: 69a93f23b7acc33a0fa512b6a2fa10c6fe86519d
candidate_runtime_tree: ef2a77135888f6fd1806e588e782f7dd455708dd
candidate_sha256: ca9ec4273ec666dd7b0995ad856b025be9400b4159ca6d727d9f153db20453cc
date: 2026-10-15
lanes: [linux-isolated-synthetic]
issue: https://github.com/peters/horizon/issues/1335
---

# Saved tailnet API credential entry — isolated native test

This report covers the Linux native candidate based on main `69a93f23b7acc33a0fa512b6a2fa10c6fe86519d`, candidate tree `ef2a77135888f6fd1806e588e782f7dd455708dd`. The frozen Horizon executable SHA-256 is `ca9ec4273ec666dd7b0995ad856b025be9400b4159ca6d727d9f153db20453cc`. This is native fixture evidence for credential entry, not evidence of remote credential validity or cloud/provider operations. Three synthetic saved tailnets are identified below as A, B and C. Public operations expose metadata only; synthetic values were entered through the native masked fields.

## Completed native scenarios

| Scenario | Actual result and evidence |
|---|---|
| E01 — enter Personal token for A | The requesting owner received PendingOwner, then Applied through both actual CLI and MCP status. While Settings remained open, the CLI catalog reported A Personal, B Personal and C OAuth, all stored/ready. The actual observer MCP catalog at 01:06:26.283890 UTC matched the CLI file captured at 01:06:24.827802 UTC. Separate private content proof confirmed A's intended synthetic Personal payload and preserved all three Auth slots. |
| E02 — replace A with OAuth | Native empty entry, masked client ID/secret entry and Save completed. Both owner interfaces reported Applied for the same request. The CLI read at 01:12:24.087353–01:12:24.390374 UTC and observer MCP reply at 01:12:26.303413 UTC agreed: A OAuth, B Personal, C OAuth, all stored/ready. The attached GIF covers this complete flow. |
| Request owner isolation | Other-owner CLI status/cancel and MCP status/cancel each returned the expected unknown-request refusal. These are expected authorization errors, not successful operations; they did not reveal or cancel the owner's request. |
| E03 — cancel, decline and unsaved close | Original-owner MCP cancellation and native decline produced Cancelled. A separately typed unsaved OAuth draft was closed, reopened with both fields empty and Save disabled, then discarded. Its owner status remained PendingOwner until explicitly cancelled; closing the editor did not silently apply the request. |
| E04 — failed write and successful retry | The test temporarily stopped the fixture credential-store service. Public catalog observations became unavailable. The attempted write reported Failed. After recovery, a successful read and Settings close/reopen retained the fixed write-error text: “The OS credential store is unavailable or locked. Unlock it and try again.” A fresh request subsequently saved B's OAuth credential and reported Applied through CLI and MCP. |
| Direct native source change | On C, a synthetic OAuth client ID was typed, source changed to Personal and back to OAuth, and both entry fields were empty with Save disabled. Discard completed; final exact private proof retained C's original OAuth payload. |
| Window size and Fit | The actual client was resized to 1220×780, workspace Fit was invoked while small, then 1480×900 was restored and Fit invoked again. This verifies the workspace Fit sequence; it does not claim Settings form readability at the small size. |

The final catalog CLI read ran at 01:18:58.124612–01:18:58.413707 UTC. The actual MCP observer reply at 01:18:58.511738 UTC matched it: the same three IDs and names, all OAuth, stored and ready. These paired observations are separate timestamped reads, not an atomic snapshot. Public proof snapshots 001–003 preserve the actual replies, sequence numbers and matching comparisons.

## Private content and native cleanup

The final private exact-payload proof confirms A's intended E02 OAuth payload, B's intended fresh E04success OAuth payload and C's original native-prepare OAuth payload. All three Auth slot identities, content hashes and catalog entries match the baseline; common item attributes were preserved. The OS service added `xdg:schema` annotations. An earlier strict-attribute comparison rejected those added annotations; that failed attempt remains preserved and is not presented as passing. The final proof is explicitly after E04success: no pre-retry B-payload preservation claim is made.

The read-only post-Set `native-validate` phase passed. Native testing ended by normal UI window close at 01:20:23 UTC. The actual native receipt reports passed, native exit 0, owned service empty, owned processes stopped and output collectors stopped. Its native-cleanup result reports passed and owned catalog empty. The evidence also records viewer closure, relay exit 0, cleanup runner exit 0 and ownership cleanup confirmation. No `native-check` destructive phase was used after Set.

## Local checks and reuse

The full applicable required local matrix passed. New coherent default library checks passed 3,035 tests with 23 ignored; default UI passed 2,221 with two ignored. Fresh speech UI passed 2,267 with two ignored, plus three font-asset and two home-isolation tests. Applicable integration, example and doctest targets passed. Fourteen independent package closures and five independent binary/integration closures retain passing results through source, dependency, feature and input comparisons. This combines new and valid reused results; it does not claim a new wholesale workspace command.

Blocking Clippy (`speech,trace-profiling`), strict Clippy (`speech`, denied unwrap/expect), formatting, maintainability, bundled-skill coverage and diff checks passed. Actual compiler-feature proofs matched 421 default library artifacts and 557 UI artifacts without mismatch. Recorded settings include rustc 1.98.1, locked/offline dependencies, two build jobs, disabled incremental compilation, line-tables-only debug and `RUSTFLAGS=-D warnings` for test tiers.

Advisory pedantic exited 101 on the inherited excessive-bool findings in provider `Description` and browser `GroupState`. The inherited unknown `clippy::assert_is_empty` configuration warning is recorded separately. No full pedantic-clean result is claimed.

## Recording and limits

`oauth-entry-save.gif` is a 20-second, 540×1020, 120-frame export of the E02 recording, covering empty OAuth fields, masked input, Save and the resulting stored OAuth state. SHA-256: `fc9117b0f9800e3615cd7bebfb82e5ee820dd9bfa40a245e33a5f960beecdf29`. Its preserved source recording is 73.676 seconds / 605 frames, SHA-256 `8c3a91c4b04257070e13e60d6b14ef1dab4be6d6f4aee34a2e63bef25397b2ab`. E01 recording reached its capture cap before Save; E01 completion is established by the actual Applied receipts and public/private checks, not a complete E01 video. The evidence also preserves videos 2 and 4; video 4 finalized at 267.588 seconds / 2215 frames without an encoder failure.

Native Undo was not tested: the test-input CLI rejected key `z` before dispatch. Initial uppercase Enter and malformed scroll requests were likewise rejected by the fixture input protocol, corrected and retained as equipment evidence; they are not product failures. Expiry, renamed/removed targets, root changes, running-request cancellation refusal and late terminal receipts are covered by qualified source regressions, not claimed as actual native scenarios here.

This report establishes desktop entry, request ownership, public status, private content and cleanup in one isolated Linux fixture. It makes no macOS/Windows, real Tailscale API, automatic node cleanup, CI or independent-review claim. The report is added after the recorded runtime build; its prose is not a compilation input. All 1,735 recorded runtime inputs and the additional schema matched before the report-only commit.
