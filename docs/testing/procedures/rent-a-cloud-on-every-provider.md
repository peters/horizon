---
procedure: rent-a-cloud-on-every-provider
feature: Rent, use and delete a cloud on every provider in New cloud
platforms: [linux]
cost: rents compute   # tasks RENT-RP, RENT-HZ and RENT-AZ each rent one cloud
destructive: yes      # the delete steps remove the cloud, its volume and its SSH key
secrets: [RunPod API-key file, Hetzner API-token file, Azure ARM bearer-token file]
owner: peters
---

# Rent a cloud on every provider test procedure

## 1. Purpose

This procedure proves the completion rule of
[ADR-1774](../../architecture/adding-a-cloud-provider.md) on every provider. A
person picks the provider in **New cloud**, rents one cloud there, runs a
command on the worker, and deletes the cloud. The delete is proven from the
provider API, not from the UI text alone.

## 2. Applicability

- Candidate: a build that offers RunPod, Hetzner and Azure in **New cloud**.
- Platforms: Linux with an isolated X display. Providers: RunPod, Hetzner and
  Azure.
- This procedure does not test:
  - The Docker host provider. It rents nothing, so it has no delete proof.
  - Stop, resume and rebuild. The procedure for the provider tests them.
  - A start from the CLI or the MCP tools. This procedure uses the UI only.

## 3. Safety

> **CAUTION:** DO NOT START A SECOND CLOUD WHILE ONE IS RUNNING. Run one
> provider at a time. A cloud bills its worker, its disks and its volume from
> the start until the delete completes.

> **CAUTION:** DO NOT STOP A RUN BEFORE YOU DELETE ITS CLOUD. If you must stop,
> delete the cloud first. A cloud that you leave keeps billing.

> **CAUTION:** IN AZURE, DELETE ONLY THE RESOURCE GROUP NAMED `horizon-ws-*`
> THAT YOUR RUN MADE. Read the group name from the cloud card before you delete
> it. Never delete a group that another person or another run made.

> **CAUTION:** DO NOT PUT A REAL API KEY OR TOKEN IN A SCREENSHOT, A RECORDING OR
> A LOG. Use a private file with mode `0600`, and keep the value out of the
> evidence.

> **CAUTION:** THE DELETE STEP REMOVES THE WORKSPACE VOLUME OF THE CLOUD. Files
> on that volume are gone. Do not run this procedure on a cloud that holds work
> that you must keep.

## 4. Equipment and preconditions

- A frozen candidate binary and its SHA-256. Build once, copy the binary to a
  task-owned directory, and record the hash in `SHA256SUMS`.
- An isolated desktop: its own X display, its own window manager, a loopback VNC
  server, and a private `HOME`. Keep the private `HOME` per provider, so one run
  cannot change the settings of the next.
- These credential files, each with mode `0600`, outside the repository:
  - A RunPod API-key file.
  - A Hetzner API-token file.
  - An Azure ARM bearer-token file, from `az account get-access-token`.
- API access to the three providers, to prove that the resources are absent.
- A test Git repository with this `.horizon/cloud.yml` file, or one file per
  provider with the profile of that provider:

  ```yaml
  version: 1
  default: runpod-cpu
  profiles:
    runpod-cpu:
      provider: runpod
      image: ghcr.io/peters/horizon-worker-base@sha256:<digest>
      cpu: 4
      memory_gb: 16
      gpu: false
      storage: {container_gb: 20, volume_gb: 20}
      bootstrap: {readiness_seconds: 900}
      capabilities: {agents: [claude, codex], browsers: [chromium], desktop: true}
    hetzner-cpu:
      provider: hetzner
      image: ghcr.io/peters/horizon-worker-base@sha256:<digest>
      cpu: 2
      memory_gb: 8
      gpu: false
      storage: {container_gb: 20, volume_gb: 20}
      bootstrap: {readiness_seconds: 900}
      capabilities: {agents: [claude, codex], browsers: [chromium], desktop: true}
    azure-build:
      provider: azure
      image: ghcr.io/peters/horizon-worker-base@sha256:<digest>
      cpu: 4
      memory_gb: 16
      gpu: false
      storage: {container_gb: 20, volume_gb: 20}
      bootstrap: {readiness_seconds: 900}
      capabilities: {agents: [claude, codex], browsers: [chromium], desktop: true}
  ```

- A cloud settings file of the provider under test, in the private `HOME`:
  `$HOME/.horizon/cloud/settings.json`. It names the credential file and the
  SSH identity file of that provider.
- The Hetzner project must have free shared-core vCPU. A request above the limit
  fails with `Provider rejected the request or capacity is unavailable: shared
  core limit exceeded`.

