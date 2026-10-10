---
procedure: tailnet-card-device-name
feature: Cloud card tailnet device identity
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Cloud card tailnet device name test procedure

## 1. Purpose

This procedure tests the device name and copy action in **Connections** >
**Tailnet**. It tests observed names, stable expected names, and saved records.

## 2. Applicability

- Candidate: the current debug build of Horizon.
- Automated tasks use synthetic data and do not start a cloud.
- The isolated desktop tests use saved synthetic cloud records.
- Provider allocation, enrollment, and DNS reachability are outside this procedure.

## 3. Equipment and preconditions

- A clean candidate worktree and its own Cargo target directory.
- The [device smoke fixture](../../../scripts/device-smoke/README.md).
- A live Device panel for the isolated desktop.
- A frozen candidate and its SHA-256.
- Private synthetic cloud records with no credentials or remote endpoints.

## 4. Automated tasks

### TN01 — Worker contract and device names

1. Run the contract tests.

   ```sh
   cargo test -p horizon-core cloud_runtime::worker_contract
   ```

   Result: Contract 2 supports enrollment and stable names. Contract 1 does not
   support derived names. Incidental text does not enable either function.

2. Run the device identity tests.

   ```sh
   cargo test -p horizon-core cloud_runtime::tailnet
   ```

   Result: The first device supplies the actual name. Renames, collision suffixes,
   and the MagicDNS domain remain. The final DNS dot is removed. Invalid data
   does not select a peer. Zero or one final DNS dot is valid; more dots are not.
   A first in-progress publication does not establish freshness. Two later atomic
   generations supply the saved observation. A failed bounded read supplies a stable fallback only
   for contract 2. Cancellation returns an error. No selection makes no read.

3. Run the saved record test.

   ```sh
   cargo test -p horizon-core tailnet_identity_survives_legacy_migration_and_refresh
   ```

   Result: The saved name survives deployment storage and legacy record conversion.
   A cleared name remains absent. Earlier records retain their encoding.

### TN02 — Stable short names

1. Examine the stable-name test results for uppercase, underscore, long, and
   reserved digest-form cloud IDs.

   Result: The derived name uses the same digest rule as the worker helper.
   The short name does not claim a MagicDNS domain.

## 5. Isolated desktop tasks

### TN03 — Observed name and copy

1. Start the frozen candidate with a saved synthetic cloud and an observed name
   such as `renamed-worker-1.example.ts.net`.

   Result: The Device panel shows the candidate. Public inspections show advancing
   frames while a fixture terminal changes.

2. Open the cloud card's **Connections** tab.

   Result: **Tailnet** shows **Device name** and the full observed name. The note
   identifies the name as the last observation and explains how to refresh it.

3. Select **Copy**.

   Result: The isolated desktop clipboard contains the displayed name. Read the
   clipboard from the isolated display to examine the exact value.

4. Resize the candidate window and use Fit.

   Result: The name can be truncated. Its hover text shows the full name. **Copy**
   stays accessible. The drawer has no overlapping text or controls.

### TN04 — Fallback and older records

1. Repeat TN03 with an expected short name and `observed: false`.

   Result: The note identifies an expected name and an unavailable MagicDNS domain.
   **Copy** copies only that short name.

2. Repeat with a bare observed name.

   Result: The note identifies an observed name and an unavailable domain.

3. Repeat with a legacy record that has no `tailnet_device` field.

   Result: The card shows no derived device name or copy action.

4. Repeat with no tailnet selection and no saved device identity.

   Result: **Tailnet** shows **None** and no device name.

## 6. Runtime refresh

During an authorized deployment or reconnection, Horizon reads the public device
snapshot after enrollment. The name is a saved observation, not a continuous
poll. A later administrator rename requires another connection. This procedure
uses synthetic data; it does not authorize a real connection or cloud operation.
The read waits for two new generations from the worker inventory publisher.
The worker must have its existing `/usr/bin/python3`. If the read exceeds its
15-second limit or cannot confirm freshness, Horizon shows no observed name.

## 7. Evidence and cleanup

Record the candidate SHA-256, task results, screenshots after launch and resize,
three timestamped public viewer inspections, and a short video of the copy flow.
Convert the video to a GIF for the PR. Use only synthetic names. Close only the
owned candidate, fixture processes, and Device panel.
