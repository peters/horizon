---
procedure: tailnet-node-cleanup
candidate_commit: 406397fa98599e4691a25f1ff66a4e71e6237980
candidate_tree: e5f9cef0603915c9efeda4277fe175054f56fee7
candidate_sha256: 1988999a808babac52db2c43cbfdbb0479eb8b97d39ba6e55988d372b1b2a8d4
date: 2026-10-15
lanes: [local-linux, isolated-native, hetzner-cpu]
issue: https://github.com/peters/horizon/issues/1335
---

# Default tailnet cleanup activation on merged Backend, 2026-10-15

## 1. Candidate and outcome

The current activation candidate passed its full applicable local matrix, isolated Native C01–C04/C07 workflow and real-input C03 through the normal default CLI. Both runtime lanes completed cleanup. This source is based on the actual merged Backend commit above; earlier prospective results retain their original identities.

The tested staged tree is `e5f9cef0603915c9efeda4277fe175054f56fee7`. All 1,795 recorded compiler/configuration/asset inputs and the additional remote-device schema fixture matched. The normal UI SHA256 is in the header; normal default CLI SHA256 is `9c79ff9cf0ea1e49845c68404563f94e92f3046011f07738456bc5cef0b113c2`. Both artifacts use Core `cloud-workspaces,default,tailnet-node-cleanup`, with `profile.test=false` and no explicit cleanup-feature override.

Default cloud-workspace builds now use the shared exact-receipt cleanup controller. Pending cards keep the exact cleanup reason and offer owner Retry. Normal removal retains unconfirmed state. Explicit owner discard hides the card and its retry controls while preserving the original record. Confirmed cards permit normal removal without an API credential. Existing asynchronous Store custody, cancellation, context/generation and active-watch checks remain authoritative. No public MCP cloud-delete grant is added.

## 2. Local matrix: new results

| Check | Result |
|---|---|
| Default Core | 2,138 passed; 21 intentional ignores; all 28 cleanup cases executed |
| Default UI | 2,276 passed; 2 intentional ignores |
| Focused cleanup UI | 6 passed |
| Standalone UI speech | 2,322 passed; 2 intentional ignores |
| UI integration assets | Font 3 and home-isolation 2 passed in each default/speech tier: 10 total |
| App host / runtime | 105 / 48 passed |
| Affected integration/native targets | Complete target runs passed; declared ignored provider/timing targets stayed ignored |
| Normal examples | Compiled with normal feature/profile accounting; not runtime-tested |
| Workspace doctests | 22 targets completed; two actual doctests passed |
| Required checks | Default-activation blocking/strict Clippy, formatting, maintainability, skill coverage and diff checks passed |

Actual dependency feature/profile proofs cover default libraries, UI, integrations and normal examples. The doctest accounting explicitly records two host TOML dependencies with different debug settings rather than claiming every profile identical. Fresh results and the verified reuse below cover the full applicable local matrix.

## 3. Reused and non-applicable results

Twenty complete unaffected package closures retain their original passing results after exact source, fixture, manifest/configuration, dependency, toolchain, environment and actual feature/profile comparisons. These include SDK 471 passes with one intentional ignore, protocol 35 passes and worker 210 passes with three intentional ignores. Ten Node API units are included within the SDK count, not additional tests. The external remote-device schema fixture was compared explicitly.

The device/browser normal artifacts retain exact 39/264 selected-input proofs. The older native helper has a separate, narrower API/schema/Store/native-phase compatibility proof; it is independent equipment, not a build of the whole current SDK. No ignored live Docker/kernel test was counted as executed. An additional explicit cleanup-feature override was not used to substitute for the actual default edge.

Advisory Clippy exited 101 with the same two inherited `struct_excessive_bools` findings in `Description` and `GroupState`, plus the inherited E0602 unknown `clippy::assert_is_empty` allowance warning. No introduced or worsened advisory finding was observed. This is not a pedantic-clean result.

## 4. Current isolated Native results

The actual running UI and ordinary browser-MCP child matched the frozen current UI hash. The public Device viewer displayed advancing frames before input. The fixture was synthetic and network-isolated; its records supplied no real provider or node authority.

