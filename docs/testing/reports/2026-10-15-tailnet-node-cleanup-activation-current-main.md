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

## 8. Repeated Retry after settings preflight failure

The second review found that deferred settings failure reported Failed after the card had left Deleted. This hid pending cleanup controls. The fix restores Deleted before reporting that failure. Only the deferred cleanup Retry takes this path; live-worker preflight and connection-watch behavior stay unchanged.

The checked parent is `6624aeb371de32df39325f2878dc321b7ae39994`, with the recorded uncommitted fix, regression and guidance changes. The 2,194-entry normal-UI input inventory SHA256 is `c0080722980aef41304e245ac0162948aa03fc46b4317f6ce3b7cd5e26349b0e`. The new normal UI SHA256 is `899c5b5c8fccdd224e28f702df4f507643c14c1b60930b139701fbdc47c1a574`; its actual features are UI `cloud-workspaces,default`, Core `cloud-workspaces,default,tailnet-node-cleanup`, SDK `[]`, and `profile.test=false`.

New local checks passed: default UI 2,278/2 intentional ignores, speech UI 2,324/2, and font/home integrations 5 per UI tier. The focused regression passed on both fresh harnesses and within each full UI run. Four gated FIFO reads cover malformed settings and cancellation, each followed by a second Retry. Normal application polling clears busy state, restores Deleted, keeps the error, pending receipt and Retry, and preserves the original deployment bytes. Blocking/strict Clippy, formatting, maintainability, skill coverage and diff checks passed. Exact input comparison retains the unchanged Core, worker and remaining workspace results from section 7. Advisory retains the same inherited findings; it is not a clean pedantic result.

The fresh isolated Native run established three advancing, displayed public-viewer observations before input. With Manage open, malformed synthetic settings produced the settings error while Deleted and Retry stayed visible. Exact settings restoration followed by a second real Retry produced the missing-API reason for the saved synthetic tailnet/device, with pending status and Retry still available. All three original deployment/selection pairs and the restored settings matched their original bytes. An initial attempt opened Manage after corrupting settings; it did not exercise Retry and supplies no passing feature result.

Both finalized public recordings fully decoded: 474 and 137 frames, matching their public recording records, with no encoder failure. The two-pass GIF uses both complete ranges in chronological order at 4× speed, 6 fps and 1000×625: 18 seconds, 108 frames, 677,915 bytes, SHA256 `b9dccbe38a539db08425127638c16bede65f11cdb06a552f8dd9097b69f4bb0f`. Root checked its contact sheet and retained-error/Retry frame. It contains generic synthetic data only.

After this proof, pending and legacy cards were explicitly discarded and the confirmed card was normally removed. Public cloud-list replies showed 3 → 2 → 1 → 0. The extra legacy files were removed under their original ownership, exact-byte and lock checks. The original fixture reported normal application exit 0, an empty synthetic catalog/credential service, stopped children and an expired target. The viewer closed, the original relay exited 0, and an independent check proved the exact container absent on the same healthy daemon.

Section 7's unrelated valid-settings, legacy-close and resize scenarios retain their original tested executable. The unchanged default-CLI C03 is reused under its recorded dependency/input comparison; no new paid run or current account-state observation is claimed. Exact-head GitHub checks and Copilot review remain separate merge gates. M1 and M2 are incomplete.


## 9. Legacy and locked-record Retry failures

The correction after head `80a79bc6b5b90500a6d3aa8d5ed00e08dadcb2db` kept one display context separate from the latest error.
The locked Store remained the authority for cleanup and removal.
The checked input tree had SHA-256 `19e554fe34404896be68038a5255a3b049446487a4cbd8176cf649483f6fd24a`.
The normal UI and its MCP child both used executable SHA-256 `f50d8c8edfd0d32976257a456402019141a5e3812420cb76b0a2838d7fa81260`.
Its actual profile had `test=false`; Core included `tailnet-node-cleanup`.

New local checks passed: five focused regressions, 2,282 default UI tests, 2,328 speech UI tests and five integrations per UI tier.
Each full UI tier kept two intentional ignores.
Blocking and strict Clippy, formatting, maintainability and skill checks passed.
Captured and selected legacy records survived repeated malformed and cancelled FIFO reads through normal polling.
Held Store locks and invalid launch identities retained Deleted and Retry.
Fresh lifecycle facts cleared obsolete context; a fresh captured pending receipt replaced the legacy explanation.
The original Store still refused removal after cached display facts changed.
Unchanged non-UI tests and default-CLI C03 results were reused after exact input comparison.
Fresh advisory checks retained eleven inherited UI findings and two dependency findings; no new finding appeared.

The new Native run used public displayed-frame and recording evidence in an isolated desktop.
A real lock on the original captured record caused Busy; Deleted and Retry remained visible.
The owned lock session then released its lock and exited successfully.
Two malformed-settings retries retained the error and Retry.
Exact settings restoration permitted another Retry, which retained the original missing-API reason.
The three original deployment/selection pairs stayed byte-exact during this captured flow.

