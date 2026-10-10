---
procedure: cloud-failure-cause
feature: The failure cause of a failed cloud, in the Status tab and in the cloud body
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Cloud failure cause test procedure

## 1. Purpose

This procedure makes sure that a long failure cause does not take the layout of
a cloud. The cause uses the size of the output lines and a maximum of three
rows. **Show more**, the tooltip and **Copy error** give the full cause. The
cause shows one time beside the steps, and the output keeps the room for
some lines.

## 2. Applicability

- Candidate: a debug build. The synthetic failed clouds are not in a release
  build.
- Platforms: Linux with the isolated device smoke fixture.
- This procedure does not test these functions:
  - The diagnosis that selects the cause line.
  - A real deployment. The synthetic clouds use no provider and no worker.

## 3. Safety

> **CAUTION:** USE ONLY THE SYNTHETIC CLOUDS OF THIS PROCEDURE. A real cloud can
> show a real container name, host or path in its output.

## 4. Equipment and preconditions

- A frozen debug candidate and its SHA-256.
- The [local device smoke fixture](../../../scripts/device-smoke/README.md),
  `scripts/device-smoke/serve.py`, with `--native-view`.
- A Horizon instance that can show a Device panel.
- A private evidence directory, `<evidence>`, outside the fixture state.

## 5. Setup

1. Start the fixture with the frozen candidate and a new state directory. Set
   `HORIZON_CLOUD_FAILURE_PREVIEW=1`.

   ```sh
   HORIZON_CLOUD_FAILURE_PREVIEW=1 python3 scripts/device-smoke/serve.py \
     --horizon <frozen-candidate> --native-view --state <new-directory>
   ```

   Result: The fixture output shows a `vnc_address`.

2. Open a Device panel with the `vnc_address`.

   Result: The Device panel shows a live view of the candidate.

3. In the candidate, click the workspace **Synthetic failed clouds** in the
   minimap.

   Result: The workspace shows two clouds side by side, with the **Grid**
   arrangement. Each cloud shows "Readiness check failed" in its header.

## 6. Tasks

### 6.1 F01: Failure cause in the Status tab

1. Look at the left cloud. It has one panel, and its **Status** tab is open.

   Result: Under the stage track, a red box shows the failure cause. The cause
   starts with "Error response from daemon: Conflict". It uses the size of the
   output lines and a maximum of three rows. The last row ends with "…".

2. Look at the **Output** list under the red box.

   Result: The list shows a minimum of six rows. The cause line is red in the
   list. No "Root cause" box shows under the list.

3. Put the pointer on the failure cause.

   Result: One tooltip shows the full cause, with the 64-character container
   ID.

4. Click **Show more**. Scroll down in the Status tab.

   Result: The red box shows the full cause. The container ID breaks across
   rows inside the box. The button changes to **Show less**.

5. Click **Show less**.

   Result: The cause uses three rows again.

6. Click **Copy error**. Paste the clipboard into the **Device input test**
   terminal of the fixture.

   Result: The terminal shows the full cause and the line "Readiness check
   failed; inspect deployment output".

### 6.2 F02: Failure cause in a narrow cloud body

1. Look at the right cloud. It has no panels, and it is narrower than 760
   pixels.

   Result: The steps show above the output. The steps show a red mark at
   **Check readiness**. The cause and the output stay inside the cloud. No text
   goes past the right edge of the cloud.

2. Scroll down in the steps until **Check readiness** is at the top of the
   steps.

   Result: Under **Check readiness**, the cause uses a maximum of three rows.
   The last row ends with "…". **Retry deploy**, **Copy error** and **Show
   more** show under the cause. The scroll bar does not cover the step times.

3. Click **Show more**.

   Result: The full cause shows under **Check readiness**. The 64-character
   container ID breaks across rows inside the steps. No text goes past the right
   edge of the steps. The button changes to **Show less**.

4. Click **Show less**.

   Result: The cause uses three rows again.

### 6.3 F03: Failure cause in a wide cloud body

1. Click **Rows** in the workspace toolbar. Scroll the board down to the cloud
   without panels.

   Result: The steps show beside the output. Under **Check readiness**, the
   cause uses a maximum of three rows. **Retry deploy**, **Copy error** and
   **Show more** show under the cause.

2. Look at the output beside the steps.

   Result: The output fills the height beside the steps. No "Root cause" box
   shows under the output.

3. Put the pointer on the failure cause.

   Result: One tooltip shows the full cause.

4. Click **Show more**. Scroll down in the steps. Click **Show less**.

   Result: The full cause shows, then the cause uses three rows again. The
   step times stay visible.

## 7. Pass criteria

- F01, F02 and F03 give the results of each step.
- The failure cause shows one time in the Status tab and one time in the cloud
  body, plus its red line in the output.

## 8. Cleanup

1. Stop the fixture with Ctrl-C.

   Result: The fixture stops its own processes.

2. Close the Device panel of this run.

   Result: The Device panel closes. Other panels do not change.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