| Native case | Observed result |
|---|---|
| C01 | MissingAPI Retry retained the pending card, precise manual-recovery reason and original receipt. |
| C02 | Ordinary pending removal was blocked; the explicit warning and Cancel retained the card. |
| C07 | The actual candidate window was made smaller, Fit ran while small, close-modal controls were readable, Cancel retained both cards, and restored-window Fit passed before final removal. |
| C03 | Explicit owner discard hid only the pending card; original deployment and selection bytes still matched. |
| C04 | Confirmed-card normal removal needed no API binding; both original records and selections still matched. |
| C05 / C06 | C05 was not observed natively. C06 has automated coverage only. |

The ordinary public observer recorded actual own-workspace cloud-list transitions 2 → 1 → 0. These are scoped public responses, not a desktop-global inference.

Four finalized public recordings were copied and fully decoded: 484/278/182/406 frames, matching their stopped recording records; every encoder_failed flag was false. The complete chronological GIF uses all four ranges at 3× speed, 6 fps and 1000×625: 52 seconds, 312 frames, 3,551,969 bytes, SHA256 `2ab00b94f738622d45a9c31ffd16fe3a96ad451d97b0a8a8405135561d018425`. Full decoding and visual review of the contact sheet and smaller-window modal passed. Earlier GIFs supplied no frames.

Native exited normally with code 0. Its service was empty, owned children/collectors stopped and target expired. The viewer closed, original relay exited 0, and the exact owned container was absent on the same healthy daemon. Record/selection byte preservation was proved **before** fixture cleanup; cleanup intentionally removed its owned synthetic files. No post-cleanup file-existence claim is made.

## 5. Current real-input C03 through the default CLI

One approved Hetzner CPU worker reached Ready in 177.0055 seconds, with initial exit 0 and no replacement or recovery deployment. The provider supplied an 80 GB fixed root disk; the requested container disk and managed workspace volume were each 20 GB. This is not a claim that the actual root disk was 20 GB. Worker isolation/marker and captured original identity checks passed.

| Step | Observed result |
|---|---|
| Revoke enrollment Auth | The task key was revoked before the first Core delete. |
| Delete with API Missing | The normal default CLI returned exit 1 after 13.0296 seconds; the original exact receipt stayed unchanged and unconfirmed. |
| Independent observations | All three original provider resources returned HTTP 404 and matching operation collections were empty; the same original captured node remained present. |
| Bind Personal and retry | The identical Core command returned exit 0 in 0.11159 seconds; confirmation of the exact original group was durable. |
| Independent node preservation | Full inventory contained only the two original devices, with no task node; the two original protected node/API ID pairs matched. |
| Remove API and repeat | API status was Missing. The same command returned silent exit 0 with unchanged original deployment bytes. |
| Final cleanup | Provider resources and task node were absent, task API alias removed, task key revoked/invalid, task Auth/catalog cleanup completed last, and original Auth remained accessible. |

The fixture adapters supplied credentials or independent observations; they did not replace Core deletion with direct SDK deletion. Post-confirmation node absence used full inventory, not an asserted exact-node GET 404. No paid task resource, node or credential alias remained active.

A plan-output parser and empty observation-directory check needed correction before their gated actions. Original operation identity was retained; no resource action was repeated to change the result. The private lane's historical directory name does not identify the tested source: its actual source/artifact proof binds the current tree and normal CLI above.

## 6. Limits and publication identity

This qualifies default-enabled Core cleanup with Personal access/missing-access retry and the declared synthetic UI flows. It does not qualify full enrolled Docker cleanup, RunPod provider cleanup, Core OAuth/network-failure lanes, M1 sharing or M2 exposure. Earlier prospective qualification, failures and recordings remain historical and unchanged.

Only this report and its index row were added after qualification; all compiler inputs and the extra schema were compared before and after. Exact published-head input/Git-consumer comparison, GitHub checks, independent review and final merge permission remain separate gates. This report does not claim a future head passed those gates.

Procedure: [tailnet node cleanup](../procedures/tailnet-node-cleanup.md).

## 7. Review-fix qualification

Sections 1–6 retain the first review's tested inputs. The following results cover the fixes made after that review. The checked parent is `bc5567c2cf9b74c046ae97775400a3c30d83555c`, with the recorded uncommitted source changes. The new normal UI SHA256 is `8b79c4484884be581b50c8683d30d0e33d9833bc6ef49c6d203bab963f2034bf`. Its 2,194-entry input inventory SHA256 is `20c8c50659e89f77ec4ccdab257d82152f4c8926e3625981c4db5c976a21ccdd`.

