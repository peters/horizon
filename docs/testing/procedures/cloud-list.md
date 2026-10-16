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
tile with a status badge, the name and a status line, that each group header shows its count
and summary on one line, and that a click on a parked row attaches its cloud.
It also makes sure that an agent in the cloud workspace can read the list and
attach and park its cloud with `cloud_list` but cannot stop it, that the sidebar
offers no bulk stop, and that a parked cloud whose session ends goes to
**Needs you**.

## 2. Applicability

- Candidate: a build that shows the cloud list in the sidebar.
- Platforms: Linux. Provider: Hetzner.
- This procedure does not test these functions:
  - The park and attach rules of a cloud. The
    [park and attach procedure](cloud-park-attach.md) tests them.
  - A cloud whose agent asks for GitHub access. The unit tests of the cloud
    list cover it.
  - A request from an agent in a different workspace. The unit tests of
    `cloud_list` cover it.

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
- An agent panel kind (Claude Code or Codex) and `horizon-browser` from the
  candidate on the `PATH` of Horizon. The agent panel runs the commands of task
  6.4 in its shell.

## 5. Setup

1. Do steps 1 to 7 of the setup of the
   [park and attach procedure](cloud-park-attach.md#5-setup). Give the cloud a
   generic title, for example `Cloud one`.

   Result: The cloud shell prints `synthetic output <n>` each second.

2. Make the sidebar visible.

   Result: The sidebar shows the header **Cloud 1** above the cloud workspace,
   and the header **This PC** with the count of the local workspaces.

## 6. Tasks

### 6.1 GROUPS: Rows and headers

1. Look at the row of the cloud workspace.

   Result: The row shows a tile with the initials of the workspace name, the
   name and, under the name, the line
   `synthetic output <n>`. The line changes about each second.

2. Look at the header line of **Cloud**.

   Result: At the right of the header line, the hourly rate of the worker shows
   in secondary text in euros, for example `€0.0088/h`.

3. Look at the header line of **This PC**.

   Result: At the right of the header line, the text `live` shows.

4. Make the Horizon window 1000 pixels wide or less. The sidebar becomes
   narrower with the window.

   Result: No header summary goes to a second line. A summary that does not fit
   becomes shorter. If no part fits, the summary goes away.

### 6.2 PARKED: A parked row

1. In the sidebar, click the name of a local workspace. If a panel of the cloud
   is still on the screen, drag the empty canvas until no panel of the cloud is
   on the screen.

   Result: No panel of the cloud is on the screen.

2. Wait 2 minutes and 15 seconds.

   Result: The cloud workspace moves to the group **Parked**. Its row is compact:
   it shows a small tile with a ring badge and the name, without a status line.
   The header of **Parked** shows the hourly rate and `no local cost`.

3. Hold the pointer on the tile of the parked row.

   Result: A tooltip shows `Parked` and the last line of the session.

### 6.3 ATTACH: Attach from the list

1. Click the name of the parked row.

   Result: The canvas moves to the cloud workspace. In about 1 second, the cloud
   attaches. The row moves back to **Cloud** and shows the status line again.

### 6.4 AGENT: The cloud list from an agent

1. Add an agent panel to the cloud workspace, outside the cloud card. In the
   agent panel, run this command.

   ```sh
   horizon-browser cloud list -o -
   ```

   Result: The JSON report shows one cloud with `"group": "cloud"`,
   `"working": false` and the status line `synthetic output <n>`. The report has
   no `"idle"` field. Write down the value of `"cloud"`, for example `cloud-1`.

2. Make sure that a panel of the cloud is on the screen. Run
   `horizon-browser cloud park <cloud> -o -`.

   Result: The report shows the error `cloud_list_in_view`. The cloud stays
   attached.

3. Drag the empty canvas until only the agent panel is on the screen. Run the
   park command again.

   Result: The report shows `"park": "parking"`. In about 1 second, the row of
   the cloud moves to **Parked**.

4. Run `horizon-browser cloud attach <cloud> -o -`.

   Result: The report shows `"attach": "in_view"`. The canvas moves to the
   cloud, and in about 1 second the cloud attaches. The row moves back to
   **Cloud**.

### 6.5 STOP: Only the card stops the worker

1. Make sure that no agent works in the cloud. Look at the header line of
   **Cloud**.

   Result: The header shows the name, the count and the summary. It shows no
   **Stop idle…**.

2. In the agent panel, run `horizon-browser cloud stop <cloud> -o -`.

   Result: The command fails with `cloud requires list, attach or park`. The
   worker continues to run.

3. On the cloud card, click **Stop**.

   Result: The cloud card shows that the worker stops, and then that it is
   stopped. The row moves to **Parked** with a ring badge.

### 6.6 NEEDS YOU: A parked session ends

1. On the cloud card, click **Resume**. Wait until the card shows **Ready**.

   Result: The cloud shell shows a prompt.

2. Click in the cloud shell. Type this command and push Enter.

   ```sh
   sleep 240; exit 3
   ```

   Result: The shell does not show a prompt.

3. Do step 1 of task 6.2. Wait 2 minutes and 15 seconds.

   Result: The cloud workspace moves to the group **Parked**.

4. Wait 2 more minutes.

   Result: The cloud workspace moves to the group **Needs you**. Its tile shows
   a yellow badge. The row shows the name and the line `Ended with status 3`.

5. Hold the pointer on the tile of the row.

   Result: A tooltip shows `Waiting for you`.

## 7. Pass criteria

- 6.1 shows the rows, the counts and the summaries on one header line.
- 6.2 moves the parked cloud to **Parked** with a compact row.
- 6.3 attaches the cloud from a click on its row.
- 6.4 lists the cloud for the agent, refuses to park it in view, parks it out of
  view and attaches it again.
- 6.5 shows no bulk stop in the sidebar, refuses a stop from the agent, and stops
  the worker from its card.
- 6.6 moves the parked cloud whose session ended to **Needs you** with the line
  `Ended with status 3`.

## 8. Cleanup

> **CAUTION:** THIS STEP DELETES THE WORKER AND ITS STORAGE.

1. On the cloud card, open **Manage** and delete the cloud. After 6.5, the worker
   is stopped: the deletion removes the workspace volume.

   Result: The card shows that the worker and the managed storage are deleted.

2. Close the Device panel. Stop the fixture with Ctrl+C in its terminal.

   Result: The fixture stops its child processes and removes its target file.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
