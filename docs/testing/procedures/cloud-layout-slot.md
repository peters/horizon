---
procedure: cloud-layout-slot
feature: Cloud in a workspace layout preset
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Cloud layout slot test procedure

## 1. Purpose

This procedure makes sure that a cloud in a workspace with a layout preset takes
one slot of the preset, like a panel. The cloud moves and resizes with the other
slots.

## 2. Applicability

- Candidate: each candidate that changes workspace presets, cloud geometry or
  the order of slots.
- Platforms: Linux, with the local device fixture.
- This procedure does not test: a connected worker, a cloud in a workspace
  without a preset (freeform placement) or the layout of the panels inside a
  cloud.

## 3. Safety

> **CAUTION:** USE ONLY THE ISOLATED DESKTOP. A drag on the developer desktop
> can move a real panel or cloud.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The [local device smoke fixture](../../../scripts/device-smoke/README.md) with
  `--native-view` and a saved session.
- A Device panel that shows a live view of the fixture.
- In the fixture: one workspace with two terminal panels and one cloud card
  that is not deployed. The cloud rents no compute.

## 5. Setup

1. Start the fixture with the frozen candidate and open its address in a Device
   panel.

   Result: The panel shows the isolated Horizon window.

2. In the toolbar of the workspace, click **Grid**.

   Result: The two terminals and the cloud stand in a grid of three slots of
   the same size. The cloud is one of the slots.

## 6. Tasks

### 6.1 SLOT — Same size as the panels

1. Compare the size of the cloud frame with the size of each terminal.

   Result: The cloud frame and the terminals have the same width and the same
   height. Nothing overlaps.

2. Click **Cols**. Then click **Rows**. Then click **Grid**.

   Result: Each preset puts the cloud in one slot beside the terminals. The
   slots have the same size. Nothing overlaps.

### 6.2 SWAP — Move the cloud to another slot

1. Drag the cloud by the empty part of its header onto a terminal.

   Result: The cloud and the terminal change slots. The sidebar shows the new
   order of the terminals.

2. Drag a terminal by its title bar onto the cloud.

   Result: The terminal and the cloud change slots.

### 6.3 SIZE — Resize the slots

1. Drag the resize grip of a terminal down and to the right.

   Result: Each terminal and the cloud grow to the same new size. The cloud
   moves with its slot.

2. Drag the resize grip of the cloud up and to the left.

   Result: Each terminal and the cloud get the same smaller size. The cloud
   does not become smaller than its header and its empty body.

### 6.4 KEEP — Restart

1. Write down the slot of the cloud. Then close Horizon normally and start the
   candidate again on the same state.

   Result: The cloud is in the same slot. The slots have the same size as
   before.

### 6.5 COLLAPSE — Collapse and expand

1. Collapse the cloud from its context menu.

   Result: The terminals fill the slots. The collapsed cloud does not take a
   slot.

2. Expand the cloud.

   Result: The cloud takes a slot again. Nothing overlaps.

## 7. Pass criteria

- In each preset the cloud takes one slot of the same size as the terminals.
- A drag of the cloud header or of a terminal title bar changes the slots.
- A resize of a terminal or of the cloud changes the size of each slot.
- The cloud keeps its slot after a restart.
- A collapsed cloud gives its slot back.

## 8. Cleanup

1. Close the Device panel. Stop the fixture processes that this run started.

   Result: The candidate process stops. The developer desktop does not change.

## 9. Record of results

Put the results in the pull request. Keep private evidence out of the
repository.
