---
procedure: cloud-close
feature: The × of a cloud, with the deletion of its resources
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Cloud close test procedure

## 1. Purpose

This procedure makes sure that the **×** of a cloud first offers to delete the
resources of the cloud. It also makes sure that **Remove from Horizon anyway**
shows only after a deletion fails or cannot start. A cloud without resources
closes with **Remove cloud**.

## 2. Applicability

- Candidate: a build that has the dialog with **Delete cloud resources**.
- Platforms: Linux with the local device smoke fixture.
- This procedure does not test these functions:
  - A deletion at a provider that completes. Area X of the
    [cloud panels procedure](cloud-panels/x-teardown.md) deletes real
    resources.
  - A deletion that starts and fails at the provider. The fixture has no
    provider settings, so the deletion cannot start.

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

1. On the card of `demo-api`, click **×**. Then click **Delete cloud resources**.

   Result: The dialog stays open. It shows the reason in red, after
   "Could not delete the cloud resources". It tells that worker `pod-7f3a2c`
   and its workspace storage can stay at RunPod and cost money. It shows
   **Delete cloud resources**, **Remove from Horizon anyway** and **Cancel**.

2. Click **Cancel**. Then click **×** again.

   Result: The dialog shows **Delete cloud resources** and **Cancel** only.
   Each new close offers the deletion first.

3. Click **Delete cloud resources**. Then click **Remove from Horizon anyway**.

   Result: The dialog closes. The cloud `demo-api` and its panel go off the
   board and the sidebar.

### 6.3 C03: Remove a cloud that has no resources

1. On the card of `scratch-notes`, click **×**.

   Result: The dialog **Close scratch-notes?** tells that the cloud has no
   worker or storage at its provider. It shows **Remove cloud** and **Cancel**.

2. Click **Remove cloud**.

   Result: The dialog closes. The cloud `scratch-notes` and its panel go off
   the board.

### 6.4 C04: Close a cloud that has storage but no worker request

1. In the sidebar, click `image-push`. On its card, click **×**.

   Result: The dialog **Close image-push?** shows **Delete cloud resources**
   and **Cancel**. It does not tell that the cloud has no worker or storage at
   its provider. It does not show **Remove from Horizon anyway**.

2. Click **Delete cloud resources**.

   Result: The dialog stays open. It shows the reason in red, after
   "Could not delete the cloud resources". It tells that a worker and its
   workspace storage can stay at RunPod and cost money. It shows
   **Remove from Horizon anyway** and **Cancel**.

3. Click **Remove from Horizon anyway**.

   Result: The dialog closes. The cloud `image-push` and its panel go off the
   board.

## 7. Pass criteria

- The first dialog for a deployed cloud never shows **Remove from Horizon
  anyway**.
- **Remove from Horizon anyway** shows only after a deletion fails or cannot
  start, and the dialog names what can stay at the provider.
- A cloud without resources closes with **Remove cloud**.
- A cloud whose record has storage offers **Delete cloud resources** first,
  even without a worker request.
- **Cancel** and the Escape key keep the cloud.

## 8. Cleanup

1. Stop the fixture with Ctrl-C.

   Result: The fixture stops its processes and removes its private data.

2. Close the Device panel.

   Result: The Device panel closes.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
