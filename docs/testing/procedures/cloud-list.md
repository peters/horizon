---
procedure: cloud-list
feature: The cloud list in the sidebar
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [provider API key in Cloud settings]
owner: peters
---

# Cloud list test procedure

## 1. Purpose

This procedure makes sure that the sidebar groups the workspaces in **Needs you**,
**Cloud**, **Parked** and **This PC**. It also makes sure that each row shows a
status dot, the name and a status line, that each group header shows its count
and summary on one line, and that a click on a parked row attaches its cloud.
It also makes sure that **Stop idle…** stops the selected idle workers after it
shows the hourly saving, and that a parked cloud whose session ends goes to
**Needs you**.

## 2. Applicability

- Candidate: a build that shows the cloud list in the sidebar.
- Platforms: Linux. Provider: Hetzner.
- This procedure does not test these functions:
  - The park and attach rules of a cloud. The
    [park and attach procedure](cloud-park-attach.md) tests them.
  - A cloud whose agent asks for GitHub access. The unit tests of the cloud
    list cover it.

## 3. Safety

> **CAUTION:** DELETE THE CLOUD OF THIS RUN AT THE END. The provider charges
> money until somebody deletes the worker and its storage.

> **CAUTION:** DO NOT PUT THE PROVIDER API KEY IN A SCREENSHOT OR A LOG. A
> person who gets the key can use the account.

> **CAUTION:** USE ONLY SYNTHETIC CONTENT AND GENERIC WORKSPACE NAMES.
> Screenshots and recordings show the sidebar and the panels.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The [local device smoke fixture](../../../scripts/device-smoke/README.md),
  `scripts/device-smoke/serve.py`, with `--native-view`.
- A Hetzner API key in the Cloud settings of the test account.
- A synthetic repository with one commit and the shell-only `.horizon/cloud.yml`
  of the [park and attach procedure](cloud-park-attach.md#4-equipment-and-preconditions).

## 5. Setup

1. Do steps 1 to 6 of the setup of the
   [park and attach procedure](cloud-park-attach.md#5-setup). Give the cloud a
   generic title, for example `Cloud one`.

   Result: The cloud shell prints `synthetic output <n>` each second.

2. Make the sidebar visible.

   Result: The sidebar shows the header **CLOUD 1** above the cloud workspace,
   and the header **THIS PC** with the count of the local workspaces.

## 6. Tasks

### 6.1 GROUPS: Rows and headers

1. Look at the row of the cloud workspace.

   Result: The row shows a dot, the name and, under the name, the line
   `synthetic output <n>`. The line changes about each second.

2. Look at the header line of **CLOUD**.

   Result: At the right of the header line, the hourly rate of the worker shows
   in secondary text, for example `$0.006/h`.

3. Look at the header line of **THIS PC**.

   Result: At the right of the header line, the text `live` shows.

4. Make the Horizon window 1000 pixels wide or less. The sidebar becomes
   narrower with the window.

   Result: No header summary goes to a second line. A summary that does not fit
   becomes shorter.

### 6.2 PARKED: A parked row

1. In the sidebar, click the name of a local workspace. If a panel of the cloud
   is still on the screen, drag the empty canvas until no panel of the cloud is
   on the screen.

   Result: No panel of the cloud is on the screen.

2. Wait 2 minutes and 15 seconds.

   Result: The cloud workspace moves to the group **PARKED**. Its row is compact:
   it shows a ring and the name, without a status line. The header of
   **PARKED** shows the hourly rate and `no local cost`.

3. Hold the pointer on the ring of the parked row.

   Result: A tooltip shows `Parked` and the last line of the session.

### 6.3 ATTACH: Attach from the list

1. Click the name of the parked row.

   Result: The canvas moves to the cloud workspace. In about 1 second, the cloud
   attaches. The row moves back to **CLOUD** and shows the status line again.

### 6.4 STOP: Stop idle workers

1. Make sure that no agent works in the cloud. Look at the header line of
   **CLOUD**.

   Result: The header shows **Stop idle…** after the count.

2. Click **Stop idle…**.

   Result: A dialog shows the cloud with a selected check box, its workspace and
   its hourly rate, the line `Saves $<rate>/h`, and the buttons **Stop 1 worker**
   and **Keep running**.

3. Clear the check box of the cloud.

   Result: The saving line goes away. **Stop 0 workers** is not available.

4. Click **Keep running**.

   Result: The dialog closes. The worker continues to run.

5. Click **Stop idle…** again, and then click **Stop 1 worker**.

   Result: The dialog closes. The cloud card shows that the worker stops, and
   then that it is stopped. The row moves to **PARKED** with a ring. The header
   of **PARKED** shows no **Stop idle…**.

### 6.5 NEEDS YOU: A parked session ends

1. On the cloud card, click **Resume**. Wait until the card shows **Ready**.

   Result: The cloud shell shows a prompt.

2. Click in the cloud shell. Type this command and push Enter.

   ```sh
   sleep 240; exit 3
   ```

   Result: The shell does not show a prompt.

3. Do step 1 of task 6.2. Wait 2 minutes and 15 seconds.

   Result: The cloud workspace moves to the group **PARKED**.

4. Wait 2 more minutes.

   Result: The cloud workspace moves to the group **NEEDS YOU**. Its row shows a
   yellow dot, the name and the line `Ended with status 3`.

5. Hold the pointer on the yellow dot.

   Result: A tooltip shows `Waiting for you`.

## 7. Pass criteria

- 6.1 shows the rows, the counts and the summaries on one header line.
- 6.2 moves the parked cloud to **PARKED** with a compact row.
- 6.3 attaches the cloud from a click on its row.
- 6.4 shows the hourly saving before the confirmation and stops only the selected
  worker.
- 6.5 moves the parked cloud whose session ended to **NEEDS YOU** with the line
  `Ended with status 3`.

## 8. Cleanup

> **CAUTION:** THIS STEP DELETES THE WORKER AND ITS STORAGE.

1. On the cloud card, open **Manage** and delete the cloud. After 6.4, the worker
   is stopped: the deletion removes the workspace volume.

   Result: The card shows that the worker and the managed storage are deleted.

2. Close the Device panel. Stop the fixture with Ctrl+C in its terminal.

   Result: The fixture stops its child processes and removes its target file.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
