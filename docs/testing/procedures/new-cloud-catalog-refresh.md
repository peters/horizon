---
procedure: new-cloud-catalog-refresh
feature: New cloud dialog, background price refresh and layout height
platforms: [linux]
cost: none
destructive: yes
secrets: [RunPod API key in Cloud settings]
owner: peters
---

# New cloud catalog refresh test procedure

## 1. Purpose

This procedure proves that a background price refresh in the **New cloud…**
dialog keeps the last comparison, the account check and the layout. While the
refresh runs, only the text **Updated N s ago** and the **Refresh** button change.
It also makes sure that the dialog body uses the full height of the dialog in a
narrow window. The dialog must not move or change size after it opens.

## 2. Applicability

- Candidate: a build that includes the fixes for
  [issue #1293](https://github.com/peters/horizon/issues/1293) and
  [issue #1299](https://github.com/peters/horizon/issues/1299).
- Platforms: Linux with Xvfb. Provider: RunPod.
- This procedure does not test:
  - A refresh that takes more than 30 seconds. Unit tests cover this case.
  - A Hetzner refresh. Hetzner prices refresh each 15 minutes. Unit tests cover
    this case.
  - The start of a cloud.

## 3. Safety

> **CAUTION:** DO NOT CLICK **Start cloud**. This button rents compute from the
> provider. The provider charges money until somebody deletes the worker.

> **CAUTION:** DO NOT PUT THE API KEY IN SCREENSHOTS, RECORDINGS OR LOGS. A
> person who gets the key can rent compute on that account.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The fixture `scripts/device-smoke/serve.py` with `--native-view`.
- A Device panel that shows a live view of the isolated desktop.
- A RunPod API key from the secret store of the test account.
- A repository with a `.horizon/cloud.yml` file and a CPU profile.
- `xdotool` on the host, for task C5.

## 5. Setup

1. Start the fixture with a new directory for its state.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <frozen-candidate> \
     --native-view --state <new-directory>
   ```

   Result: The fixture output shows a `vnc_address`.

2. Open a Device panel with the `vnc_address`.

   Result: The Device panel shows a live view of the candidate.

3. In the candidate, open **Cloud settings…**.

   Result: The Cloud settings dialog opens.

   > **CAUTION:** PASTE THE KEY ONLY INTO THE CANDIDATE IN THE ISOLATED DESKTOP.
   > If you paste it in another window, other people or logs can get the key.

4. Paste the RunPod API key.

   Result: The dialog shows that the key is saved.

## 6. Tasks

Give each task an ID. A report uses the ID to give a result.

### 6.1 C1 — Current prices

1. Open **Cloud › New cloud…**.

   Result: The New cloud dialog opens.

2. Type the path of the repository.

   Result: The dialog reads the repository.

3. Select the CPU profile.

   Result: The dialog shows **Fetching prices and stock…**.

4. Wait until the prices load.

   Result: The dialog shows the **Cheapest**, **Balanced** and **Most powerful**
   cards.

5. Examine the **BEFORE YOU START** list.

   Result: The list shows **RunPod account accepted, prices are current**.

### 6.2 C2 — Background refresh

1. Start a recording of the isolated desktop.

   Result: The recorder writes frames.

2. Take a screenshot of the dialog each second for 60 seconds.

   Result: Some screenshots show **Updated N s ago · checking again…**.

3. If no screenshot shows **checking again…**, do step 2 again.

   Result: At least one screenshot shows **checking again…**.

4. Examine each screenshot.

   Result: No screenshot shows **Comparison incomplete**.

5. Examine the **BEFORE YOU START** list in each screenshot.

   Result: Each screenshot shows **RunPod account accepted, prices are current**.

6. Compare the height of the three cards and the list of workers in all screenshots.

   Result: The cards and the list of workers do not move.

7. Stop the recording.

   Result: The recorder writes the file and stops.

### 6.3 C3 — Click during a refresh

1. Wait until the dialog shows **checking again…**.

   Result: The dialog shows **Updated N s ago · checking again…**.

2. Immediately click the **Most powerful** card.

   Result: The summary shows the worker of the **Most powerful** card.

3. If the refresh stops before the click, do this task again.

   Result: The click occurs while the dialog shows **checking again…**.

### 6.4 C4 — Manual refresh

1. Click **Refresh** beside the text **Updated N s ago**.

   Result: The dialog shows **checking again…**.

2. Wait until the prices load again.

   Result: The dialog shows the **Cheapest**, **Balanced** and **Most powerful**
   cards.

   Note: Until the new prices arrive, the dialog can show **Comparison
   incomplete**. This is the correct result for a manual refresh.

### 6.5 C5 — Narrow window

This task does not need prices. A saved key that is not valid is enough. The
fixture opens the candidate window at 1480 × 900 pixels.

1. Close the New cloud dialog with **Cancel**.

   Result: The dialog closes.

2. Open **Cloud › New cloud…**.

   Result: The dialog shows the summary to the right of the worker fields.

3. Measure the bottom edge of the dialog.

   Result: You have the reference position for step 8.

4. Close the New cloud dialog with **Cancel**.

   Result: The dialog closes.

5. Set the candidate window to 800 × 900 pixels. Use the `display` value from
   the fixture output.

   ```sh
   DISPLAY=<display> xdotool search --name '^Horizon$' windowsize %1 800 900
   ```

   Result: The window is 800 pixels wide. The toolbar shows **More** instead of
   **Cloud**.

6. Start a recording of the isolated desktop.

   Result: The recorder writes frames.

7. Open **More › Cloud › New cloud…**.

   Result: The dialog shows the summary below the worker fields in one column.

8. Measure the bottom edge of the dialog.

   Result: The bottom edge is less than 40 pixels from the reference position.

9. Find the first recorded frame that shows the dialog.

   Result: The frame shows the dialog heading **New cloud**, **Cancel** and the
   action button.

10. Measure the top edge of the dialog in each frame of the next 2 seconds.

    Result: The top edge is at the same position in each frame.

11. Scroll the body of the dialog to the bottom.

    Result: The summary moves up. The dialog heading **New cloud**, **Cancel**
    and the action button do not move.

12. Stop the recording.

    Result: The recorder writes the file and stops.

## 7. Pass criteria

- C1 shows the three cards and the accepted RunPod account.
- C2 shows no **Comparison incomplete** and no **Checking the RunPod account**.
- The cards and the list of workers do not move during C2.
- The click in C3 selects the worker of the clicked card.
- C4 shows the three cards again after the new prices arrive.
- In C5, the bottom edge of the dialog in one column is less than 40 pixels
  from its position in two columns.
- In C5, the dialog does not move after the first frame that shows it.

## 8. Cleanup

1. Close the New cloud dialog with **Cancel**.

   Result: The dialog closes. No cloud starts.

2. Close the Device panel.

   Result: The Device panel closes. The fixture continues.

3. Stop the fixture with Ctrl+C.

   Result: The fixture stops the candidate, the display and the VNC server.

   > **CAUTION:** DELETE ONLY THE DIRECTORY THAT THIS RUN GAVE TO `--state`.
   > This step deletes the saved key. Other directories can hold data of other people.

4. Delete the directory of the fixture state.

   Result: The directory and the saved key are deleted.

## 9. Record of results

Put the results in the pull request. Keep the screenshots and the recording
private, because they show account data.
