---
procedure: cast-workspace-clouds
feature: Casting a workspace that contains clouds
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Cast a workspace with clouds test procedure

## 1. Purpose

This procedure makes sure that a cast of a workspace shows the cloud cards in
that workspace.

## 2. Applicability

- Candidate: each candidate that changes cast capture, cloud cards or
  workspace bounds.
- Platforms: Linux, with the local device fixture and one paired Apple TV or
  Chromecast receiver on the same network.
- This procedure does not test: the cast of one cloud card as its own source,
  a workspace with only clouds (it has no Cast entry until the cloud card gets
  one; a unit test covers its bounds), pairing, or the encoder.

## 3. Safety

> **CAUTION:** USE ONLY THE ISOLATED DESKTOP. The receiver shows everything in
> the cast source. Do not cast a workspace that shows a key, a token or a
> private terminal.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The [local device smoke fixture](../../../scripts/device-smoke/README.md) with
  `--native-view` and a saved session.
- A Device panel that shows a live view of the fixture.
- In the fixture: workspace A with one terminal and one cloud card that is not
  deployed. A cloud that is not deployed rents no compute.
- One receiver that the candidate can pair with.

## 5. Setup

1. Start the fixture with the frozen candidate and open its address in a Device
   panel.

   Result: The panel shows the isolated Horizon window.

2. Zoom the canvas out until workspace A and its cloud are completely in view.

   Result: The terminal and the whole cloud card are in the canvas.

## 6. Tasks

### 6.1 BOTH — Workspace with a panel and a cloud

1. Click the Cast icon of the terminal in workspace A. In the picker, select
   the workspace source and the receiver. Click **Start**.

   Result: The receiver shows the terminal and the cloud card, with the card
   header and body. The cast does not stop with `covered`.

2. Open the drawer of the cloud card with its chevron.

   Result: The receiver also shows the drawer. The cast continues.

3. Click **Stop** in the cast controls.

   Result: The receiver stops showing Horizon.

### 6.2 VIEW — Part of the cloud out of view

1. Start a cast of workspace A. Move the canvas until a part of the cloud card
   is outside the canvas.

   Result: The cast pauses or stops with `Fit the entire source into view`.

### 6.3 OVER — A foreign panel over the cloud

1. Start a cast of workspace A again. Drag a terminal from another workspace
   over the cloud card.

   Result: The cast stops with `Another panel overlaps this source`.

## 7. Pass criteria

- A workspace cast shows its cloud cards and their drawers.
- The cloud's own header, body and drawer, and a panel's own resize grip, do
  not stop the cast as covering it.
- A foreign panel over the cloud stops the cast.

## 8. Cleanup

1. Stop each cast. Close the Device panel. Stop the fixture processes that this
   run started.

   Result: The receiver shows no Horizon content. The developer desktop does
   not change.

## 9. Record of results

Put the results in the pull request. Keep private evidence out of the
repository.
