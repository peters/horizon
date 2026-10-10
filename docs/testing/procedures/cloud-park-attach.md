---
procedure: cloud-park-attach
feature: Parked terminals of a cloud that is out of view
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [provider API key in Cloud settings]
owner: peters
---

# Cloud park and attach test procedure

## 1. Purpose

This procedure makes sure that Horizon parks the terminals of a cloud that stays
out of view. A parked terminal has no local SSH client or terminal, it shows its
last screen as dim text, and its session continues on the worker. The procedure
also makes sure that the terminal attaches again when it comes into view, that it
then shows the output of the session, and that the first panel of a new cloud does
not move a view that went to another workspace.

## 2. Applicability

- Candidate: a build that parks cloud terminals.
- Platforms: Linux. Provider: Hetzner. RunPod is optional.
- This procedure does not test these functions:
  - Browser panels and Device panels of a cloud. Horizon does not park them.
  - The stop and the resume of a worker. The
    [stopped panel procedure](cloud-stopped-panel-restore.md) tests them.

## 3. Safety

> **CAUTION:** DELETE THE CLOUD OF THIS RUN AT THE END. The provider charges
> money until somebody deletes the worker and its storage.

> **CAUTION:** DO NOT PUT THE PROVIDER API KEY IN A SCREENSHOT OR A LOG. A
> person who gets the key can use the account.

> **CAUTION:** USE ONLY SYNTHETIC CONTENT IN THE PANELS. Screenshots and
> recordings can show the content of each panel.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The [local device smoke fixture](../../../scripts/device-smoke/README.md),
  `scripts/device-smoke/serve.py`, with `--native-view`.
- A provider API key in the Cloud settings of the test account.
- A synthetic repository with one commit and this `.horizon/cloud.yml`. The
  profile is shell-only, so no agent key is necessary.

  ```yaml
  version: 1
  default: hetzner-shell
  profiles:
    hetzner-shell:
      provider: hetzner
      image: <worker-image>
      min_cpu: 2
      min_memory_gb: 4
      storage:
        volume_gb: 10
      capabilities: {}
  ```

  `<worker-image>` is a public CPU worker image, pinned by digest.

## 5. Setup

1. Start the fixture with the frozen candidate and a new state directory.

   Result: The fixture output shows a `vnc_address`.

2. Open a Device panel with the `vnc_address`.

   Result: The Device panel shows a live view of the candidate.

3. Open **Sessions** and click **Open New Session**.

   Result: The candidate shows a new saved session. A cloud needs a saved session.

4. Open **Cloud › New cloud…**. Type the path of the synthetic repository and a
   title. Select the profile `hetzner-shell`.

   Result: The dialog shows a Hetzner offer and three passed checks.

   > **CAUTION:** START ONLY ONE CLOUD FOR THIS RUN. The provider charges money
   > from the next step until the cleanup.

5. Click **Start cloud**. While the cloud deploys, click the name of a local
   workspace in the sidebar. Wait until the cloud row in the sidebar shows that
   the cloud is ready.

   Result: The canvas stays on the local workspace: the first panel of the cloud
   opens out of view. After 2 minutes, the cloud row moves to **Parked**.

6. Click the name of the cloud workspace in the sidebar.

   Result: The canvas moves to the cloud, which attaches. The cloud workspace
   shows a shell panel.

7. In the shell panel, type this command. Then push Enter.

   ```sh
   i=0; while :; do i=$((i+1)); echo "synthetic output $i"; sleep 1; done
   ```

   Result: The panel shows a new line each second.

## 6. Tasks

### 6.1 PARK — Park a cloud that stays out of view

1. Find the process ID of the Horizon child. Count its `ssh` child processes.

   ```sh
   pgrep -P <horizon-pid> -c ssh
   ```

   Result: The count is 1 or more. Record it as ATTACHED.

2. In the sidebar, click the name of a local panel, for example `Device input test`.

   Result: The canvas moves to the local panel, and it has the focus. A click on
   empty canvas does not remove the focus from a panel, and a focused panel does
   not park.

3. If a panel of the cloud is still on the screen, drag the empty canvas until
   no panel of the cloud is on the screen.

   Result: No panel of the cloud is on the screen.

4. Wait 2 minutes and 15 seconds.

   Result: Nothing changes on the screen.

5. Count the `ssh` child processes of the Horizon child again.

   Result: The count is less than ATTACHED. The terminal client of the cloud panel
   stopped.

### 6.2 STATUS — Show the status of a parked panel

1. Drag the canvas until the cloud panel is half in view. Do the next step in
   less than 1 second.

2. Drag the canvas back until the cloud panel is out of view.

   Result: The count of `ssh` child processes does not increase. A short look does not attach the cloud.

3. Wait 15 seconds. Then drag the canvas until the panel is fully in view.
   Look at the panel at once.

   Result: The panel shows its last screen as dim text without a cursor. For
   about 1 second, the bottom of the panel shows a strip. The strip starts with
   `Parked ·` and shows the last line of the session, `synthetic output <n>`.
   Record `<n>`.

### 6.3 ATTACH — Attach the panel when it comes into view

1. Keep the cloud panel in view for 2 seconds.

   Result: The strip goes away. The panel shows new lines each second.

2. Compare the line number with `<n>` from task 6.2.

   Result: The number is larger. The session continued while the panel was parked.

3. Count the `ssh` child processes of the Horizon child.

   Result: The count is ATTACHED again.

### 6.4 FOCUS — Attach at once on focus

1. Do steps 1 to 5 of task 6.1 again.

   Result: The panel is parked.

2. In the sidebar, click the name of the cloud panel.

   Result: The canvas moves to the panel, and the panel attaches at once.

## 7. Pass criteria

- 6.1 stops the terminal client of the cloud after the grace period.
- 6.2 shows the strip with the last line, and a short look does not attach.
- 6.3 attaches again, and the session output continued while it was parked.
- 6.4 attaches at once when the panel gets focus.

## 8. Cleanup

> **CAUTION:** THIS STEP DELETES THE WORKER AND ITS STORAGE.

1. On the cloud card, open **Manage** and delete the cloud.

   Result: The card shows that the worker and the managed storage are deleted.

2. Close the Device panel. Stop the fixture with Ctrl+C in its terminal.

   Result: The fixture stops its child processes and removes its target file.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
