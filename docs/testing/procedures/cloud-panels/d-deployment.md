---
procedure: cloud-panels-d-deployment
feature: Cloud panels smoke test, area D (deployment on Hetzner and RunPod)
platforms: [linux]
cost: rents compute
destructive: no
secrets: [RunPod API key in Cloud settings, Hetzner Cloud API token in Cloud settings, registry pull credential, test tailnet auth key in the Secret Service]
owner: peters
---

# Cloud panels test procedure, area D: deployment

## 1. Purpose

This area deploys the first clouds on Hetzner and RunPod. It makes sure that the
image, the allocation, the storage, the source upload and the readiness checks
work. It also makes sure that the progress card shows the real stages.

## 2. Applicability

- Candidate: the frozen candidate from area S.
- Platforms: Linux. Providers: Hetzner for CPU, RunPod for CPU and GPU.
- This area does not test: the panels in a cloud, the lifecycle actions or the
  tailnet traffic. Areas E, L and T test them.

## 3. Safety

> **CAUTION:** START ONLY THE CLOUDS IN THE PLANNED CLOUDS TABLE. Each cloud rents
> compute and storage. The provider charges money until somebody deletes it.

> **CAUTION:** WRITE EACH NEW PROVIDER RESOURCE IN THE RESOURCE LEDGER. Area X
> deletes only the resources that the ledger records.

> **CAUTION:** SELECT ONLY THE TEST TAILNET. A cloud on a tailnet can reach the
> devices that the tailnet policy permits.

## 4. Equipment and preconditions

