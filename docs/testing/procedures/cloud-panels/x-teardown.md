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

1. Make sure that the header files and the list scripts of the main setup exist.

   ```sh
   ls -l <run>/hetzner.header <run>/runpod.header <run>/hetzner-list.sh <run>/runpod-list.sh
   ```

   Result: The four files exist. The header files have the mode `-rw-------`.

## 6. Tasks

### 6.1 X01 — Delete every test cloud through the UI

First make the resource ledger complete. A run can stop after the provider
accepts a request but before the card shows an ID, for example during the watch of C08.

> **CAUTION:** SEND THE PROVIDER CREDENTIALS ONLY TO THE PROVIDER APIS. The header
> files contain them. Do not show the files or the request headers.

1. List the current Hetzner and RunPod resources in the format of the baselines.

   ```sh
   ( set -e; for k in servers volumes ssh_keys networks; do bash <run>/hetzner-list.sh "$k"; done
     for k in pods network-volumes registries templates; do bash <run>/runpod-list.sh "$k"; done ) > <evidence>/before-teardown.jsonl
   ```

   Result: The file has one line for each current provider resource.
   If an API call fails, the command stops with an error. Then do this step again.

2. Compare the list with the two baselines.

   ```sh
   diff <(sort <evidence>/hetzner-before.jsonl <evidence>/runpod-before.jsonl) <(sort <evidence>/before-teardown.jsonl)
   ```

   Result: You have the new resources since the setup. Some can belong to other people.

3. For each new resource that is not in the ledger, find its ID on a test cloud card.

   Result: A resource belongs to this run only when a card or the state directory
   of a test cloud of this run shows its ID. A name alone is not enough.

4. Write each new resource of this run in the resource ledger.

   Result: The ledger contains every provider resource of this run.

5. Record each new resource that you cannot connect to this run in the evidence.

   Result: The evidence lists the resources that this run does not own.

6. Tell the operator about each of these resources.

   Result: The teardown does not delete them. The operator decides what to do with them.

7. Examine the card of each planned test cloud on the board.

   Result: You know which cards show a worker, a volume or an operation that waits.

Do steps 8 to 15 for each cloud in the resource ledger that has an active resource.

8. On the card of the cloud, click **Delete cloud resources…**.

   Result: The card asks for confirmation and shows **Delete resources permanently**.

   > **CAUTION:** THIS STEP DELETES THE WORKER AND THE VOLUME OF THIS CLOUD. Their
   > files cannot come back.

9. Click **Delete resources permanently**.

   Result: The card shows **Deleting cloud resources**, then **Deleted**.

10. Examine the storage line of the card.

    Result: The card shows **Workspace storage cleaned up**.

11. If the card shows **Deletion failed**, record the message in the evidence.

    Result: The evidence has the message.

12. If the card shows **Deletion failed**, do steps 8 to 10 again.

    Result: The card shows **Deleted**.

13. If the card still does not show **Deleted**, stop the teardown of this cloud.

    Result: The ledger keeps the resources as active. The card keeps the retry
    action. Open a defect issue and tell the operator that the resources can cost money.

14. If the card shows **Deleted**, mark the worker, storage and SSH key resources of the cloud as deleted.

    Result: The ledger shows no active provider resource for this cloud. Its
    tailnet line stays active until X03 removes the node.

    > **CAUTION:** REMOVE ONLY A CLOUD THAT SHOWS **DELETED**. Horizon removes the
    > cloud and its panels from the board, and the retry action goes away.

15. If the card shows **Deleted**, click **Remove cloud**.

    Result: The cloud is not on the board.

16. Find each test cloud that never got a worker, for example `smoke-lib0` from G08.

    Result: Its card shows no worker. The ledger has no provider resource for it.

    > **CAUTION:** REMOVE ONLY THE TEST CLOUD WITHOUT A WORKER. Horizon removes the
    > cloud and its panels from the board.

17. On the card of that cloud, click **Remove cloud**.

    Result: The cloud is not on the board.

18. Examine the board.

    Result: The board shows no test cloud.

19. Find each RunPod registry credential that the resource ledger shows as active.

    Result: You have the list. A run that stopped before the cleanup of area A or L can leave one.

    > **CAUTION:** REVOKE ONLY THE REGISTRY CREDENTIALS THAT THE LEDGER RECORDS. A
    > worker that needs another credential cannot pull its image after a restart.

20. For each listed credential, click **Revoke pull binding** on its entry in **Cloud settings…**.

    Result: Each entry shows that the provider pull credential is revoked.

    > **CAUTION:** SEND THE RUNPOD KEY ONLY TO THE RUNPOD API. The header file
    > contains the key. Do not show the file or the request headers.

21. List the registry credentials of the RunPod account.

    ```sh
    bash <run>/runpod-list.sh registries
    ```

    Result: The list contains no ID from step 19.

