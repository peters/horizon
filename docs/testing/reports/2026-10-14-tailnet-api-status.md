---
procedure: tailnet-api-status
candidate_base: d1a675a64b24b013306e97e2d70b4c1046489fe8
candidate_runtime_tree: 085568405f2a895ec31b8ba15cac700a62667545
candidate_sha256: 2b0342c3350661ebdc5fa95394a630c617c27579b2388c10091b6cb7727f6731
date: 2026-10-14
lanes: [linux-isolated-synthetic]
issue: https://github.com/peters/horizon/issues/1335
---

# Tailnet API credential status test report, 2026-10-14

## 1. Summary

The [status procedure](../procedures/tailnet-api-status.md) passed its automated
and native checks on Linux. Settings, the actual CLI and public MCP reported the
same synthetic credential kinds, unavailable service and changed catalog.
The run used no real credential, Tailscale request, worker or provider allocation.
The public Device viewer recorded native video. The inspected PR GIF used the final recording at 12 times its normal speed.

This change covered read-only presence and kind. It did not include an API
credential editor, remote validation, node receipts or automatic node deletion.
Stored presence did not prove remote validity or scope.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| S01 | pass | Shared adapter tests distinguished pending, unavailable and missing. Close-transition tests refused stale catalogs and another root. | — |
| S02 | pass | Both cloud bundles matched. Coverage passed for 59 tools, five servers and six skills. | — |
| S03 | pass | Settings-closed public MCP and the actual CLI reported three prepared rows. Native Settings showed missing, personal-token and OAuth-client kinds. Window resize and workspace Fit kept the settled rows readable. | — |
| S04 | pass | The owned service stopped. Explicit Refresh produced unavailable status in UI, MCP and CLI. Restore and Refresh returned the original kinds. | — |
| S05 | pass | Name/auth-key save completion reached MCP and CLI while Settings stayed open. Removal stayed absent after Refresh, close and reopen. | — |

Three public viewer inspections reported connected and displayed frames. Their
decoded frame sequences advanced from 44 to 83 to 106. The public video stopped
without an encoder error.

Public MCP preserved legacy tailnet items with only `id` and `name` fields.
The captured responses showed ready and unavailable observations. This native
run did not capture a pending observation; focused tests covered that state.
Blocked-reader tests supplied the deterministic unfinished-barrier evidence.

## 3. Defects and local validation

No defect remained in the tested status scenarios. Review found that extra CLI
arguments fell through to Settings loading. The command now checks its argument
count first. The CLI example suite and actual frozen CLI checks covered the fix.
Both skill references now state that an unknown observation does not prove
credential absence. The GIF is an uploaded attachment. A later review found a stale global
interruption message after a successful retry. The fixed completion clears that
message and retains row errors. A further review found that the parent Settings
state retained a copied error after a reader closed. Reader errors now stay in
the reader state. Recovery clears them without clearing save or catalog errors.
Six cases cover failure and disconnection with independent save/catalog errors. A deterministic regression covers failed and
disconnected readers followed by success; the native lane separately covers
service failure and recovery.

A later review found that completed per-ID storage failures disappeared through
Settings close while a replacement read was pending. The close handoff now retains
only current saved-ID failures. Fresh replies replace them; successful snapshots
and removed-ID failures are discarded. The parent cache and reader regressions
cover mixed rows, stale replies, pending retry and recovery. Native S04 also
closes and reopens Settings while the service remains unavailable.

The fifth review found that auth-key save or catalog failures masked a successful
independent API observation, and that closing a pending Settings reader could
discard an existing production-cache error. API observations now keep their own
failure state. Close retains current cache failures and overlays current editor
failures. Before the first editor catalog loads, close preserves the last known
same-root catalog; an explicitly loaded empty catalog removes those old rows.
Three new regressions and the mixed-row regression cover these transitions.
The final native run forced a real synthetic auth-key save failure: Settings and
public MCP retained cached API kinds while the separate save error remained.
A fresh CLI read during the unavailable service reported unavailable storage;
that independent read was not claimed as cached-observation parity. After
recovery, eleven contemporaneous CLI/MCP snapshots agreed across the ordinary
status, unavailable, close/reopen, rename and removal flow.

Five independent PR review rounds completed. The fifth-round fixes received
local review and the affected checks; a sixth review was not requested. The
repository requires explicit permission from peters for the fully qualified
fixed head to replace the exhausted Copilot review gate.

Before this run, the close-transition fix prevented a removed row from returning through an older
production reader. The native run used the source with that fix.

The required local matrix passed. The final fifth-round fixes received new UI
default and speech suites, focused tailnet tests, blocking and strict Clippy,
formatting and a new UI build. Other package and CLI/device build results reused
matching inputs. After the M0b base update, new default tests
covered app-host, app-runtime, Core and UI. New UI speech, full blocking Clippy,
full strict Clippy and formatting checks passed. Maintainability, skill coverage
and whitespace checks passed too.

Unchanged workspace packages reused the original full-matrix results. The input
comparison covered 1,679 consumed files, dependencies, toolchain and build flags.
The changed Core/UI inputs and embedded skill assets received new checks.
The new advisory run reported the same pre-existing excessive-bool findings in
`horizon-cloud::Description` and `horizon-browser::GroupState`.

The parity outcome required 11 source/test files and 1,466 changed source lines.
The shared observation, background state, cache accessor and CLI/MCP adapters
worked together. A separate adapter change would leave discovery incomplete or
require Settings to stay open.

## 4. Deviations from the procedure