- The equipment in the [main procedure](../cloud-panels.md#4-equipment-and-preconditions).
- Areas S, A and B and the tasks C01 to C30 are complete. C31 comes after D01 and D02.
  Cloud settings contain the RunPod and Hetzner keys.
- Task T01 of [area T](t-tailnets.md) is complete. **Settings › Tailnets**
  contains the test tailnet.
- `<repo>` has a committed `.horizon/cloud.yml` with these profiles:
  - `hetzner-cpu`: `provider: hetzner`, image-only, with the worker image of the run.
  - `runpod-cpu`: `provider: runpod`, image-only, with a network volume.
  - `runpod-gpu`: `gpu: true` and `min_cuda_version: "12.8"`.
- The facts in [Cloud workspaces](../../../cloud-workspaces.md) and
  [Hetzner Cloud workers](../../../cloud-hetzner.md).

## 5. Setup

Area B makes the synthetic repositories without Git LFS content and without a
submodule. Steps 1 to 4 add them to `<repo>`. Do these steps before D01, in the
fixture terminal.

1. In `<repo>`, turn on Git LFS and track `*.bin` files.

   ```sh
   git -C <repo> lfs install --local && git -C <repo> lfs track '*.bin'
   ```

   Result: `.gitattributes` in `<repo>` contains a line for `*.bin`.

2. Write a file of 1 MiB of random data to `<repo>/assets/sample.bin`.

   ```sh
   mkdir -p <repo>/assets && head -c 1048576 /dev/urandom > <repo>/assets/sample.bin
   ```

   Result: The file exists. It contains only random test data.

3. Add `<lib>` to `<repo>` as a submodule at `vendor/lib`.

   ```sh
   git -C <repo> -c protocol.file.allow=always submodule add <lib> vendor/lib
   ```

   Result: `git -C <repo> submodule status` shows `vendor/lib` and its commit.

4. Commit the LFS file and the submodule in `<repo>`.

   ```sh
   git -C <repo> add .gitattributes assets/sample.bin .gitmodules vendor/lib && git -C <repo> commit -m 'Add LFS and submodule test content'
   ```

   Result: The commit contains the LFS pointer and the submodule link.

5. In `<repo>`, make sure that the committed tree has a Git LFS file and a submodule.

   ```sh
   git -C <repo> lfs ls-files
   git -C <repo> submodule status
   ```

   Result: The first command lists one LFS file or more. The second command
   shows one pinned submodule.

6. Write the uncommitted file `uncommitted-sentinel.txt` in `<repo>`.

   Result: `git -C <repo> status --short` shows the file as untracked.

7. Write a file `ignored-sentinel.log` in `<repo>`. Do not commit it.

   ```sh
   echo ignored > <repo>/ignored-sentinel.log && echo 'ignored-sentinel.log' >> <repo>/.git/info/exclude
   ```

   Result: `git -C <repo> status --short --ignored` shows the file as ignored.

8. Record the full commit ID of `HEAD` of `<repo>` in the evidence.

   ```sh
   git -C <repo> rev-parse HEAD
   ```

   Result: You have the expected commit for D04.

## 6. Tasks

### 6.1 D01 — Deploy a Hetzner CPU cloud end to end

1. Open **Cloud › New cloud…**.

   Result: The New cloud dialog opens.

2. Wait 3 seconds.

   Result: The dialog stays open.

3. Take a new screenshot.

   Result: The dialog does not move.

4. Type `smoke-a` in the title field.

   Result: The title field shows `smoke-a`.

5. Type `<repo>` in the repository field.

   Result: The field shows `<repo>`.

6. Click **Read .horizon/cloud.yml**.

   Result: **Profile** lists the profiles that area B committed.

7. In **Profile**, select `hetzner-cpu`.

   Result: The **Machine** list shows Hetzner workers.

8. Click the cheapest Hetzner row that is in stock.

   Result: The summary shows the server type and the location.

9. Record the selected location in the private evidence.

   Result: C31 compares this location with the location of the server.

   > **CAUTION:** SELECT ONLY THE TEST TAILNET. The worker joins the tailnet and
   > can reach the devices that its policy permits.

10. In **Tailnet**, select the test tailnet.

    Result: The test tailnet is selected. **None** is not selected.

    > **CAUTION:** THIS STEP RENTS COMPUTE. Hetzner charges money for the server
    > and the volume until somebody deletes them.

11. Click **Start cloud**.

    Result: The dialog closes. The card of `smoke-a` shows the active stage and its
    elapsed time.

12. Wait until the card shows **Ready**.

    Result: The card shows the measured time to worker readiness.

13. Open the **Machine** tab of the card.

    Result: The tab shows the server type, the location and the server ID.

14. Write the server, the volume and the SSH key of `smoke-a` in the resource ledger.

    Result: The ledger has three Hetzner lines for `smoke-a`.

15. Write the tailnet node name of `smoke-a` in the resource ledger.

    Result: The ledger has a tailnet line for `smoke-a` with the node ID from the
    admin console of the test tailnet.

    > **CAUTION:** SEND THE HETZNER TOKEN ONLY TO THE HETZNER API. The header file
    > contains the token. Do not show the file or the request headers.

16. List the networks of the Hetzner project.

    ```sh
    bash <run>/hetzner-list.sh networks
    ```

    Result: If the list has a new network with a `horizon-network` label, write it in the ledger as kept.

17. Record the time to **Ready** in the evidence.

    Result: The evidence shows the deployment time.

### 6.2 D02 — Deploy a RunPod CPU cloud on a network volume

1. Open **Cloud › New cloud…**.

   Result: The New cloud dialog opens.

2. Wait 3 seconds.

   Result: The New cloud dialog opens and does not move.

3. Type `smoke-r` in the title field.

   Result: The title field shows `smoke-r`.

4. Type `<repo>` in the repository field.

   Result: The field shows `<repo>`.

5. Click **Read .horizon/cloud.yml**.

   Result: **Profile** lists the profiles that area B committed.

6. In **Profile**, select `runpod-cpu`.

   Result: The **Machine** list shows RunPod CPU workers.

7. Click the cheapest RunPod row that is in stock.

   Result: The summary shows the worker, its stock and a network volume.

8. Examine the storage lines of the summary.

   Result: The summary shows **Standard** or **High-performance** and the volume size.

9. In **Data center**, select one exact data center that has the worker in stock.

   Result: The summary names the data center. Record it in the private evidence for C31.

   > **CAUTION:** SELECT ONLY THE TEST TAILNET. The worker joins the tailnet and
   > can reach the devices that its policy permits.

10. In **Tailnet**, select the test tailnet.

    Result: The test tailnet is selected.

    > **CAUTION:** THIS STEP RENTS COMPUTE. RunPod charges money for the pod, and
    > for the network volume also when the pod is stopped.

11. Click **Start cloud**.

    Result: The card of `smoke-r` shows the active stage and its elapsed time.

12. Wait until the card shows **Ready**.

    Result: The card shows the measured time to worker readiness.

13. Open the **Machine** tab of the card.

    Result: The tab shows the pod ID, the data center and the region.

14. Write the pod and the network volume of `smoke-r` in the resource ledger.

    Result: The ledger has two RunPod lines for `smoke-r`.

15. Write the tailnet node name of `smoke-r` in the resource ledger.

    Result: The ledger has a tailnet line for `smoke-r` with the node ID from the
    admin console of the test tailnet.

16. In a worker shell of `smoke-r`, show the mount of `/workspace`.

    ```sh
    findmnt /workspace
    ```

    Result: `/workspace` is a mount of the network volume.

### 6.3 D03 — Deploy a RunPod GPU cloud with a CUDA minimum

1. Open **Cloud › New cloud…**.

   Result: The New cloud dialog opens.

2. Wait 3 seconds.

   Result: The New cloud dialog opens and does not move.

3. Type `smoke-g` in the title field.

   Result: The title field shows `smoke-g`.

4. Type `<repo>` in the repository field.

   Result: The field shows `<repo>`.

5. Click **Read .horizon/cloud.yml**.

   Result: **Profile** lists the profiles that area B committed.

6. In **Profile**, select `runpod-gpu`.

   Result: The **Machine** list shows GPU types and their stock.

7. Click the cheapest GPU type that is in stock.

   Result: The summary shows the GPU type and a pod volume.

8. In **Tailnet**, select **None**.

   Result: **None** is selected.

   > **CAUTION:** THIS STEP RENTS A GPU. A GPU costs more money for each hour
   > than a CPU worker.

9. Click **Start cloud**.

   Result: The card of `smoke-g` shows the active stage and its elapsed time.

10. Wait until the card shows **Ready**.

    Result: The card names the GPU type.

11. Write the pod of `smoke-g` in the resource ledger.

    Result: The ledger has a RunPod line for `smoke-g`.

12. In a worker shell of `smoke-g`, show the GPU and the CUDA version of the driver.

    ```sh
    nvidia-smi
    ```

    Result: The output shows the GPU type of the card. The CUDA version is 12.8 or
    higher.

### 6.4 D04 — Make sure that only committed source arrives

Use `smoke-r` from D02. The setup of this area prepared `<repo>` before D02.

1. In a worker shell of `smoke-r`, show the commit of the checkout.

   ```sh
   git rev-parse HEAD
   ```

   Result: The commit is the same as the commit that you recorded in the setup.

2. Look for the two sentinel files in the checkout.

   ```sh
   ls uncommitted-sentinel.txt ignored-sentinel.log
   ```

   Result: The command shows that neither file exists.

3. Show the status of the checkout.

   ```sh
   git status --short
   ```

   Result: The output is empty.

4. Show the LFS files of the checkout.

   ```sh
   git lfs ls-files
   ```

   Result: Each LFS file that the `source.lfs` selection includes shows `*`. Each
   excluded file shows `-` and stays a pointer file.

5. Show the submodules of the checkout.

   ```sh
   git submodule status
   ```

   Result: Each submodule is at the commit that `<repo>` pins.

6. If `<repo>` sets `submodule_history: pinned`, show the history of the submodule.

   ```sh
   git -C <submodule-path> log --oneline | wc -l
   ```

   Result: The output is `1`. Without that setting, the output is the full history.

### 6.5 D05 — Make sure that the readiness card shows the real stages

Use `smoke-a` from D01.

1. Open the **Status** tab of the card of `smoke-a`.

   Result: The card shows the time to **Ready**, a ribbon of phases and
   **Where the time went**.

2. Examine the phases in **Where the time went**.

   Result: The phases show the largest first. An image-only profile shows no
   build and no push phase.

3. Read the output under the summary on the **Status** tab.

   Result: The output shows the stages with timestamps.

4. Compare the duration of each phase with the timestamps in the output.

   Result: Each duration is in the same range as the timestamps show.

5. Add the durations of all phases.

   Result: The total is about the same as the time to **Ready**.

6. Record a screenshot of the card and the stage list in the evidence.

   Result: The evidence shows the readiness card.

## 7. Pass criteria

- `smoke-a`, `smoke-r` and `smoke-g` each show **Ready**.
- The resource ledger records each server, pod, volume, SSH key and tailnet node.
- `nvidia-smi` on `smoke-g` shows a CUDA version of 12.8 or higher.
- The checkout of `smoke-r` is at the recorded commit and has no sentinel file.
- LFS files and submodules agree with the committed selection.
- **Where the time went** agrees with the stages in the output.

## 8. Cleanup

Do not delete the clouds. The areas E, T, G, N and L use them. Area X deletes
them.

1. If you do not need `smoke-g` for more tests, stop it with **Stop worker…**.

   Result: The card shows **Stopped**. The pod volume continues to cost money.

2. Delete the two sentinel files from `<repo>`.

   Result: `git -C <repo> status --short --ignored` does not show them.

## 9. Record of results

Write the results in the report of the run. Use the
[report template](../../reports/TEMPLATE.md). Keep provider IDs and tailnet
names in the private evidence only.
