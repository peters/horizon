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