In this procedure, `$CLOUD` is `$HOME/.horizon/cloud`.

## 5. Setup

1. Build the candidate and freeze the binary.

   ```sh
   cp target/debug/horizon "$smoke_bin/horizon"
   sha256sum "$smoke_bin/horizon" > "$smoke_bin/SHA256SUMS"
   ```

   Result: The file `SHA256SUMS` holds one SHA-256 value.

2. Start the isolated desktop and start the candidate with the private `HOME` of
   the provider under test.

   Result: `/proc/<pid>/exe` points at the frozen copy, and
   `sha256sum /proc/<pid>/exe` gives the value in `SHA256SUMS`.

3. Write `$CLOUD/settings.json` for the provider under test with mode `0600`.

   Result: **Menu › Cloud settings** shows the provider as ready, and Start of
   **New cloud** is not locked for a missing credential.

## 6. Tasks

Run one task, then delete its cloud, then start the next task. Do not run two
tasks at the same time.

### 6.1 RENT-RP — Rent and delete one RunPod cloud

> **CAUTION:** THIS TASK RENTS COMPUTE AT RUNPOD. The pod bills from its start
> until the delete completes.

1. Open **New cloud**, and select **RunPod** in the provider heading. Type the
   path of the test repository in **Where is your code?**.

   Result: The heading selects RunPod, and the profile list shows the profile of
   that repository that sets `provider: runpod`.

2. Select the profile, and read the checks under **BEFORE YOU START**.

   Result: The checks name the repository and the RunPod key, the list holds at
   least one machine, and **Start cloud** is enabled.

3. Read the machine, its data center and its price. Note which data center the
   pod uses. Press **Start cloud**.

   Result: The card shows the steps, and **Provision worker** names the pod ID.

4. Wait for the ready state.

   Result: The card reads **Ready**, and the workspace shows a panel.

5. Open a Shell panel on the cloud with **Quick Nav** (`Ctrl+Shift+K`), **New
   Panel**. Run this command:

   ```sh
   echo h1774-ui-smoke > /workspace/marker.txt && cat /workspace/marker.txt && docker images | head -5
   ```

   Result: The shell prints `h1774-ui-smoke`. A RunPod worker mounts its
   workspace as a network volume, so the write to `/workspace` succeeds. The
   base worker image has no `docker` command, so `docker images` prints
   `docker: command not found`. That message is the expected result, and it
   shows that the command reached the worker.

6. Run these commands, and compare them with the cloud card:

   ```sh
   hostname; grep PRETTY_NAME /etc/os-release; findmnt -no TARGET,SOURCE,FSTYPE /workspace; ls /etc/horizon-worker
   ```

   Result: `hostname` gives the container identity, `/etc/os-release` gives the
   worker image release, `findmnt` shows the workspace volume of the pod, and
   `/etc/horizon-worker` holds `agent-versions.json` and `capabilities.json`.

7. Open the card menu with the chevron next to **Stop…**, and select **Manage**.
   Select **Delete cloud resources…**, then **Delete resources permanently**.

   Result: The card reads **Worker deleted** and **Workspace storage cleaned
   up**, and the heading reads **Nothing billing now**. Select **Remove cloud**.

> **CAUTION:** THE NEXT STEP READS THE PROVIDER ACCOUNT. DO NOT DELETE A POD,
> A VOLUME OR A TEMPLATE THAT YOUR RUN DID NOT MAKE. Compare the ID and the
> cloud ID in the name before you delete anything.

8. Prove the absence from the RunPod API. Use the key file of the run:

   ```sh
   curl -s -H "Authorization: Bearer $(cat "$KEY")" https://rest.runpod.io/v1/pods
   curl -s -H "Authorization: Bearer $(cat "$KEY")" https://rest.runpod.io/v1/networkvolumes
   curl -s -o /dev/null -w "%{http_code}\n" -H "Authorization: Bearer $(cat "$KEY")" https://rest.runpod.io/v1/pods/<pod-id>
   ```

   Result: The pod list holds no pod with the cloud ID of the run, the volume
   list holds no `horizon-volume-<cloud-id>`, and the single-pod request gives
   `404`.

### 6.2 RENT-HZ — Rent and delete one Hetzner cloud

> **CAUTION:** THIS TASK RENTS COMPUTE AT HETZNER. The server and its volume bill
> from their start until the delete completes.

1. Open **New cloud**, and select **Hetzner** in the provider heading. Type the
   path of the test repository.

   Result: The profile list shows the profile of that repository that sets
   `provider: hetzner`.

2. Select the profile. Read the checks and the machine cards.

   Result: The checks name the token and the SSH identity, the machine cards
   show server types of the `hetzner` section, and **Start cloud** is enabled.

