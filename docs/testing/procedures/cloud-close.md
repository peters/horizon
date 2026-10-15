---
procedure: cloud-close
feature: The × of a cloud, with the deletion of its resources
platforms: [linux]
cost: none
destructive: yes
secrets: none
owner: peters
---

# Cloud close test procedure

## 1. Purpose

This procedure makes sure that the **×** of a cloud first offers to delete the
resources of the cloud. It also makes sure that **Remove from Horizon anyway**
shows only after a deletion fails or cannot start. A cloud without resources
closes with **Remove cloud**. Saved-record checks run outside the UI thread.
The dialog shows **Checking the saved cloud record…** until a check finishes.

## 2. Applicability

- Candidate: a build that has the dialog with **Delete cloud resources**.
- Platforms: Linux with the local device smoke fixture.
- This procedure does not test these functions:
  - A deletion at a provider that completes. Area X of the
    [cloud panels procedure](cloud-panels/x-teardown.md) deletes real
    resources.
  - A deletion that starts and fails at the provider. The fixture has no
    provider settings, so the deletion cannot start.
  - **Remove from Horizon anyway** when Horizon cannot lock the state
    directory of the cloud. Unit tests make sure that the cloud stays and
    that the dialog shows the reason.
  - The dialog while a different Horizon instance runs an operation of the
    cloud. Unit tests make sure that the dialog shows only **Cancel** until
    that operation ends.

## 3. Safety

> **CAUTION:** USE ONLY THE SYNTHETIC FIXTURE IN THIS PROCEDURE. The fixture
> has no provider settings, so no step sends a request to a provider. With
> real settings, **Delete cloud resources** deletes a real worker and its storage.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The [local device smoke fixture](../../../scripts/device-smoke/README.md),
  `scripts/device-smoke/serve.py`, with `--native-view`.
- A tools root, `<tools>`, that contains `usr/bin/x11vnc` if the host does not
  have `x11vnc`. It must be outside the home directory.
- The files in [`cloud-close/`](cloud-close/): `horizon-seeded`, `seed.py` and
  `runtime.yaml`. `seed.py` writes a session with three clouds into the private
  home of the fixture:
  - `demo-api`: a deployed cloud with a RunPod worker `pod-7f3a2c`.
  - `scratch-notes`: a cloud that was not deployed.
  - `image-push`: a cloud whose image push stopped before a worker was
    requested. Its record has a workspace storage marker.
- A private evidence directory, `<evidence>`, outside the fixture state.

## 5. Setup

1. Copy the three fixture files and the frozen candidate into `<tools>`. Name
   the candidate `horizon`.

   ```sh
   cp docs/testing/procedures/cloud-close/* <tools>/
   cp <frozen-candidate> <tools>/horizon
   ```

   Result: `<tools>` contains `horizon`, `horizon-seeded`, `seed.py` and
   `runtime.yaml`.

