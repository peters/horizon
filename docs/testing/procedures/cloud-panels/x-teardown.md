---
procedure: cloud-panels-x-teardown
feature: Cloud panels smoke test, area X (teardown)
platforms: [linux]
cost: none
destructive: yes
secrets: [RunPod API key file, Hetzner Cloud API token file, test tailnet auth key in the Secret Service]
owner: peters
---

# Cloud panels test procedure, area X: teardown

## 1. Purpose

This area deletes every resource that the run made. It makes sure, with the
provider APIs, that no test server, pod or volume continues to cost money.

## 2. Applicability

- Candidate: the frozen candidate from area S.
- Platforms: Linux. Providers: RunPod, Hetzner and the test tailnet.
- Do this area at the end of each run, also after a failed or stopped run.
- This area does not test: a new function. It only removes test resources.

## 3. Safety

> **CAUTION:** DELETE ONLY THE RESOURCES THAT THE RESOURCE LEDGER RECORDS. Other
> workers, volumes and tailnet devices can belong to other people.

> **CAUTION:** KEEP THE PROVIDER KEYS OUT OF COMMAND ARGUMENTS. A command argument
> shows in the process list and in the shell history.

> **CAUTION:** USE ONLY READ REQUESTS ON THE PROVIDER APIS. This area deletes
> resources through Horizon only.

## 4. Equipment and preconditions