3. Select a server type that the project may create. Press **Start cloud**.

   Result: The card shows **Provision worker**, and then **Check readiness**. A
   request above the shared-core limit fails with
   `Provider rejected the request or capacity is unavailable: shared core limit
   exceeded` within a few seconds. Record that text, and select the next smaller
   type that the limit allows.

4. Wait for the ready state.

   Result: The card reads **Ready in** a time, and the workspace shows a panel.

5. Open a Shell panel on the cloud, and run this command:

   ```sh
   echo h1774-ui-smoke > /workspace/marker.txt && cat /workspace/marker.txt && docker images | head -5
   ```

   Result: A Hetzner worker mounts the workspace volume under `/workspace`, and
   the volume root belongs to the host. The shell answers
   `bash: /workspace/marker.txt: Permission denied`. Record that text. Write the
   marker in the checkout instead, and run the command again:

   ```sh
   echo h1774-ui-smoke > /workspace/checkout/marker.txt && cat /workspace/checkout/marker.txt && docker images | head -5
   ```

   Result: The shell prints `h1774-ui-smoke`, and then
   `docker: command not found`.

6. Run the identity commands of step 6 of task RENT-RP.

   Result: `findmnt` shows `/dev/sdb[/workspace] ext4`, which is the volume of
   the server, and `/etc/horizon-worker` holds the two contract files.

7. Delete the cloud with the card menu: chevron, **Manage**, **Delete cloud
   resources…**, **Delete resources permanently**.

   Result: The card reads **Worker deleted**, **Workspace storage cleaned up**,
   and **Nothing billing now**. Select **Remove cloud**.

8. Prove the absence from the Hetzner API:

   ```sh
   curl -s -H "Authorization: Bearer $(cat "$TOKEN")" https://api.hetzner.cloud/v1/servers
   curl -s -H "Authorization: Bearer $(cat "$TOKEN")" https://api.hetzner.cloud/v1/volumes
   curl -s -H "Authorization: Bearer $(cat "$TOKEN")" https://api.hetzner.cloud/v1/ssh_keys
   ```

   Result: No server and no volume names the cloud ID of the run, and the SSH
   key of the run is gone. Keep every key that your run did not make.

### 6.3 RENT-AZ — Rent and delete one Azure cloud

> **CAUTION:** THIS TASK RENTS COMPUTE IN THE AZURE SUBSCRIPTION. The VM, its
> OS disk, its workspace disk and its static address bill from the start until
> the delete completes.

1. Open **New cloud**, and select **Azure** in the provider heading. Type the
   path of the test repository.

   Result: The profile list shows the profile of that repository that sets
   `provider: azure`, and the dialog lists the VM sizes and regions of the
   `azure` section.

2. Select the profile and the VM size.

   Result: **Checks passed** shows, and **Start cloud** is enabled.

3. Press **Start cloud**, and wait for the ready state.

   Result: The card reads **Ready in** a time, on the VM size and the region of
   the section.

4. Open a Shell panel on the cloud, and run the marker command of task RENT-HZ
   step 5. An Azure worker refuses the write to the root of `/workspace` in the
   same way, so use the checkout path and record which command answered.

   Result: The shell prints `h1774-ui-smoke`, and then
   `docker: command not found`.

5. Delete the cloud with the card menu: chevron, **Manage**, **Delete cloud
   resources…**, **Delete resources permanently**.

   Result: The card reads **Worker deleted** and **Workspace storage cleaned
   up**.

6. Prove the absence with the Azure CLI. Read the group name from the cloud
   card, and use it in the first command:

   ```sh
   az group exists -n <resource-group>
   az group list --query "[?starts_with(name,'horizon-ws-')].name" -o tsv
   az resource list --query "[?tags.\"horizon-operation\"=='<cloud-id>']" -o tsv
   ```

   Result: `az group exists` gives `false`, the group list holds no
   `horizon-ws-<cloud-id>` of this run, and the tag sweep is empty. The group
   list can hold a `horizon-ws-*` group of another run; that group is not this
   run's proof, and you must not delete it.

## 7. Pass criteria

- Every task of section 6 shows **Checks passed** and an enabled **Start cloud**
  before the start.
- Every cloud reaches **Ready**, and the marker command prints
  `h1774-ui-smoke` on its worker.
- Every delete completes, and the provider API shows no resource of the run.
- The account shows no resource of the run after the last task.

## 8. Cleanup

1. Stop the candidate and the isolated desktop of each provider.

   Result: The process of the frozen binary is gone, and the X display is
   closed.

2. Remove the private `HOME`, the credential copies and the local token files
   of the run.

   Result: The files are gone from the task-owned directory.