Cleanup Retry now reads settings on its background worker. The busy receiver is installed before dispatch. Cancellation is checked before the read and before cleanup. The regression holds the settings read open, renders another UI frame, refuses a second operation and verifies malformed-settings and cancellation failures without changing the deployment. Existing live-worker Delete preflight and watch behavior remain covered by their unchanged tests. A restored legacy card's raw record reason now supplies the discard warning even when the rendered reason has a state-unknown prefix. The direct-restore regression selects Cancel and verifies the original record and selection bytes.

| New qualification | Result |
|---|---|
| Default Core | 2,138 passed; 21 intentional ignores; related integration targets passed |
| Default UI | 2,278 passed; 2 intentional ignores; font 3 and home-isolation 2 passed |
| UI speech | 2,324 passed; 2 intentional ignores; font 3 and home-isolation 2 passed |
| Worker | 210 passed; 3 intentional ignores |
| Remaining workspace targets | Standard package, integration and doctest targets passed |
| Required checks | Blocking and strict Clippy, formatting, maintainability, skill coverage and diff checks passed |

The workspace result uses complete target groups. Initial whole-workspace commands stopped on environment failures. A private test directory fixed an unsafe ancestor path. Isolated runs supplied a clean temporary directory, root-owned SSH configuration and the original init reaper. They used the exact frozen test binaries and source, with read-only runtime libraries, private home/state and no host credentials or Docker socket. Failed attempts remain recorded; they are not passing whole-command results. Only unaffected non-UI results from this same recovery checkout were reused after input comparison. No lost temporary test logs were reused.

The required workspace advisory command exited 101 on the same two inherited boolean-struct findings. A supplementary UI check exposed eleven more inherited boolean-struct findings. All thirteen diagnostic files match the checked parent. The inherited unknown-lint diagnostic also remains. No changed UI source had a new actionable advisory finding. This is not a pedantic-clean result.

The fresh synthetic Native run used the new normal UI, Core `cloud-workspaces,default,tailnet-node-cleanup`, SDK features `[]` and `profile.test=false`. The running UI and its ordinary public browser-MCP child matched the new hash. Three public viewer inspections showed displayed, advancing frames before input. Direct Close on a restored legacy card showed the warning before any Retry. Cancel kept the card and both saved files; explicit discard hid it. Missing-API Retry retained the exact receipt and manual reason. The actual window changed from 1480×900 to 1100×700; Fit, the readable close modal, Cancel, window restore and Fit passed. Pending explicit discard and confirmed normal removal then passed. Ordinary public cloud-list replies showed 3 → 2 → 1 → 0. All three original deployment/selection pairs matched before fixture cleanup.

Four current-run public recordings were finalized and copied before viewer close. All four fully decoded with frame counts 525/946/437/383, matching public recording status; every encoder-failed flag was false. The two-pass GIF uses all four full ranges in order at 4× speed, 6 fps and 1000×625: 66.17 seconds, 397 frames and 5,449,692 bytes, SHA256 `8aeeb0f23a90922c8abc6c10f38a06e67630257d6ad597ed303c95574a82c0a2`. Its full decode, contact sheet and small-window modal review passed. The application exited normally with code 0; its synthetic credential service was empty, children and collectors stopped, and target expired. The exact owned container was absent on the same healthy daemon. The viewer closed and the original relay exited 0. The added synthetic legacy files were removed only after their preservation proof and explicit discard; the existing fixture then cleaned its own catalog and records.

Two equipment errors supplied no feature result: an external parent Cargo configuration stopped one launch before container creation, and an invalid version-4 empty-ledger fixture was rejected by production. The latter run exited normally and completed cleanup. The final run used the actual legacy encoding, version 1 with no enrollment ledger. No production schema or controller was weakened.

The real default-CLI C03 in section 5 is reused. Its exact frozen normal CLI, full original input inventory and contemporaneous successful summary survive. All 1,275 rows of a conservative non-UI input comparison match; the four UI files and two UI-only embedded guides are outside the CLI dependency closure. No new paid run was required. Raw temporary HTTP/ledger files were lost in the reboot; they were not reconstructed. This reuse proves the unchanged CLI behavior tested in section 5, not a new observation of provider or account state.

Final report prose was added after qualification. Compiler inputs, the external schema fixture and runtime behavior remain bound to their recorded evidence. Exact-head GitHub checks and independent review remain separate merge gates. M1 and M2 remain outside these activation results.
