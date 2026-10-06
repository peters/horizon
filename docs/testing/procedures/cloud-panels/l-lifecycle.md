---
procedure: cloud-panels-l-lifecycle
feature: Cloud panels smoke test, area L (stop, resume, reconnect, restart, reconcile, rebuild, resize, idle stop and delete)
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [RunPod API key in Cloud settings, Hetzner Cloud API token in Cloud settings, registry push and pull credentials]
owner: peters
---

# Cloud panels test procedure, area L: lifecycle

## 1. Purpose

This area makes sure that the lifecycle actions of a cloud keep its data and its
identity. It covers stop, resume, reconnect, a restart of Horizon, the provider
check, the image rebuild, resize, idle stop, deletion and redeploy.

## 2. Applicability

- Candidate: the frozen candidate from area S.
- Platforms: Linux. Clouds: `smoke-a`, `smoke-r`, `smoke-sib` and `smoke-x`.
- This area does not test: a lost worker or an uncertain provider answer. The
  [cloud workspaces smoke guide](../../cloud-workspaces-mvp-smoke.md#disconnect-restart-and-failure-recovery)
  covers those cases with a provider simulator.

## 3. Safety

> **CAUTION:** DO THIS AREA AFTER AREAS T, G AND N. This area stops and deletes
> clouds that those areas use.

> **CAUTION:** STOP, DELETE AND REMOVE ONLY THE CLOUD THAT THE TASK NAMES. If you
> select another cloud, other people can lose their work.

> **CAUTION:** WRITE EACH NEW PROVIDER RESOURCE IN THE RESOURCE LEDGER. A Hetzner
> resume, a rebuild and a redeploy can make a new server or volume.

## 4. Equipment and preconditions

- The equipment in the [main procedure](../cloud-panels.md#4-equipment-and-preconditions).
- `smoke-a`, `smoke-r` and `smoke-sib` show **Ready**.
- `<repo>` has a committed profile `hetzner-idle`. It is image-only, uses
  `provider: hetzner` and sets `idle_stop_minutes: 10`.
- The profile of `smoke-sib` has a `build` section. The registry push credential
  is in Cloud settings.
- The facts in [Cloud workspaces](../../../cloud-workspaces.md#sessions-and-lifecycle)
  and [Hetzner Cloud workers](../../../cloud-hetzner.md#stop-resume-check-and-delete).

## 5. Setup

1. Open a Shell panel in `smoke-a`.

   Result: The Shell panel shows a prompt in the shared checkout.

2. Write a marker file with a random value on the workspace volume.

   ```sh
   head -c 8 /dev/urandom | od -An -tx1 | tr -d ' \n' > ~/lifecycle-marker; cat ~/lifecycle-marker
   ```

   Result: The panel shows the random value. Record it in the evidence.

3. Do steps 1 and 2 again in `smoke-r`.

   Result: Each cloud has its own marker value in the evidence.

## 6. Tasks

### 6.1 L01 — Stop and resume a worker

Use `smoke-a`.

1. On the card of `smoke-a`, click **Stop worker…**.

   Result: The card asks for a confirmation and shows **Stop worker** and
   **Keep running**. On Hetzner, the text says that the server is deleted and
   that the workspace volume is kept and stays billable.

   > **CAUTION:** STOP ONLY `smoke-a`. The processes on the worker stop. On
   > Hetzner, the stop deletes the server and keeps the volume.

2. Click **Stop worker**.

   Result: The card shows **Stopped**. The card says that storage can remain billable.

3. Mark the Hetzner server of `smoke-a` as deleted in the resource ledger.

   Result: The ledger shows the old server as deleted and the volume as kept.

   > **CAUTION:** THIS STEP RENTS COMPUTE. On Hetzner, the resume makes a new
   > server that costs money.

4. Click **Resume worker**.

   Result: The card shows the stages until **Ready**.

5. If the card shows **Reconnect cloud** and not **Ready**, click **Reconnect cloud**.

   Result: The card shows **Ready**.

6. Write the new Hetzner server of `smoke-a` in the resource ledger.

   Result: The ledger has a line for the new server.

7. In a Shell panel of `smoke-a`, show the marker file.

   ```sh
   cat ~/lifecycle-marker
   ```

   Result: The value is the same as the value in the evidence.

### 6.2 L02 — Reconnect a cloud and keep its sessions

1. In the Shell panel of `smoke-a`, start a counter that writes the time each second.

   ```sh
   while true; do date +%s > ~/l02-counter; sleep 1; done
   ```

   Result: The panel shows no prompt. The counter runs.

2. In a second Shell panel, show the tmux session and pane of the first panel.

   ```sh
   tmux list-panes -a -F '#{session_name} #{pane_id} #{pane_pid}'
   ```

   Result: The output lists the sessions. Record them in the evidence.

3. On the card of `smoke-a`, click **Reconnect cloud**.

   Result: The card shows the reconnect stages, then **Ready**.

4. In the second Shell panel, show the tmux sessions again.

   Result: The sessions, pane IDs and process IDs are the same as in step 2.

5. Show the counter file two times, 3 seconds apart.

   ```sh
   cat ~/l02-counter
   ```

   Result: The value increases. The counter did not stop.

6. Examine the first Shell panel.

   Result: The panel shows the same session. It accepts Ctrl-C.

### 6.3 L03 — Restart Horizon and reattach the cloud

1. Record the tmux sessions of `smoke-a` as in L02 step 2.

   Result: The evidence has the sessions before the restart.

2. On the host, make the restart marker.

   ```sh
   touch <state>/restart-request
   ```

   Result: The file `<state>/restart-request` exists.

3. Close the candidate window with the close button of the window manager.

   Result: The candidate stops. The launcher deletes the marker and starts the
   candidate again.

4. Do task S03 step 4 of [area S](s-test-fixture.md) again.

   Result: The Device panel shows a live view of the new candidate window.

5. Do task S04 of area S again.

   Result: The new child has the frozen SHA-256. Record the new process ID.

6. Find the card of `smoke-a`.

   Result: The card shows the cloud with the same title and panels.

7. If the card shows **Reconnect cloud**, click **Reconnect cloud**.

   Result: The card shows **Ready**.

8. In a Shell panel of `smoke-a`, show the tmux sessions.

   Result: The sessions are the same as in step 1.

### 6.4 L04 — Reconcile the worker ID with the provider

1. Record the worker ID from the **Machine** tab of `smoke-a`.

   Result: You have the worker ID.

2. Open the **Manage** tab of the card and click **Check provider**.

   Result: The card shows **Checking provider**, then the result.

3. Examine the result.

   Result: The card shows the same worker ID and **Ready**. Nothing starts or stops.

4. In the fixture terminal, run the reconcile command of the `cloud_deploy` example.

   ```sh
   <run>/bin/cloud_deploy reconcile <home>/.horizon/cloud/settings.json <home>/.horizon/cloud/<cloud-id>
   ```

   Result: The output gives a structured outcome with the same worker ID. It
   contains no key and no environment of the worker.

### 6.5 L05 — Rebuild the image, and cancel a rebuild

Use `smoke-sib`. Its profile has a `build` section.

> **CAUTION:** THE REBUILD PUSHES AN IMAGE AND RESTARTS THE WORKER. The processes
> on `smoke-sib` stop.

1. On the card of `smoke-sib`, click **Rebuild image & restart…**.

   Result: The card asks for confirmation and names the consequences.

   > **CAUTION:** THIS STEP STARTS A BUILD AND A PUSH. If you do not cancel in
   > time, the worker restarts on the new image.

2. Click **Rebuild and restart**.

   Result: The card lists the steps of the rebuild with their durations.

3. While the card reads the committed recipe, click **Cancel rebuild**.

   Result: The rebuild stops. No rebuild step waits to continue. The card offers
   **Reconnect cloud**.

4. On the card, click **Reconnect cloud**.

   Result: The card shows **Ready** on the previous image.

5. Record the worker ID from the **Machine** tab.

   Result: You have the worker ID before the rebuild.

6. On the card, click **Rebuild image & restart…**.

   Result: The card asks for confirmation.

   > **CAUTION:** THIS STEP PUSHES AN IMAGE AND RESTARTS THE WORKER. The
   > container disk is reset.

7. Click **Rebuild and restart**.

   Result: The card shows the build, the push and the image switch.

8. Wait until the card shows **Ready**.

   Result: The card names the new image, or it says that the image did not change.

9. Examine the worker ID on the **Machine** tab.

   Result: On RunPod, the worker ID is the same as in step 5.

10. In a Shell panel of `smoke-sib`, show the checkout status.

    ```sh
    git status --short; git log -1 --format=%H
    ```

    Result: The worktree is the same as before the rebuild.

### 6.6 L06 — Resize compute and grow the workspace

Use `smoke-r`. It is a RunPod CPU cloud.

1. On the card of `smoke-r`, click **Resize compute…**.

   Result: The card shows the vCPU and memory choices.

2. Select a larger vCPU count that is in stock.

   Result: The card shows the new size and its price.

3. Click **Review resize…**.

   Result: The card names the consequences: processes stop and charges change.

   > **CAUTION:** THIS STEP REPLACES THE WORKER AND CHANGES THE CHARGES. The
   > provider charges the new price for each hour.

4. Click **Confirm resize**.

   Result: The card shows the resize stages, then **Ready**.

5. Write the new pod of `smoke-r` in the resource ledger.

   Result: The ledger shows the old pod as deleted and the new pod as active.

6. In a Shell panel of `smoke-r`, show the marker file and the CPU count.

   ```sh
   cat ~/lifecycle-marker; nproc
   ```

   Result: The marker value is the same. `nproc` shows the new vCPU count.

7. On the card of `smoke-r`, click **Grow workspace…**.

   Result: The card shows **New workspace size**.

8. Type a size that is 10 GB larger than the current size.

   Result: The card shows the new size and the new storage price.

   > **CAUTION:** THE WORKSPACE CANNOT SHRINK AGAIN. The larger volume costs more
   > money each month.

9. Click **Confirm resize**.

   Result: The card shows **Ready**. The worker does not change.

10. In the Shell panel, show the size of `/workspace`.

    ```sh
    df -h /workspace
    ```

    Result: The size is the new size.

### 6.7 L07 — Make sure that idle stop works

1. Open **Cloud › New cloud…** and wait 3 seconds.

   Result: The New cloud dialog opens and does not move.

2. Type `smoke-x` in the title field.

   Result: The title field shows `smoke-x`.

3. In **Profile**, select `hetzner-idle`.

   Result: The summary shows a Hetzner worker.

4. In **Tailnet**, select **None**.

   Result: **None** is selected.

   > **CAUTION:** THIS STEP RENTS COMPUTE. Hetzner charges money for the server
   > and the volume until somebody deletes them.

5. Click **Start cloud**.

   Result: The card of `smoke-x` shows **Ready**.

6. Write the server, the volume and the SSH key of `smoke-x` in the resource ledger.

   Result: The ledger has three lines for `smoke-x`.

7. Do not open a panel in `smoke-x`. Keep the candidate open and connected.

   Result: The worker has no agent output and no CPU load.

8. Wait 15 minutes.

   Result: The card of `smoke-x` shows **Stopped**.

9. Mark the server of `smoke-x` as deleted in the resource ledger.

   Result: The ledger shows the server as deleted and the volume as kept.

10. Open the **Manage** tab and click **Check provider**.

    Result: The card shows **Stopped** and offers **Resume worker**. Nothing
    starts the worker.

### 6.8 L08 — Delete, remove and redeploy a cloud

Use `smoke-x` from L07.

1. On the card of `smoke-x`, click **Delete cloud resources…**.

   Result: The card asks for confirmation and shows **Delete resources permanently**.

   > **CAUTION:** THIS STEP DELETES THE VOLUME OF `smoke-x`. Its files cannot
   > come back.

2. Click **Delete resources permanently**.

   Result: The card shows **Deleted** and **Workspace storage cleaned up**.

3. Mark the volume and the SSH key of `smoke-x` as deleted in the resource ledger.

   Result: The ledger shows no active resource for `smoke-x`.

4. Examine the card actions.

   Result: The card shows **Redeploy cloud…** and **Remove cloud**.

5. Click **Redeploy cloud…**.

   Result: The card asks for confirmation.

   > **CAUTION:** THIS STEP RENTS COMPUTE. The redeploy makes a new server and a
   > new volume.

6. Click **Redeploy cloud**.

   Result: The card shows the stages, then **Ready**.

7. Write the new server, volume and SSH key of `smoke-x` in the resource ledger.

   Result: The ledger has three new lines for `smoke-x`.

8. Click **Delete cloud resources…**.

   Result: The card shows **Delete resources permanently**.

   > **CAUTION:** THIS STEP DELETES THE NEW VOLUME OF `smoke-x`. Its files cannot
   > come back.

9. Click **Delete resources permanently**.

   Result: The card shows **Deleted**.

10. Mark the new resources of `smoke-x` as deleted in the resource ledger.

    Result: The ledger shows no active resource for `smoke-x`.

    > **CAUTION:** REMOVE ONLY `smoke-x`. Horizon removes the cloud and its panels
    > from the board.

11. Click **Remove cloud**.

    Result: The cloud `smoke-x` is not on the board.

### 6.9 L09 — Resume a Hetzner cloud on a new server

Use `smoke-a`.

1. Get the endpoint of `smoke-a` as in task E09 step 2 of [area E](e-panels.md).

   Result: You have `known_hosts` and `host_key_alias`.

2. Record the host key fingerprint.

   ```sh
   ssh-keygen -l -F "<host_key_alias>" -f "<known_hosts>"
   ```

   Result: The evidence has the fingerprint before the stop.

3. Record the server ID from the **Machine** tab.

   Result: The evidence has the server ID before the stop.

4. On the card, click **Stop worker…**.

   Result: The card asks **Stop this worker?**.

   > **CAUTION:** STOP ONLY `smoke-a`. The stop deletes the server and keeps the volume.

5. Click **Stop worker**.

   Result: The card shows **Stopped**.

6. Mark the old server as deleted in the resource ledger.

   Result: The ledger shows the old server as deleted.

   > **CAUTION:** THIS STEP RENTS COMPUTE. The resume makes a new server.

7. Click **Resume worker**.

   Result: The card shows the stages until **Ready**.

8. If the card shows **Reconnect cloud** and not **Ready**, click **Reconnect cloud**.

   Result: The card shows **Ready**.

9. Write the new server in the resource ledger.

   Result: The **Machine** tab shows a server ID that is not the old ID.

10. Show the marker file in a Shell panel.

    ```sh
    cat ~/lifecycle-marker
    ```

    Result: The value is the same as the value in the evidence.

11. Do steps 1 and 2 again.

    Result: The fingerprint is the same as before. The worker restores its host
    key from `/workspace`, and Horizon pins it for the new server.

12. Connect as in task E09 step 4.

    Result: SSH connects with the pinned host key and shows no prompt.

### 6.10 L10 — Stop and resume a RunPod cloud

Use `smoke-r`.

1. Record the pod ID and the network volume ID from the **Machine** tab.

   Result: The evidence has both IDs.

2. In a Shell panel of `smoke-r`, record the commit of the checkout.

   ```sh
   git rev-parse HEAD
   ```

   Result: The evidence has the commit.

3. On the card, click **Stop worker…**.

   Result: The card asks **Stop this worker?**.

   > **CAUTION:** STOP ONLY `smoke-r`. The network volume continues to cost money
   > while the pod is stopped.

4. Click **Stop worker**.

   Result: The card shows **Stopped**.

   > **CAUTION:** THIS STEP RENTS COMPUTE. RunPod charges money for the pod while
   > it runs.

5. Click **Resume worker**.

   Result: The card shows the stages until **Ready**.

6. Examine the **Machine** tab.

   Result: The pod ID and the network volume ID are the same as in step 1.

7. Show the marker file and the commit.

   ```sh
   cat ~/lifecycle-marker; git rev-parse HEAD
   ```

   Result: The marker value and the commit are the same as before the stop.

## 7. Pass criteria

- The marker files survive stop, resume, resize and the Hetzner resume.
- Reconnect and the restart of Horizon keep the same tmux sessions.
- The restarted candidate child has the frozen SHA-256.
- **Check provider** and `cloud_deploy reconcile` show the same worker ID.
- After **Cancel rebuild**, no rebuild step waits. A full rebuild reaches **Ready**.
- Idle stop stops `smoke-x` after the profile limit.
- Deletion removes the resources, and redeploy makes new resources.
- A Hetzner resume makes a new server. The host key from `/workspace` stays, and
  pinned SSH works.
- A RunPod resume keeps the pod ID, the network volume and the checkout.

## 8. Cleanup

1. Stop the counter of L02 with Ctrl-C in its Shell panel.

   Result: The Shell panel shows a prompt.

2. Make sure that the resource ledger shows each deleted resource as deleted.

   Result: Each active line in the ledger belongs to a cloud on the board.

L05 is the last task that uses the `<build-repository>` entry of A08. Steps 3
to 9 revoke it while the fixture runs.

3. Open **Cloud › Cloud settings…** and wait 3 seconds.

   Result: The **Container registry** card lists `<build-repository>` from A08.

   > **CAUTION:** REVOKE ONLY THE `<build-repository>` ENTRY OF THIS RUN. A worker
   > that needs this credential cannot pull its image after a restart.

4. On the `<build-repository>` entry, click **Revoke pull binding**.

   Result: The entry shows that the provider pull credential is revoked.

5. Write the status action for `<build-repository>` as in A08 step 12.

   Result: `<data-home>/smoke/registry-status.json` names `<build-repository>` and its generation.

6. Run the status action with the CLI.

   ```sh
   <run>/bin/cloud_deploy registry <home>/.horizon/cloud/settings.json <home>/smoke/registry-status.json
   ```

   Result: The output shows that the provider pull credential is revoked.

   > **CAUTION:** REVOKE ONLY THE TWO TOKENS THAT THIS RUN MADE FOR `<build-repository>`.
   > Other tokens of the registry can give access to other work.

7. Ask the operator to revoke the pull token and the push token of `<build-repository>` at the registry.

   Result: The tokens no longer give access. Horizon does not revoke a token at its issuer.

8. Show the registry entries in the settings file.

   ```sh
   jq '[.registries.bindings[]? | .repository]' <data-home>/.horizon/cloud/settings.json
   ```

   Result: Each listed entry has a revoked pull credential. Record the list in the evidence.

9. Delete `<data-home>/smoke/registry-status.json`.

   Result: No registry action file of this run stays in the private home.

Area X deletes the other clouds.

## 9. Record of results

Write the results in the report of the run. Use the
[report template](../../reports/TEMPLATE.md). Keep provider IDs and host key
fingerprints in the private evidence only.