- The equipment in the [main procedure](../cloud-panels.md#4-equipment-and-preconditions).
- The resource ledger `<ledger>` and the provider baselines from the setup of
  the main procedure.
- `curl` and `jq` on the host.
- The facts in [Cloud workspaces](../../../cloud-workspaces.md#sessions-and-lifecycle)
  and [Hetzner Cloud workers](../../../cloud-hetzner.md#stop-resume-check-and-delete).

## 5. Setup

1. Find the Hetzner token file in `<data-home>/.horizon/cloud/settings.json` at `hetzner.token_file`.

   Result: You have the token file path. On the host, it starts with `<data-home>`.

2. Make a Hetzner header file with mode `0600` from the token file.

   ```sh
   umask 077; { printf 'Authorization: Bearer '; cat <hetzner-token-file>; } > <run>/hetzner.header
   ```

   Result: The header file exists. The token is not in a command argument.

3. Find the RunPod key file in `settings.json` at `runpod_key_file`.

   Result: You have the key file path.

4. Make a RunPod header file with mode `0600` from the key file.

   ```sh
   umask 077; { printf 'Authorization: Bearer '; cat <runpod-key-file>; } > <run>/runpod.header
   ```

   Result: The header file exists. The key is not in a command argument.

## 6. Tasks

### 6.1 X01 — Delete every test cloud through the UI

Do steps 1 to 8 for each cloud in the resource ledger that has an active resource.

1. On the card of the cloud, click **Delete cloud resources…**.

   Result: The card asks for confirmation and shows **Delete resources permanently**.

   > **CAUTION:** THIS STEP DELETES THE WORKER AND THE VOLUME OF THIS CLOUD. Their
   > files cannot come back.

2. Click **Delete resources permanently**.

   Result: The card shows **Deleting cloud resources**, then **Deleted**.

3. Examine the storage line of the card.

   Result: The card shows **Workspace storage cleaned up**.

4. If the card shows **Deletion failed**, record the message in the evidence.

   Result: The evidence has the message.

5. If the card shows **Deletion failed**, do steps 1 to 3 again.

   Result: The card shows **Deleted**.

6. If the card still does not show **Deleted**, stop the teardown of this cloud.

   Result: The ledger keeps the resources as active. The card keeps the retry
   action. Open a defect issue and tell the operator that the resources can cost money.

7. If the card shows **Deleted**, mark each resource of the cloud as deleted in the resource ledger.

   Result: The ledger shows no active resource for this cloud.

   > **CAUTION:** REMOVE ONLY A CLOUD THAT SHOWS **DELETED**. Horizon removes the
   > cloud and its panels from the board, and the retry action goes away.

8. If the card shows **Deleted**, click **Remove cloud**.

   Result: The cloud is not on the board.

9. Find each test cloud that never got a worker, for example `smoke-lib0` from G08.

   Result: Its card shows no worker. The ledger has no provider resource for it.

   > **CAUTION:** REMOVE ONLY THE TEST CLOUD WITHOUT A WORKER. Horizon removes the
   > cloud and its panels from the board.

10. On the card of that cloud, click **Remove cloud**.

    Result: The cloud is not on the board.

11. Examine the board.

    Result: The board shows no test cloud.

### 6.2 X02 — Make sure that Hetzner shows no test resources

1. List the servers of the Hetzner project.

   ```sh
   curl -sS -H @<run>/hetzner.header 'https://api.hetzner.cloud/v1/servers' | jq '[.servers[] | {id, name}]'
   ```

   Result: The list contains no server ID from the resource ledger.

2. List the volumes of the Hetzner project.

   ```sh
   curl -sS -H @<run>/hetzner.header 'https://api.hetzner.cloud/v1/volumes' | jq '[.volumes[] | {id, name}]'
   ```

   Result: The list contains no volume ID from the resource ledger.

3. List the SSH keys of the Hetzner project.

   ```sh
   curl -sS -H @<run>/hetzner.header 'https://api.hetzner.cloud/v1/ssh_keys' | jq '[.ssh_keys[] | {id, name}]'
   ```

   Result: The list contains no SSH key ID from the resource ledger.

4. Compare the three lists with `<evidence>/hetzner-before.json`.

   Result: Each resource in the lists was also in the baseline.

5. Save the three lists as `<evidence>/hetzner-after.json`.

   Result: The evidence shows the final Hetzner state.

### 6.3 X03 — Remove the test tailnet key and record the tailnet nodes

1. Open **Settings** and click the **Tailnets** tab.

   Result: The tab lists the test tailnet.

   > **CAUTION:** REMOVE ONLY THE TEST TAILNET. Other tailnets in the list
   > can belong to other tests.

2. On the row of the test tailnet, click **Remove**.

   Result: The test tailnet is not in the list.

3. Look for an auth key in the cloud files of the private home.

   ```sh
   grep -r -l 'tskey-' <data-home>/.horizon/cloud
   ```

   Result: The output is empty. Horizon keeps auth keys only in the Secret Service.

4. Open the admin console of the test tailnet in a browser panel.

   Result: The console lists the devices of the test tailnet.

5. Find each device whose name agrees with a tailnet line in the resource ledger.

   Result: You have a list of leftover test nodes.

6. Record the name, the last seen time and the state of each leftover test node in the evidence.

   Result: The evidence lists the leftover tailnet nodes.

7. Do not delete a device that the resource ledger does not record.

   Result: The other devices of the tailnet do not change.

### 6.4 X04 — Close the Device panel and stop the fixture

1. Send the `device_panel` operation `close` for the panel ID from task S03.

   ```json
   {"operation":"close","panel_id":"<panel-id>"}
   ```

   Result: The Device panel closes. The fixture continues.

2. Record the owned process IDs from `<state>/lab.json`.

   Result: You have the process IDs of the fixture children.

3. In the terminal of the persistent launcher, press Ctrl-C.

   Result: The launcher stops Xvfb, D-Bus, the keyring, VNC and the candidate.

4. Look for each owned process ID from step 2.

   ```sh
   ps -o pid=,comm= -p <pid-list>
   ```

   Result: The output is empty. No fixture child continues.

5. Examine `<state>/target.json`.

   Result: The file does not exist, or it shows that the target expired.

6. Examine the Horizon of the operator.

   Result: Its panels and sessions did not change.

### 6.5 X05 — Make sure that RunPod shows no test resources

1. List the pods of the RunPod account.

   ```sh
   curl -sS -H @<run>/runpod.header 'https://api.runpod.io/v2/pods' | jq '[.. | objects | select(has("id")) | {id, name}]'
   ```

   Result: The list contains no pod ID from the resource ledger.

2. List the network volumes of the RunPod account.

   ```sh
   curl -sS -H @<run>/runpod.header 'https://api.runpod.io/v2/network-volumes' | jq '[.. | objects | select(has("id")) | {id, name}]'
   ```

   Result: The list contains no network volume ID from the resource ledger.

3. Open the templates in the RunPod console.

   Result: The list contains no template that the run made.

4. Compare the lists with `<evidence>/runpod-before.json`.

   Result: Each resource in the lists was also in the baseline.

5. Save the lists as `<evidence>/runpod-after.json`.

   Result: The evidence shows the final RunPod state.

## 7. Pass criteria

- The board shows no test cloud.
- The Hetzner API shows no server, volume or SSH key from the resource ledger.
- The RunPod API shows no pod or network volume from the resource ledger.
- The provider lists agree with the baselines.
- The **Tailnets** tab does not list the test tailnet.
- The evidence lists each leftover tailnet node.
- The fixture children exited, and the target expired.

## 8. Cleanup

1. Delete the two header files.

   ```sh
   rm -f <run>/hetzner.header <run>/runpod.header
   ```

   Result: No file in `<run>` contains a provider key.

2. If a provider list still shows a resource from the ledger, record a defect and tell the operator.

   Result: The report names each resource that continues to cost money.

## 9. Record of results

Write the results in the report of the run. Use the
[report template](../../reports/TEMPLATE.md). Write only counts of resources
in the report. Keep provider IDs and tailnet node names in the private evidence.