2. Start the fixture with the wrapper and a new state directory.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <tools>/horizon-seeded \
     --tools <tools> --native-view --state <new-directory>
   ```

   Result: The fixture output shows a `vnc_address`.

3. Open a Device panel with the `vnc_address`.

   Result: The Device panel shows a live view of the candidate.

4. Find the Horizon process of the fixture. Compare the SHA-256 of
   `/proc/<pid>/exe` with the SHA-256 of the frozen candidate.

   Result: The two values are the same.

5. Examine the board of the candidate.

   Result: The board shows the clouds `demo-api`, `scratch-notes` and
   `image-push`. The card
   of `demo-api` shows a failure, because the fixture has no provider settings.

## 6. Tasks

Give each task an ID. A report uses the ID to give a result.

After each **×**, wait for the saved-record check before the next action.
A check shows a spinner and permits **Cancel**. It grants no removal action.

> **CAUTION:** REMOVE ONLY SYNTHETIC CLOUDS. The removal steps close panels and remove fixture data from the board.
> Remove anyway leaves the original deployment record on disk.

### 6.1 C01: Close a deployed cloud

1. On the card of `demo-api`, click **×**.

   Result: The dialog **Close demo-api?** opens. It tells which resources the
   deletion removes. It shows **Delete cloud resources** and **Cancel**. It
   does not show **Remove from Horizon anyway**.

2. Click **Cancel**.

   Result: The dialog closes. The cloud and its panel stay on the board.

3. Click **×** again. Then push the Escape key.

   Result: The dialog closes. The cloud stays on the board.

### 6.2 C02: Remove a cloud after its deletion cannot start

1. On the card of `demo-api`, click **×**.

   Result: The dialog opens after its record check.

> **CAUTION:** USE ONLY THE SYNTHETIC CLOUD. Real provider settings can delete a worker and its storage.

2. Click **Delete cloud resources**.

   Result: The dialog stays open. It shows the reason in red, after
   "Could not delete the cloud resources". It tells that worker `pod-7f3a2c`
   and its workspace storage can stay at RunPod and cost money. It shows
   **Delete cloud resources**, **Remove from Horizon anyway** and **Cancel**.

3. Click **Cancel**.

   Result: The dialog closes.

4. Click **×** again.

   Result: The dialog shows **Delete cloud resources** and **Cancel** after its record check.
   Each new close offers the deletion first.

> **CAUTION:** USE ONLY THE SYNTHETIC CLOUD. Real provider settings can delete a worker and its storage.

5. Click **Delete cloud resources**.

   Result: The fixture refuses deletion and offers **Remove from Horizon anyway**.

> **CAUTION:** REMOVE ONLY THE SYNTHETIC CLOUD. The cloud and its panels leave the board. The original deployment record stays.

6. Click **Remove from Horizon anyway**.

   Result: The dialog closes. The cloud `demo-api` and its panel go off the
   board and the sidebar.

### 6.3 C03: Remove a cloud that has no resources

1. On the card of `scratch-notes`, click **×**.

   Result: The dialog **Close scratch-notes?** tells that the cloud has no
   worker or storage at its provider. It shows **Remove cloud** and **Cancel**.

> **CAUTION:** REMOVE ONLY THE SYNTHETIC CLOUD. The cloud and its panels leave the board.

2. Click **Remove cloud**.

   Result: The dialog closes. The cloud `scratch-notes` and its panel go off
   the board. The operation lock stays in the state directory.

### 6.4 C04: Close a cloud that has storage but no worker request

1. In the sidebar, click `image-push`. On its card, click **×**.

   Result: The dialog **Close image-push?** shows **Delete cloud resources**
   and **Cancel**. It does not tell that the cloud has no worker or storage at
   its provider. It does not show **Remove from Horizon anyway**.

> **CAUTION:** USE ONLY THE SYNTHETIC CLOUD. Real provider settings can delete workspace storage.

2. Click **Delete cloud resources**.

   Result: The dialog stays open. It shows the reason in red, after
   "Could not delete the cloud resources". It tells that the workspace
   storage can stay at RunPod and cost money. It does not name a worker,
   because no worker was requested. It shows **Remove from Horizon anyway**
   and **Cancel**.

> **CAUTION:** REMOVE ONLY THE SYNTHETIC CLOUD. The cloud and its panels leave the board. The original deployment record stays.

3. Click **Remove from Horizon anyway**.

   Result: The dialog closes. The cloud `image-push` and its panel go off the
   board.

### 6.5 C05: Refresh an unknown or externally deleted record

Use a new fixture for this task. Change only its private synthetic records.

> **CAUTION:** CHANGE ONLY THE OWNED SYNTHETIC RECORD. A missing production record can prevent cleanup of real resources.

1. Move `demo-api/deployment.json` out of the fixture's cloud directory.

   Result: The synthetic deployment record is absent.

2. Click **×** on `demo-api`.

   Result: After the check, the dialog explains that Horizon cannot read the resource state.
   It shows **Remove from Horizon anyway** and **Cancel**, with no deletion action.

3. Click **Cancel**.

   Result: The dialog closes.

4. Restore the synthetic deployment record.

   Result: The original synthetic record returns.

5. Click **×** on `demo-api`.

   Result: The dialog offers deletion after its record check.

> **CAUTION:** USE ONLY THE SYNTHETIC CLOUD. Real provider settings can delete a worker and its storage.

6. Click **Delete cloud resources**.

   Result: The fixture refuses deletion and shows its failure.

> **CAUTION:** CHANGE ONLY THE SYNTHETIC RECORD. A false production deletion record can hide resources that still cost money.

7. Set the private record's stage to `Deleted` and its operation to `terminated`.

   Result: The synthetic record represents a completed deletion.

8. Wait for the open dialog to refresh.

   Result: The dialog offers **Remove cloud**, with no stale failure or resource warning.

> **CAUTION:** REMOVE ONLY THE SYNTHETIC CLOUD. The cloud and its panels leave the board.

9. Click **Remove cloud**.

   Result: The cloud disappears.

### 6.6 C06: Check blocked reads and stale results

1. Run the focused worker regressions in the candidate checkout.

   ```sh
   RUSTFLAGS="-D warnings" cargo test --locked -p horizon-ui cloud_panel::production::removal::tests
   RUSTFLAGS="-D warnings" cargo test --locked -p horizon-ui cloud_panel::production::close
   ```

   Result: A real `deployment.json` FIFO blocks the worker read. UI polling, dialog rendering and an unrelated board action return.
   Cancel, session reset and repeated requests retain one job for that record until its read finishes.
   Only the newest generation receives a final result. The worker holds its Store lock through the UI discard.
   A changed launch cannot use an older result. Automatic close treats Pending as work in progress.
   A blocked periodic refresh keeps settled close controls visible. A final removal waits for a fresh worker decision.
   Cancel and Escape clear automatic close intent after a completed deletion. When the blocked read finishes, the cloud and its panels remain.
   A Ready connection watch permits explicit removal after deletion fails before startup. A busy operation or a held Store lock still refuses removal.

### 6.7 C07: Check the small window

1. Resize the candidate window to a smaller size.

   Result: The window becomes smaller.

2. Click **Fit** while the window is small.

   Result: The board fits the small window. The close controls remain readable.

3. Restore the candidate window size.

   Result: The window returns to its original size.

4. Click **Fit** again.

   Result: The board fits the restored window.

## 7. Pass criteria

- The first dialog for a deployed cloud with a readable resource record
  offers deletion. A missing record offers removal anyway with an unknown-state warning.
- **Remove from Horizon anyway** shows only after a deletion fails or cannot
  start, and the dialog names what can stay at the provider.
- A cloud without resources closes with **Remove cloud**.
- A cloud whose record has storage offers **Delete cloud resources** first,
  even without a worker request.
- **Cancel** and the Escape key keep the cloud.
- A current empty record offers **Remove cloud** after an earlier failure.
- A blocked record read does not block the UI or authorize removal.
- A cancelled or stale result cannot remove a replacement cloud.
- A final check retains its Store lock through the UI discard.

## 8. Cleanup

1. Stop the fixture with Ctrl-C.

   Result: The fixture stops its processes and removes its private data.

2. Close the Device panel.

   Result: The Device panel closes.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