22. Mark each credential from step 19 as deleted in the resource ledger.

    Result: The ledger shows no active RunPod registry credential.

23. Find each issuer token that the resource ledger shows as active.

    Result: You have the list. A run that stopped before A08, A09 or the cleanup of area L can leave one.

    > **CAUTION:** REVOKE ONLY THE ISSUER TOKENS THAT THE LEDGER RECORDS. Other
    > tokens of the accounts can give access to other work.

24. Ask the operator to revoke each listed token at its issuer.

    Result: The tokens no longer give access.

25. Mark each of these tokens as revoked in the resource ledger.

    Result: The ledger shows no active issuer token.

### 6.2 X02 — Make sure that Hetzner shows no test resources

> **CAUTION:** SEND THE HETZNER TOKEN ONLY TO THE HETZNER API. The header file
> contains the token. Do not show the file or the request headers.

1. List the servers of the Hetzner project.

   ```sh
   bash <run>/hetzner-list.sh servers
   ```

   Result: The script reads all pages. The list contains no server ID from the resource ledger.

> **CAUTION:** SEND THE HETZNER TOKEN ONLY TO THE HETZNER API. The header file
> contains the token. Do not show the file or the request headers.

2. List the volumes of the Hetzner project.

   ```sh
   bash <run>/hetzner-list.sh volumes
   ```

   Result: The list contains no volume ID from the resource ledger.

> **CAUTION:** SEND THE HETZNER TOKEN ONLY TO THE HETZNER API. The header file
> contains the token. Do not show the file or the request headers.

3. List the SSH keys of the Hetzner project.

   ```sh
   bash <run>/hetzner-list.sh ssh_keys
   ```

   Result: The list contains no SSH key ID from the resource ledger.

> **CAUTION:** SEND THE HETZNER TOKEN ONLY TO THE HETZNER API. The header file
> contains the token. Do not show the file or the request headers.

4. Save all Hetzner lists in the format of the baseline.

   ```sh
   ( set -e; for k in servers volumes ssh_keys networks; do bash <run>/hetzner-list.sh "$k"; done ) > <evidence>/hetzner-after.jsonl
   ```

   Result: The evidence shows the final Hetzner state.
   If an API call fails, the command stops with an error. Then do this step again.

5. Compare the final Hetzner state with the baseline.

   ```sh
   diff <(sort <evidence>/hetzner-before.jsonl) <(sort <evidence>/hetzner-after.jsonl)
   ```

   Result: Each new line is a kept Hetzner network of Horizon or a resource that
   X01 step 5 records as not owned by this run.

### 6.3 X03 — Remove the test tailnet key and the test tailnet nodes

1. Open **Settings**.

   Result: The Settings window opens.

2. Click the **Tailnets** tab.

   Result: The tab lists the test tailnet.

   > **CAUTION:** REMOVE ONLY THE TEST TAILNET. Other tailnets in the list
   > can belong to other tests.

3. On the row of the test tailnet, click **Remove**.

   Result: The test tailnet is not in the list.

4. Look for an auth key in the cloud files of the private home.

   ```sh
   grep -r -l 'tskey-' <data-home>/.horizon/cloud
   ```

   Result: The output is empty. Horizon keeps auth keys only in the Secret Service.

5. Open the admin console of the test tailnet in a browser panel.

   Result: The console lists the devices of the test tailnet.