Ordinary Remove on the legacy card refused the unresolved original Store record.
It displayed the missing-device-ID explanation and made Retry available.
Two malformed-settings retries retained both the current error and the separate manual explanation.
Exact settings restoration permitted another Retry, which returned the missing-ID explanation again.
All three deployment/selection pairs stayed byte-exact throughout these affected legacy checks.

Before the legacy checks, one separate owner Redeploy attempt failed at a missing-file preflight.
That path reserialized only the legacy deployment record, from 1,648 to 2,007 bytes, with explicit serde defaults.
Its original values, Deleted stage and absent worker stayed unchanged; the other five files stayed byte-exact.
The evidence retained both byte sequences and the semantic difference.
The affected legacy checks used the resulting original record; no file restoration or fabricated event supplied their result.
This preparatory attempt was not a Retry byte-preservation pass.

The owner discarded the pending and legacy cards and removed the confirmed card normally.
Public cloud-list reads showed 3, 2, 1 and 0 cards.
The exact legacy extra files passed hash, inode and lock checks before nonrecursive fixture cleanup.
The app exited with code 0; the synthetic catalog and credential service were empty.
Owned children stopped, the CLI target expired, the public viewer closed and the original relay exited with code 0.
An independent healthy-daemon inventory proved the original container absent.
No real provider, API credential or user device changed in this Native run.

The new GIF used both complete current recording ranges in order at 4× speed.
It contained 436 frames at 6 fps, with a 1,000×625 image and a 72.67-second duration.
Its 1,833,086 bytes had SHA-256 `757c6637ca8367df6b703f38f9ddea3ac4e8c4a205e6d0e998c74d919b8afb42`.
Both source videos fully decoded, with 571 and 1,805 frames, equal to the public encoded counts.
The encoders reported no failure. Representative captured and legacy states passed visual review.
The public image contained generic synthetic data only.
The earlier GIF retained its original executable identity for resize, Close and removal evidence.
All earlier limits in this report still apply; GitHub checks and review of the next pushed head remained separate gates.

## 10. Authoritative cleanup snapshots and final real-input qualification

Sections 1–9 retain their historical executable and source identities. This section covers the final correction based on parent `27c546f16e113f5d302096dcc05bd06e1906d61c`, with its recorded source, regression and guidance changes. Core now publishes the original locked Store's durable deletion snapshot before cleanup can fail and its last saved cleanup snapshot afterward. The UI uses those authoritative facts to retain the first legacy removal explanation and pending Retry context. A later settings error or lock failure cannot turn cached display advice into removal authority.

The final tracked input inventory SHA-256 is `47212b90e8d436c45954ef6a61c592481501cb40b7e5c194ad3261e4d902a78e`. The normal products were built from input tree `8c6a54efc73030915077e42f859987f2f03d3e2cc6cc4e244a6a68b90171c3ce`: UI SHA-256 `c4dc96f164ab579704638f33bf6af993b3f2516b9e5de9221dedd973dd5f55a6`, owner CLI SHA-256 `fa5a83d3147407d75f73aaab9b48790262848c57cade2fad90b41367255dbe6e`. Both are normal `test=false` products with actual Core `cloud-workspaces,default,tailnet-node-cleanup` and SDK features `[]`. A later change to two Core test expectations is outside the actual normal Core dependency file; exact consumed-input comparison preserves these normal products and their runtime results. The fresh full Core run covers the updated test leaf.

| Final local coverage | Result |
|---|---|
| Core default library | 2,142 passed, zero failed, 21 intentional ignores |
| Default UI | 2,284 host passes plus one exact-binary isolated environment complement; two intentional ignores |
| Speech UI | 2,331 passed; two intentional ignores; font/home integrations added five passes |
| Worker | 205 host passes plus five exact-binary isolated SSH ownership complements; three intentional ignores |
| Workspace | All 85 target and documentation groups accounted for through fresh results and exact-input reuse |
| Required tiers and repository checks | Blocking/strict Clippy, formatting, maintainability, bundled skill coverage and diff checks passed |

The initial workspace command's exit 101 remains recorded. Two old Core callbacks rejected the new authoritative Snapshot event; the corrected expectations accept only that event, assert canonical durable bytes and the held original Store lock, and retain cancellation, no-provider/API and receipt invariants. The complete replacement Core run passed. The other six failures came from the host's SSH configuration ownership and repository-discovery environment; the same frozen test binaries passed those exact tests in isolated, network-disabled fixtures. The remaining successful workspace targets have matching consumed inputs. All owned test containers were independently absent afterward. No ignored live test was counted as executed.

The full advisory command retained its two inherited findings. Supplementary checks accounted for six inherited Core and eleven inherited UI findings. None was introduced or worsened, and no warning was suppressed. This is not a pedantic-clean result.