3. Read the API lists of section 6 one more time.

   Result: No resource of any run remains.

## 9. Record of results

Keep the screenshots and the recording of the run outside the repository. Put
these items in the pull request or in a report under
`docs/testing/reports/`:

- The candidate head and the SHA-256 of the frozen binary.
- Per provider: the cloud ID, the machine type, the region, the start to ready
  time, the marker output, the delete time, and the API proof of absence.
- Each step that failed, the screenshot path, and the exact text of the failure.

### 9.1 Results of the run on 2026-10-25

Candidate `88f100dd6`, frozen debug binary
`sha256 4f7b8527d4fa35ba853e60fe9c617894cefcbfd5220997816bfd769283b9965f`.
Each provider ran on its own X display, with its own private `HOME` and a
loopback VNC server.

| Provider | Machine | Region | Ready | Delete | Proof of absence |
|---|---|---|---|---|---|
| RunPod | 4 vCPU · 16 GB, pod `ukhxm37jhe2sy2` | AP-JP-1 | 1m 54s | 0m 06s | Pod `404`, no `horizon-cloud-*` pod, no `horizon-volume-<cloud-id>` |
| Hetzner | cx33, server `172339636` | fsn1 | 2m 24s | 0m 13s | No server, no volume, no SSH key of the run |
| Azure | `Standard_D4s_v3`, group `horizon-ws-261a07aa-c410-4693-b647-858a5d699aee` | northeurope | 2m 15s | 3m 16s | `az group exists` `false`, empty group list, empty tag sweep |

Three findings from the run:

1. **Hetzner refuses 8 shared-core vCPU on this account.** The first request
   used `cx43` in `fsn1` and failed after 2.8 s with `Provider rejected the
   request or capacity is unavailable: shared core limit exceeded`. The same
   profile at 2 vCPU and 8 GB selected `cx33` and reached ready. The volume that
   the refused run made was deleted with **Delete cloud resources…**, and the
   Hetzner API then listed no volume and no SSH key of that run.
2. **The worker refuses a write to the root of `/workspace` on Hetzner and on
   Azure.** Both mount the workspace volume as a host filesystem, and the volume
   root belongs to the host. The marker is written in `/workspace/checkout`
   instead. RunPod mounts a network volume and accepts the write.
3. **The base worker image has no `docker` command.** `docker images` prints
   `docker: command not found` on every provider. The container identity, the
   release in `/etc/os-release`, the mount in `findmnt` and the two contract
   files in `/etc/horizon-worker` are the proof that the image ran.

### 9.2 Results of the run on 2026-10-30

Candidate `884e818b4`, frozen debug binary
`sha256 c0a0f78747dbec0dd4b38d016621d279192469018eb80b78dce8def35bd7f6ab`.
The run made three clouds in one fixture home, one provider at a time, and
deleted each cloud before the next provider started. The report is
[the Azure completion rule report](../reports/2026-10-30-azure-completion-rule.md).

| Provider | Machine | Region | Ready | Delete | Proof of absence |
|---|---|---|---|---|---|
| Azure | `Standard_D4s_v3`, group `horizon-ws-5485aecf-eee5-4bdd-8cf9-f9a94bb00de1` | northeurope | 3m 31s | 3m 03s | `az group exists` `false`, empty group list, empty tag sweep, no resource of the run in 389 |
| RunPod | 4 vCPU · 16 GB, pod `4de78455zae0d3` | any, network volume on AP-JP-1 | 1m 54s | 0m 07s | Pod `404`, no `horizon-cloud-*` pod, no volume of the cloud |
| Hetzner | cx33, server `173542857` | fsn1 | 1m 53s | 0m 10s | No server, no volume of the run |

Findings 1 and 3 of Section 9.1 held again. One request at 8 shared-core vCPU
failed in 3.8 s with the same message; it was the misrouted request of finding 4,
not the Hetzner lane. The Hetzner lane started on `cx33` and passed.

One new finding:

4. **A later profile choice replaces the provider of the heading.** The run
   selected **RunPod** in the heading and then the `runpod-cpu` profile chip. The
   placement moved to the cheapest offer across all providers, which was a
   Hetzner `cx43`, and the card read `Hetzner · runpod-cpu`. The provision
   request then went to Hetzner and failed there. A person who chooses a provider
   and then a profile gets a cloud on the other provider.
   [Issue #1922](https://github.com/peters/horizon/issues/1922) tracks it.

Two conditions of the host, and not defects, are in the report: a `TMPDIR` of
`/var/tmp` makes the validate step fail with a read-only file system, and the
fixture needs its own Docker daemon because it has no Docker socket.
