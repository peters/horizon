---
procedure: one-service-gateway-engine
candidate_commit: 37e53050a04c3bd239a5e0e1d43a6d9a789666c6
candidate_sha256: be07d2da229c3b874ab3628fe28366d21001632f981d67c1fef7decbdf8f6d97
tested_source_inventory_sha256: 5e074353eb3c91be7bd12d5584633ff6f4131cd8f51c9f9e23aaa0e0d522bd93
date: 2026-10-15
lanes: [local-linux, real-kernel-tailnet]
issue: https://github.com/peters/horizon/issues/1335
---

# One-device endpoint engine test report, 2026-10-15

## 1. Summary

The required local matrix and the real R01/R02 component tasks passed.
The endpoint engine stayed off by default.
The real tasks used the immutable `be07` Core test artifact from two owned Linux guests.
These results did not qualify native owner approval, durable recovery, worker SSH, actual LAN bytes or UI/CLI/MCP parity.

The header commit identified the base of the uncommitted Engine candidate.
Private inventories identified all 2,204 tested source files.
The inventory hash covered sorted records of each file's SHA-256, two spaces, its repository-relative path and a newline.

The two enabled Core artifacts had different identities and features.
Both had `profile.test=true`.

| Artifact | SHA-256 | Actual Core features | Use |
|---|---|---|---|
| Real component, `be07` | `be07d2da229c3b874ab3628fe28366d21001632f981d67c1fef7decbdf8f6d97` | `cloud-workspaces,default,one-service-gateway,tailnet-node-cleanup` | R01 and R02 |
| Fresh enabled Core, `ea9b` | `ea9bcf5c79f4fbbc84b7f8f43d4e2bae946e2fb8e8dd48450f6d3051c7d3185a` | `cloud-workspaces,default,one-service-gateway` | Current local component matrix |

The real artifact included the optional cleanup feature.
The fresh enabled Core artifact did not include it.
The independent Engine change did not add a cleanup default edge.
Cleanup activation remained a separate outcome in #1646.

All Rust, dependency, build configuration and embedded inputs for `be07` still matched the candidate.
Only two plain documents changed before the fresh matrix: the Engine procedure and its review instructions.
Those edits corrected the cleanup default description.
The new report and its index entry changed only prose after the matrix.
The fresh `ea9b` artifact did not replace `be07` as the real-tested executable.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| E01 | pass | Fresh enabled Core covered 2,117 executed cases, with 23 intentional ignores. Parser, binding, relay, half-close and exact-receipt cases passed. | — |
| E02 | pass | Default Core covered 2,106 executed cases, with 21 intentional ignores. Existing bridge checks passed. Both new engine and cleanup features stayed absent from defaults. | — |
| E03 | pass | Workspace blocking/strict tiers, enabled Core blocking/strict tiers, doctests, formatting, maintainability and bundled skill checks passed. | — |
| R01 | pass | Actual peer preflight and gateway bytes passed. Self admission was refused. The receipt explicitly excluded owner and worker qualification. | — |
| R02 | pass | An actual source observation selected a non-Self source for a non-Self CGNAT destination. Production preflight returned `UnsupportedSource` before peer admission. | — |

The default workspace build produced 63 test harnesses and seven normal integration binaries.
All 63 harness commands returned zero in the pinned isolated environment.
The custom live harness skipped its opt-in live case.
No ignored test was counted as an executed case.

The isolated default Core run passed 2,095 cases.
Eleven exact cases passed on the host with the same frozen binary.
The isolated worker run passed 209 cases.
One exact worker case passed on the host with the same frozen binary.
The fresh enabled Core run passed 2,106 isolated cases and the same eleven host cases.

The default UI suite passed 2,284 tests.
The speech UI suite passed 2,330 tests.
Each UI configuration had two intentional ignores, three font test passes and two home test passes.
These were unit and integration tests, not native UI smoke.

All checks used Rust 1.98.1, locked offline dependencies and `RUSTFLAGS=-D warnings`.
Builds used twelve jobs, no incremental cache and line-table debug information.
The toolchain hashes matched the recorded inputs.
The capacity guards passed throughout the run.

## 3. Defects

No blocking or strict failure remained.
Advisory pedantic Clippy exited 101 for inherited `struct_excessive_bools` findings in provider `Description` and browser `GroupState`.
Both affected source files were byte-identical to the base.
No lint suppression was added.
This was not a complete pedantic pass.

## 4. Deviations from the procedure

The pinned guest image lacked the `ip` utility.
An initial command exited 127 before the Engine test ran.
That command failure was not an Engine failure.
The successful R02 task used an actual UDP socket connect to observe the selected source.
It sent no packet and changed no route.

The workspace tests used frozen harnesses in isolated containers after their complete build.
The containers had UID 10001, `tini`, no external network and a root-owned sticky `/tmp`.
The known read-only/Git-directory cases and one worker lease case used exact host complements.
These cases used the same frozen binaries, without a source workaround.