The fresh isolated Native run used the current normal UI and its ordinary MCP child, with both executable hashes verified. The public viewer displayed advancing frames before input. The first ordinary legacy Remove was refused by the original Store, displayed the missing-device-ID explanation and exposed Retry. A second ordinary Remove while that original Store was locked retained the manual explanation and Retry. After lock release, valid Retry retained the manual outcome. Repeated malformed-settings Retry on legacy and captured records retained Deleted, the current failure and Retry; exact settings restoration on the captured record returned the missing-API reason. All six original deployment/selection files remained byte-exact. No Redeploy supplied or prepared these results.

After the affected checks, the owner explicitly discarded pending/legacy cards and normally removed the confirmed card. Public cloud-list responses showed 3 → 2 → 1 → 0. The original six-file preservation proof preceded fixture cleanup. The application and original relay exited 0, the synthetic catalog and credential service were empty, children stopped, the CLI target expired and the viewer closed. An independent healthy-daemon inventory proved the exact original fixture container absent.

Both complete current public recordings decoded successfully: 1,168 and 737 frames, matching their public encoded counts, with no encoder failure. The chronological two-pass GIF uses both full ranges at 4× speed, 6 fps and 1000×625: 60.17 seconds, 361 frames, 2,595,370 bytes, SHA-256 `438d4c2106200797584685dbcab310cb75614d9194ff0d2ace5aea2f1ff0453b`. Its contact sheet and affected states passed visual review. It contains generic synthetic data only. [Current Native GIF](https://github.com/user-attachments/assets/b897e9a0-06cf-4438-bf02-c0dd3f9b81da). Earlier resize and Close recordings retain their own executable identities.

A new real C03 run used the current normal default CLI above, rather than reusing the earlier CLI result. One approved Hetzner `cpx22` CPU worker in `fsn1`, with the pinned A3 worker image and a 10 GB managed workspace volume, reached Ready with exit 0 in 275.87 seconds. Its unprivileged agent account UID was 10001, marker was 4, sole Self tag was `tag:horizon-worker`, and its captured receipt matched the original provider, worker, allocation request and saved tailnet. Local Network Bridge remained off. The offer quoted EUR 0.0312 per hour; a conservative USD 1 reserve was below the USD 3 run cap. These are a recorded offer and reserve, not a final billing statement.

| Final real C03 step | Observed result |
|---|---|
| Initial preflight | A Docker-config file/directory mistake failed at Validate/Prepared before any provider allocation. The task Docker config was corrected to a directory containing `config.json`; Root replanned and retained the original cloud ID. |
| Original API status | The original catalog's lock write was unavailable on the managed filesystem. A task-owned nonsecret catalog copy, with no API binding or pending journal, let the current public SDK status resolver confirm the original legacy Personal API slot without migrating or changing it. |
| Missing-API delete | After shared task Auth revocation, ordinary Core delete returned exit 1 only after provider deletion. The original exact receipt remained unconfirmed, and independent inventory still found its node. |
| Provider absence | Exact original server and workspace-volume GETs returned 404. Paginated SSH-key inventory contained neither the original task public key nor its operation identity. The task registration ID was not independently captured, so no exact-key-ID 404 is claimed. No matching operation resource remained. |
| Exact retry | A private metadata pointer mapped only the task saved ID to the existing legacy Personal API slot. The identical original Core delete command returned exit 0 and durably confirmed the captured receipt. No API secret was copied and no borrowed OS slot was saved or deleted. |
| Idempotence | Restoring the private API entry to Disabled and repeating identical delete returned silent exit 0. Confirmed original deployment bytes stayed exact. |
| Final task cleanup | Independent inventory confirmed the C03 node absent. Both original kernel guest nodes and exact containers were cleaned, the empty owned bridge was removed, and the revoked/invalid task Auth alias was deleted last. Final verification contained only the two original devices; the original catalog stayed byte-exact. |

The provider baseline changed concurrently: an unrelated gate-1648 server, SSH registration and primary IPs disappeared. These were observed read-only and were not targets of this run's ordinary original-cloud deletion. This report does not infer causation or claim that unrelated provider inventory stayed unchanged. The pre-existing shared zone network remained protected; this run created or deleted no network.

The shared task Auth cleanup first refused a task-owned inactive SDK lock with mode 0664. Root hardened the same original inode to 0600 under an exclusive lock, preserving the refusal and repair evidence; the qualified cleanup then passed. No original user file or credential slot was altered by that repair. All task live resources were positively cleaned by 17:31 UTC. Persistent nonsecret C03/kernel summaries, command records, raw deployment snapshots and independent observations survive outside temporary storage.

An additional private SSH component lane stopped at its fixed fixture path's read-only guard before extraction or SSH. It supplies no SSH qualification. Separate preliminary Engine/kernel and bounded producer experiments do not qualify full M1 sharing, M2 public exposure or a production worker gate. RunPod cleanup, enrolled Docker cleanup, Core OAuth and actual network-failure lanes remain outside this activation result. Final report prose is compiler-unconsumed; the publication input comparison, exact-head GitHub checks and independent review remain separate merge gates.