Reviewed private equipment supplied synthetic setup and cleanup. It was not a
product prerequisite or a delivered fixture. Public MCP responses, actual CLI
output and native frames supplied the behavior evidence.

The run recorded native video. The PR GIF used the same native frames; it did
not replace public responses or CLI results. This report did not treat private
Store results as MCP proof. The native result covered Linux only. The final
`api-status-native-final-ee0806d2` run used the existing private-bus CLI control.
UI, CLI and public MCP agreed on eleven stages in one run: prepared status,
unavailable service, close/reopen while unavailable, restored service, rename,
removal, close and reopen.
Window resize and workspace Fit remained readable. An earlier run had omitted
the private bus for initial CLI calls; those results remain unqualified.
The corrected final run completed exact cleanup.

## 5. Cleanup

Cleanup removed the three exact synthetic bindings and emptied the owned store.
The native process exited with code zero. Its output collectors, owned service
and relay stopped. The exact container was absent, the native target expired
and the public viewer closed.

The run kept manifests, public responses, CLI results, screenshots and video as
private evidence. The fixture did not mount the operator's home or credential
service. Cleanup touched only the owned fixture resources.

## 6. Evidence

The first native run was `api-status-native-5f84-5ba9445f` on source `5f84f007`.
The final UI/CLI/MCP run was `api-status-native-final-ee0806d2`. Its evidence
included `public-viewer-0.json` through `public-viewer-2.json`,
`state/mcp-evidence/rpc.jsonl`, eleven matching CLI results and matching public responses,
S03–S05 screenshots, `paired-results.json`, `public-video-final.json`,
`native-status.webm` and the completed ownership record. The reviewed
[native GIF](https://github.com/user-attachments/assets/6a721238-e861-4aeb-8042-0e3b2bc9448b)
shows the final native run. The video recorded 2,595 frames and stopped at its five-minute limit without
an encoder failure. It covered initial kinds, auth-key save failure and API
service failure/close/reopen/recovery. Later rename, removal, resize and Fit
were verified by native screenshots and public responses after the recording ended.

The build manifest recorded the corrected source inputs on base `63f9a7b7`.
All 1,679 consumed file hashes matched committed runtime head `ee0806d2`.
It recorded complete consumed-input hashes and all four frozen executables.
The native ownership record matched the frozen UI, cloud CLI and device hashes.
The preparation source and artifact had separate identity evidence.

The UI used `cloud-workspaces`. The device CLI used its `cli` feature.
Builds used Rust 1.98.1, four jobs, line-table debug information and
`RUSTFLAGS="-D warnings"`. Runtime source and artifacts stayed unchanged during
the native run. This report was a later prose-only addition.


## 7. Qualification after the current-main reconciliation

The status change was reconciled onto main `d1a675a6`, preserving the current
Docker provider default and working-copy image recipe. The CLI dispatch now keeps
its argument check in the focused tailnets leaf and centralizes the usage text.
The qualified status tree was `085568405f2a895ec31b8ba15cac700a62667545`.
All 1,700 consumed source/build inputs were recorded before this prose update.

The current full workspace suite, actual CLI regressions, blocking and strict
Clippy, formatting, maintainability, skill coverage and whitespace checks passed.
Thirty focused tests passed across UI, MCP inventory, CLI, Settings-closed
bootstrap and the shared contract. UI speech reused only identical individual
inputs; the previous whole-workspace result was not reused after the CLI change.
Ordinary advisory Clippy still failed on pre-existing excessive-bool findings;
a diagnostic warning-capped run was not treated as an advisory pass.

A fresh isolated native run, `api-status-native-d1-08556840`, executed the frozen
current-main UI, CLI, device and browser artifacts. The UI SHA256 was
`2b0342c3350661ebdc5fa95394a630c617c27579b2388c10091b6cb7727f6731`.
The private preparation helper was compiled against the same current SDK with
three separately recorded test-equipment leaves. Its identity was not presented
as product source or a clean build of the old PR head.

S03–S05 passed again: initial missing/personal-token/OAuth rows, actual unavailable
service and explicit recovery, same-ID name/auth-key save, removal, Settings
close/reopen and application shrink/grow with workspace Fit. Eight settled
CLI/public `cloud_companions` snapshots matched within 2.1 seconds. `cloud_list`
remained empty. Four immediate observations taken before background completion
differed and remain preserved as transient evidence. No pending phase was claimed
from those differences. Typed private-bus evidence verified the synthetic auth
contents before and after recovery, replacement and removal; it did not expose
a credential value. The earlier independent save-failure run remains historical
evidence; this fresh run did not repeat that failure scenario.

The public viewer recorded two finalized videos: 2,303 frames over 266.63 seconds
and 1,765 frames over 203.596 seconds, without encoder errors. The inspected
[current native GIF](https://github.com/user-attachments/assets/9c29a680-5f43-4004-a730-70bf8ab04c87)
shows the actual status rows and removal. Screenshots preserve window resize and
Fit separately. Completed evidence is linked in
[the epic comment](https://github.com/peters/horizon/issues/1335#issuecomment-6174227630).

The application exited normally with code zero. The exact synthetic service was
empty, its children and output collectors stopped, the native target expired,
the viewer closed and the scoped relay stopped. The exact container was absent.
No real credential, API request, worker or paid resource was used. Source and
artifact identity, CLI/MCP responses, transient comparisons, screenshots, video
and cleanup records remain private. The five-round review exception still needs
explicit permission from peters for the newly published head after CI passes.