The copied result counter misclassified successful runs because nested child harnesses added summary lines.
The default run returned zero for all 63 sequential commands and recorded 70 passing libtest summaries.
The enabled Core run returned zero and recorded eight passing summaries.
Separate coverage proofs retained the original counter results and accounted for each command.
No product test failed because of that counter.

## 5. Cleanup

- The task Auth key was revoked and invalid.
- Both owned guest containers were absent.
- The captured task nodes were absent from the final inventory.
- The empty task bridge was absent.
- Task credential aliases were removed last.
- The original credential catalog stayed byte-identical, and the original two devices remained unchanged.
- Global managed-API metadata and its pending journal stayed absent.
- The existing staged Personal entry stayed in its original slot.
- All three local qualification containers were removed, with exact absence evidence.

## 6. Evidence

Private evidence retained exact source inventories, toolchain hashes, feature/profile records, frozen artifacts, commands, route observations and cleanup receipts.
The report excluded credentials, operational identifiers, account names and private paths.
The real component receipts explicitly recorded `owner_or_worker_qualification: false`.

The [Engine procedure](../procedures/one-service-gateway-engine.md) defined E01–E03 and R01/R02.
Independent review and exact-head GitHub checks remained separate gates.
Completed M1 delivery still required the native owner path, worker SSH, actual LAN and tailnet sharing, and public interface parity.

## 7. Final review fixes and fresh real component run

The first PR review found two endpoint defects. Requested names with one terminal
dot did not use the approved name's normalization. LAN uniqueness was checked
after policy filtered or limited the answers. Both defects are fixed. The gate
checks the complete canonical IPv4 set before it applies the existing LAN policy
to the single literal. The existing bridge keeps its behavior. Six regression
cases cover normalized names, mixed and mapped answers, duplicates before another
address, duplicate-only resolution, start and active ambiguity, permanent
revocation, and route loss during resolution.

The final enabled Core artifact SHA-256 was
`30fa08eb0dfe8a48a748627ac169e44ea3d97e7793563bf821d4af1019f42665`.
Its features were `cloud-workspaces,default,one-service-gateway`, with cleanup
off and `profile.test=true`. Earlier artifact identities remain historical.

The fresh enabled Core matrix passed 2,123 cases, with 23 intentional ignores:
2,111 isolated cases and 12 exact cases with the same binary on the host. The
focused endpoint suite passed 17 cases; its two real cases ran separately.
Default UI passed 2,284 cases and speech UI passed 2,330 cases. Each configuration
also passed three font and two home tests, with two intentional UI ignores.
Required workspace and enabled-Core Clippy, formatting, maintainability and
bundled skill checks passed. Unchanged default workspace targets and doctests
used the complete recorded input comparison. The later test-only receipt change
was excluded from normal and default products by feature and dep-info comparison.
Enabled checks ran again. Advisory findings remained inherited; this was not a
pedantic pass. No introduced or worsened warning remained.

Fresh R01 and R02 passed on the final artifact in two owned kernel-mode Linux
guests. R01 used actual full MagicDNS resolution, exact peer WhoIs, Self source
and relay bytes with half-close. Wrong-port and Self requests were refused. R02
observed an actual non-Self source without changing a route and received the
production `UnsupportedSource` refusal. Both receipts reported
`qualification: "engine_component"`.

The task Auth key was revoked and confirmed invalid. Both original guest
containers and their captured nodes were absent. The empty bridge was removed.
The additional CPU cleanup-test consumer was absent before the task Auth alias
was removed last. A final API read showed only the original two devices. A
task-owned catalog lock was hardened from 0664 to 0600 under its original
exclusive lock before qualified alias cleanup passed. Original user catalogs
and credential slots were preserved.

These tasks do not qualify actual LAN bytes, native owner admission, worker SSH
or UI/CLI/MCP sharing. Optional SSH component preparation stopped at a read-only
guest fixture path before extraction or SSH effects. It was not counted as a
passing component test.

## 8. Admission revocation recheck

A later review found that revocation could occur during a successful source
observation. Admission now checks the sticky gate state again before it returns
the destination. A regression pauses that observation, revokes the same gate and
verifies refusal when the observation resumes. No shared bridge policy changed.

The new Core artifact SHA-256 was
`65cd28fb72c0dfaae367fea232a4c454ccc65dd1b0da68eb4d2c6b204eeea1bb`.
It passed 2,124 enabled cases, with 23 intentional ignores. One existing shell
shutdown case exceeded its host time bound in the full run; that exact case
passed separately with the same frozen executable. Both raw results were kept.
Enabled blocking and strict Clippy, Core doctests and static checks passed.
The recorded input comparison preserved the applicable default matrix results.

Fresh R01 and R02 passed on this executable with two newly enrolled kernel-mode
guests. The actual DNS, relay, half-close and refusal checks used the production
engine. Both containers, their captured nodes and the bridge were removed. The
task key was revoked and confirmed invalid before its private alias was removed
last. The original two-device tailnet remained intact. These remain engine
component results; the broader M1 qualification listed above is still required.
