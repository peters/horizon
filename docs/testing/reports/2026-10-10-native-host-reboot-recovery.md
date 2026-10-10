---
procedure: native-host-reboot-recovery.md
candidate_commit: 54bfe796bf8becbf580684c15df4f24472c933ec
candidate_sha256: 348a159852ab18d5ac42a3eb4209eed8fecfb899d2caad2c1b08a49dbf4084f3
candidate_patch_sha256: d7fa7ccc2900330d9c8236d237972319a46e8e49b0e8209592866ef173aa1307
date: 2026-10-10
lanes: [historical-local-recovery, synthetic-recovery, original-owner-status]
issue: https://github.com/peters/horizon/issues/1373
---

# Native host reboot recovery test report, 2026-10-10

## 1. Summary

The current candidate passed the required local validation and read-only status inspection.
It reported zero pending operations and preserved all 1,302 entries in the original private state.
The earlier, separately approved recovery completed exact local cleanup after a confirmed Linux reboot.
The current candidate did not repeat that cleanup or allocate paid devices.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| RECOVERY | pass | Synthetic tests released only exact resources from an earlier boot and preserved original receipts. | — |
| REFUSAL | pass | Invalid boot proof, live legacy processes, complete operations, and missing or extra local IDs caused refusal. | — |
| EXISTING | pass | Existing-state fixtures preserved files and refused missing journals or original owner bindings without initialization. | — |
| STATUS | pass | The frozen candidate reported zero pending operations. All 1,302 entries retained their contents, inode, device, mode, size, and modification time. | — |
| MATRIX | pass | All required local commands passed. The workspace had 5,753 unfiltered passes and four additional spawned fixture passes. Speech had 1,856 passes. Host tests had 113 passes. | — |
| Pedantic advisory | fail | Advisory only. Existing warnings in five unchanged source files caused exit code 101. Blocking and strict Clippy passed. | — |
| Historical recovery | pass | Two Run resources and two Tunnel resources reached confirmed cleanup. Four private records retained the exact operation and boot proof. | — |
| Historical receipts | pass | All four original process receipts stayed byte-for-byte unchanged. | — |
| Historical provider cleanup | pass | The provider confirmed deletion of the one approved test upload. | — |
| Historical owner status | pass | Status reported no pending operations. The catalog reported no running or queued sessions. | — |

## 3. Defects

No defect was recorded in the current recovery tests or read-only status inspection.
The advisory warnings came from source files that matched the main baseline.

## 4. Deviations from the procedure

The historical recovery used separately approved real resources, not the synthetic procedure.
The operator approved four exact local resources and one owned test upload.
The boot ID, resource list, owner, receipt hashes, and absent process identities matched the private proof.
An earlier approved controller performed that cleanup.
The current candidate binary did not perform it.

The listed source commit contains the recovery source and guides on main commit `69d2b836cee7ad1ec9d625fc7cc8c6c9e3a089f3`.
The report metadata followed as a document-only change.
The full matrix and status inspection used identical runtime inputs.
The final procedure checks passed after the template correction.
Private hash and timestamp comparisons proved that this correction changed no runtime input.

This historical recovery did not qualify the interrupted endurance run.
The later iOS qualification has a separate [deep-link report](2026-10-10-native-ios-deep-links.md).

## 5. Cleanup

The historical run completed the approved local and provider cleanup.
The original owner, private state, receipts, and recovery records stayed available.
The current status inspection made no persistent change.
No real cleanup or paid allocation repeated.

## 6. Evidence

Private evidence retains the historical invocation, receipt hashes, process identities, recovery records, and controller output.
The current evidence retains the source manifest, command results, frozen binary hash, and complete state snapshots.
The source commit and executable hashes above identify the current candidate.
Public evidence omits private references and machine identities.