6. Find each device whose node ID agrees with a tailnet line in the resource ledger.

   Result: You have the list of leftover test nodes. A node can have a new name
   after a resume ([issue #1310](https://github.com/peters/horizon/issues/1310)), so use the node ID.

7. Record the node ID, name, last seen time and state of each leftover test node.

   Result: The evidence lists the leftover tailnet nodes.

   > **CAUTION:** REMOVE ONLY THE NODE IDS THAT THE RESOURCE LEDGER RECORDS. Other
   > devices of the tailnet can belong to other people, and their access stops.

8. In the admin console, remove each leftover test node.

   Result: The console does not list the node IDs of the ledger.

9. Mark each tailnet line of the resource ledger as deleted.

   Result: The ledger shows no active tailnet node.

10. Compare the device list with the tailnet baseline of area T.

    Result: The list is the same as the baseline. Each device that the ledger does not record is unchanged.

### 6.4 X04 — Close the Device panel and stop the fixture

1. Send the `device_panel` operation `close` for the panel ID from task S03.

   ```json
   {"operation":"close","panel_id":"<panel-id>"}
   ```

   Result: The Device panel closes. The fixture continues.

2. Record the owned process IDs from `<state>/lab.json`.

   Result: You have the process IDs of the fixture children from the first start.

3. Add the newest candidate child process ID to the list.

   ```sh
   pstree -p <launcher-pid> | grep -o 'horizon([0-9]*)' | tr -dc '0-9\n'
   ```

   Result: The list also contains the candidate child after the restarts of N05 and L03.

4. In the terminal of the persistent launcher, press Ctrl-C.

   Result: The launcher stops Xvfb, D-Bus, the keyring, VNC and the candidate.

5. Look for each process ID from steps 2 and 3.

   ```sh
   ps -o pid=,comm= -p <pid-list>
   ```

   Result: The output is empty. No fixture child continues.

6. Examine `<state>/target.json`.

   Result: The file does not exist, or it shows that the target expired.

7. Examine the Horizon of the operator.

   Result: Its panels and sessions did not change.

8. Look for the state directories of a second fixture from A06 or C03.

   ```sh
   ls -d <run>/fixture-repair <run>/fixture-unsaved 2>/dev/null
   ```

   Result: If the output is empty, this task is complete. A run that stopped in A06 or C03 can leave one.

9. For each listed directory, find its Device panel with the `device_panel` operation `list`.

   Result: You have the panel ID of the second fixture, if it has one.

10. Send the `device_panel` operation `close` for that panel ID.

    Result: The Device panel of the second fixture closes.

11. In the terminal of the second launcher, press Ctrl-C.

    Result: The second launcher stops its Xvfb, VNC and candidate.

    > **CAUTION:** DELETE ONLY THE LISTED DIRECTORIES OF THE SECOND FIXTURES. Other
    > directories can hold data of this run or of other people.

12. Delete each listed directory.

    ```sh
    rm -r <run>/fixture-repair <run>/fixture-unsaved 2>/dev/null; ls -d <run>/fixture-* 2>/dev/null
    ```

    Result: The output shows only `<run>/fixture`.

### 6.5 X05 — Make sure that RunPod shows no test resources

> **CAUTION:** SEND THE RUNPOD KEY ONLY TO THE RUNPOD API. The header file
> contains the key. Do not show the file or the request headers.

1. List the pods of the RunPod account.

   ```sh
   bash <run>/runpod-list.sh pods
   ```

   Result: The script reads all pages. The list contains no pod ID from the resource ledger.

> **CAUTION:** SEND THE RUNPOD KEY ONLY TO THE RUNPOD API. The header file
> contains the key. Do not show the file or the request headers.

2. List the network volumes of the RunPod account.

   ```sh
   bash <run>/runpod-list.sh network-volumes
   ```

   Result: The list contains no network volume ID from the resource ledger.

> **CAUTION:** SEND THE RUNPOD KEY ONLY TO THE RUNPOD API. The header file
> contains the key. Do not show the file or the request headers.

3. List the templates of the RunPod account.

   ```sh
   bash <run>/runpod-list.sh templates
   ```

   Result: Each template was also in the baseline. Horizon makes no RunPod template.

> **CAUTION:** SEND THE RUNPOD KEY ONLY TO THE RUNPOD API. The header file
> contains the key. Do not show the file or the request headers.

4. List the registry credentials of the RunPod account.

   ```sh
   bash <run>/runpod-list.sh registries
   ```

   Result: The list contains no registry credential that the ledger records as active.

> **CAUTION:** SEND THE RUNPOD KEY ONLY TO THE RUNPOD API. The header file
> contains the key. Do not show the file or the request headers.

5. Save all RunPod lists in the format of the baseline.

   ```sh
   ( set -e; for k in pods network-volumes registries templates; do bash <run>/runpod-list.sh "$k"; done ) > <evidence>/runpod-after.jsonl
   ```

   Result: The evidence shows the final RunPod state.
   If an API call fails, the command stops with an error. Then do this step again.

6. Compare the final RunPod state with the baseline.

   ```sh
   diff <(sort <evidence>/runpod-before.jsonl) <(sort <evidence>/runpod-after.jsonl)
   ```

   Result: Each new line is a resource that X01 step 5 records as not owned by this run.

## 7. Pass criteria

- The board shows no test cloud.
- The Hetzner API shows no server, volume or SSH key from the resource ledger.
- The RunPod API shows no pod or network volume from the resource ledger.
- The provider lists agree with the baselines.
- The **Tailnets** tab does not list the test tailnet.
- The evidence lists each leftover tailnet node, and the admin console no
  longer lists a node ID from the resource ledger.
- The fixture children exited, and the target expired.

## 8. Cleanup

1. Delete the two header files and the two list scripts.

   ```sh
   rm -f <run>/hetzner.header <run>/runpod.header <run>/hetzner-list.sh <run>/runpod-list.sh
   ```

   Result: The API header copies are gone. The private data of the fixture below
   `<state>` still contains the saved provider keys. The main cleanup removes them.

2. If a provider list still shows a resource from the ledger, record a defect and tell the operator.

   Result: The report names each resource that continues to cost money.

## 9. Record of results

Write the results in the report of the run. Use the
[report template](../../reports/TEMPLATE.md). Write only counts of resources
in the report. Keep provider IDs and tailnet node names in the private evidence.
