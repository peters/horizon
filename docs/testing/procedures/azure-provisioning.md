---
procedure: azure-provisioning
feature: The Azure worker bootstrap, its ARM deployment, the creation of an Azure worker, and its stop, start and delete
platforms: [linux]
cost: rents compute   # tasks AZV-L1 and AZV-L2 only; tasks AZV-01 to AZV-19 cost nothing
destructive: yes      # task AZV-L2 and the cleanup delete the test resource groups
secrets: [Azure ARM bearer token file, SSH identity of the cloud settings]
owner: peters
---

# Azure provisioning test procedure

## 1. Purpose

This procedure makes sure that `horizon-cloud` plans an Azure worker correctly before it sends a request.
It examines the host bootstrap, the metadata guard, the custom data limit and the ARM deployment template.

## 2. Applicability

- Candidate: a pull request of #1774 that changes `horizon_cloud::host`, `horizon_cloud::azure::template`, `horizon_cloud::azure::provision`, `horizon_cloud::azure::lifecycle`, `horizon_cloud::azure::release`, the `azure` cloud settings, the Azure provider arms, the rebuild dispatch in `horizon_core::cloud_runtime::deployment::replacement` or `horizon_core::cloud_runtime::deployment::azure`.
- Platform: Linux with the repository Rust toolchain.
- Tasks AZV-01 to AZV-19 use synthetic specs, recorded ARM answers and a loopback ARM endpoint. They need no network and no Azure account.
- Tasks AZV-L1 and AZV-L2 are the live acceptance. AZV-L1 creates one real VM in an Azure subscription. AZV-L2 stops, resumes and deletes it. Both need the Azure CLI.
- The trust boundary is in [the Azure provisioning architecture](../../architecture/azure-provisioning.md).
- This procedure does not test these items:
  - The rebuild of an Azure cloud. It stays refused.
  - Azure in the New cloud dialog, past its checks. The [Azure New cloud procedure](azure-new-cloud.md) tests the Azure rows, their prices and the start of a cloud. AZV-17 examines the check rows.

## 3. Safety

Tasks AZV-01 to AZV-19 do not rent compute, delete data, send a secret or change access.

> **CAUTION:** TASK AZV-L1 RENTS AN AZURE VM, A DISK AND A STATIC ADDRESS. The subscription pays for them until you delete the resource group. Do the cleanup of this task in the same session.

> **CAUTION:** TASK AZV-L1 SENDS THE ARM BEARER TOKEN TO `management.azure.com`. Keep the token file private (`0600`) and delete it after the run.

> **CAUTION:** TASK AZV-L2 AND THE CLEANUP DELETE THE RESOURCE GROUP `horizon-ws-<cloud id>` AND ALL DATA ON ITS DISKS. Examine the group name and its `horizon-operation` tag before you delete it.

## 4. Equipment and preconditions

- A checkout of the candidate commit.
- The repository's supported Rust toolchain.
- For tasks AZV-01 to AZV-19: no Azure account, Azure credential or provider key.
- For tasks AZV-L1 and AZV-L2: an Azure subscription with quota for one `Standard_D4s_v3`, the Azure CLI (`az`) signed in to it, `jq`, an SSH identity and a worker image digest that the VM can pull.

## 5. Setup

1. Open a terminal in the candidate checkout.

   Result: `Cargo.toml` names the Horizon workspace.

2. Build the tests of `horizon-cloud`.

   ```sh
   cargo test -p horizon-cloud --lib --no-run
   ```

   Result: The build succeeds.

## 6. Tasks

Do the tasks in order. Each task has a stable ID for the report.

### 6.1 AZV-01 — Check that a prepared host keeps its bootstrap

1. Run the prepared host tests.

   ```sh
   cargo test -p horizon-cloud --lib -- host::tests::a_prepared_host_keeps_its_user_data host::tests::the_metadata_block_and_volume_check_run_before_every_container_start
   ```

   Result: 2 tests pass. These tests examine the items that follow:
   - The user data of a Hetzner host has the same keys as before: `ssh_pwauth`, `disable_root`, `mounts`, `runcmd` and `write_files`.
   - Its guard script is the same, byte for byte. It drops container traffic to `169.254.0.0/16` only.
   - It has no `packages`, no `fs_setup` and no `ip6tables` rule.

### 6.2 AZV-02 — Check the Azure host bootstrap

1. Run the Azure host tests.

   ```sh
   cargo test -p horizon-cloud --lib -- host::tests::an_azure_host_installs_docker_and_formats_only_a_blank_data_disk host::tests::an_azure_host_formats_no_device_but_its_data_disk
   ```

   Result: 2 tests pass. These tests examine the items that follow:
   - cloud-init updates the package lists and installs `docker.io`.
   - cloud-init formats `/dev/disk/azure/scsi1/lun0` as ext4 only when it has no file system (`overwrite: false`).
   - The host mounts that disk at `/mnt/horizon-volume`.
   - The other files, the service and the setup command are the same as on a prepared host.
   - An Azure plan refuses each other device. A prepared plan refuses the Azure device.

### 6.3 AZV-03 — Check the metadata guard

1. Run the guard test.

   ```sh
   cargo test -p horizon-cloud --lib -- host::tests::an_azure_guard_closes_both_metadata_addresses_and_ipv6_before_every_start
   ```

   Result: 1 test passes. This test examines the items that follow:
   - The guard keeps the drop of `169.254.0.0/16`. This range holds the Azure instance metadata service, `169.254.169.254`.
   - The guard drops container traffic to ports 80 and 32526 of the Azure platform
     address, `168.63.129.16`, and leaves the rest of the address reachable because it
     also serves DNS.
   - The guard drops each IPv6 packet that the host forwards from `docker0`.
   - The guard runs before the container starts. The start script has none of these addresses.

### 6.4 AZV-04 — Check the custom data limit

1. Run the size tests.

   ```sh
   cargo test -p horizon-cloud --lib -- azure::tests::template::custom_data_is_refused_above_64_kib_once_encoded azure::tests::template::a_bootstrap_too_large_for_a_host_is_refused_while_planning
   ```

   Result: 2 tests pass. These tests examine the items that follow:
   - 49,152 bytes of bootstrap encode to 65,536 bytes and fit. One more byte is refused.
   - The largest bootstrap that the host accepts, 32 KiB, fits Azure's limit.
   - A plan with a pull token of 32,768 characters is refused while it is planned, before a request.

### 6.5 AZV-05 — Check the ARM deployment template

1. Run the template tests.

   ```sh
   cargo test -p horizon-cloud --lib -- azure::tests::template
   ```

   Result: 7 tests pass. These tests examine the items that follow:
   - One deployment makes these resources in this order: the network security group, the virtual network with its subnet, the public address, the data disk, the network interface and the VM.
   - Each resource has the tags parameter. A `Microsoft.Resources/tags` resource tags the OS disk after the VM exists.
   - The only inbound rule allows TCP port 22 from the internet. The network interface uses the security group.
   - The public address is Standard, static and IPv4.
   - The data disk starts empty and is detached, not deleted, with the VM. The OS disk is deleted with the VM.
   - The VM has no managed identity and no password login. `customData` is a secure parameter.
   - The parameters have the tag `horizon-operation` with the operation id.
   - The custom data decodes to the Azure `#cloud-config` with the pull login.
   - The debug text of a plan does not show the custom data or the pull secret.
   - A plan refuses a workspace disk outside 4 to 32,767 GB, a long id, a mutable image, a shared worker and an OS disk above 4,095 GB.
   - VM sizes are used in the order of the settings. Each size must have the CPU and memory of the profile.

### 6.6 AZV-06 — Check a new worker

1. Run the creation test.

   ```sh
   cargo test -p horizon-cloud --lib -- azure::tests::provision::a_new_cloud_creates_its_group_then_one_deployment_and_binds_the_vm
   ```

   Result: 1 test passes. This test examines the items that follow:
   - The client reads the group name. Then it saves the group fence and sends one PUT of the group with the tag.
   - It saves the worker fence and sends one PUT of the deployment. It polls the operation until it succeeds.
   - It binds the worker to the `vmId` of the VM.
   - The worker has the address `192.0.2.10:22`, 4 vCPUs, 16 GB, a 30 GB OS disk and a 64 GB workspace disk.

### 6.7 AZV-07 — Check that a lost create is not sent again

1. Run the reconcile tests.

   ```sh
   cargo test -p horizon-cloud --lib -- azure::tests::provision::a_lost_create_answer_is_reconciled_and_never_sent_again azure::tests::provision::a_conflict_keeps_the_fence_and_an_unfound_create_is_unresolved azure::tests::provision::a_failed_deployment_keeps_the_fence_until_arm_shows_it_made_no_vm
   ```

   Result: 3 tests pass. These tests examine the items that follow:
   - After a lost answer (503), the next attempt sends GET requests only. It follows the deployment to its end and binds the VM. The transcript has one deployment PUT.
   - A 409 keeps the fence at `Requested`. If no VM and no deployment are found, the result is `CreationUnresolved`. The fence stays.
   - A group whose create answer was lost and that is not found gives `CreationUnresolved`. The group is not created again.
   - A failed deployment keeps the fence. When ARM reports the deployment as ended without a VM, the fence goes back to `Prepared`.

### 6.8 AZV-08 — Check a bound worker and a found group

1. Run the read tests.

   ```sh
   cargo test -p horizon-cloud --lib -- azure::tests::provision::a_bound_worker_is_only_read_and_must_be_the_same_instance azure::tests::provision::a_found_group_is_adopted_only_with_the_operation_tag_and_never_written azure::tests::provision::a_waiting_caller_reads_the_bound_worker_with_the_same_checks
   ```

   Result: 3 tests pass. These tests examine the items that follow:
   - A bound worker is read. It is never written to.
   - A deallocated VM is a stopped worker.
   - A VM with another `vmId`, or no VM, is `WorkerLost`.
   - A VM with a second data disk, or with the tag of another operation, is refused.
   - A group with the tag of another operation gives `IdentityMismatch`. The client writes no record and no request.
   - A group with the operation's tag in another region is refused. The client writes no record.
   - A group and VM from an attempt whose records were lost are bound without a PUT.
   - `inspect_worker`, which a caller uses while it waits for the VM, makes the same checks with GET requests only. A VM in another region is refused.

### 6.9 AZV-09 — Check the refusals and the capacity rules

1. Run the refusal tests.

   ```sh
   cargo test -p horizon-cloud --lib -- azure::tests::provision::a_request_that_cannot_succeed_sends_nothing azure::tests::provision::a_sold_out_size_moves_to_the_next_and_proves_nothing_was_made
   ```

   Result: 2 tests pass. These tests examine the items that follow:
   - These requests are refused before the first request, and no record is written: a workspace disk of 3 GB, an unknown VM size, no region, a region that is no longer allowed, a bootstrap above the host limit and a pull login for another registry.
   - A capacity refusal (`SkuNotAvailable` or `QuotaExceeded`) puts the fence back to `Prepared` and tries the next VM size.
   - When each size is refused, the result is `Capacity`. The empty group stays bound.

### 6.10 AZV-10 — Check the settings and the provider answers

1. Run the settings and provider tests.

   ```sh
   cargo test -p horizon-core --lib -- cloud_runtime::settings cloud_runtime::providers
   ```

   Result: All tests pass. These tests examine the items that follow:
   - The `azure` section needs an absolute private token file, a subscription UUID, and VM sizes and regions from the catalog.
   - A token file that others can read, or with two words, is refused.
   - Without an `azure` section, `cpu_flavors` and `data_centers` refuse. The `RunPod` settings do not answer for an Azure cloud.
   - With an `azure` section, `cpu_flavors` and `data_centers` give the VM sizes and regions in the settings order, and `compute` gives a client without a request.
   - Without an `azure` section, `preflight` and `compute` give "Add an azure section to the cloud settings before deploying an Azure cloud".
   - With an `azure` section, `preflight` admits the profile. It refuses a workspace disk of 3 GB, a cloud id of 80 characters or with a dot, a profile that no VM size of the settings fits, and an unreadable token file.
   - Without an `azure` section, check, stop, resume and delete refuse with the same text and write no file. Release refuses with "Azure clouds hold no hosted devices to release".
   - `retained` is true when `azure.json` has a group fence at `requested` or `bound`.

### 6.11 AZV-11 — Check the scripted create through the deployment records

1. Run the loopback tests.

   ```sh
   cargo test -p horizon-core --lib -- cloud_runtime::deployment::azure
   ```

   Result: 20 tests pass and 4 tests (`live_create`, `live_stop`, `live_resume` and `live_delete`) are ignored. The filter also selects the tests of tasks AZV-14 and AZV-19. These tests examine the items that follow:
   - A create against a loopback ARM endpoint sends these requests in this order: GET of the group, PUT of the group, PUT of the deployment, GET of the VM, GET of the deployment.
   - Each mutation is announced only after its fence is durable. The deployment record has the bound `vmId` and the worker, and `azure.json` has the bound group and the region.
   - After a lost create answer (503), the deployment record keeps `requested`. The next run sends GET requests only and binds the VM.
   - A transport failure before the first create writes no `azure.json` and retains nothing. A requested worker keeps its fence.
   - A workspace disk of 3 GB writes no `azure.json`. A spec for another cloud is refused.
   - A private image of a registry binding needs a verified grant before allocation.

### 6.12 AZV-12 — Check the stop and the start of the VM

1. Run the power tests.

   ```sh
   cargo test -p horizon-cloud --lib -- azure::tests::lifecycle::a_stop azure::tests::lifecycle::a_powered_off azure::tests::lifecycle::a_deallocation azure::tests::lifecycle::a_start azure::tests::lifecycle::a_power_change azure::tests::lifecycle::a_failed_start
   ```

   Result: 6 tests pass. These tests examine the items that follow:
   - A stop sends one `POST .../worker/deallocate`, after the reads of the VM and its address. It follows the operation and then reads the VM until it is `PowerState/deallocated`. No request has `powerOff`.
   - The stopped worker has the same `vmId` and the status `Stopped`. A second stop sends GET requests only and is not announced.
   - A VM that is powered off but still allocated (`PowerState/stopped`) has the status `STOPPED_ALLOCATED`. It is not a stopped worker, and a stop deallocates it.
   - A deallocation that is under way is announced and awaited. No request is sent again.
   - A start sends one `POST .../worker/start` and ends with the same `vmId` running at the same address.
   - An absent VM, another instance and a VM without the operation's tag are refused before an announcement or a request.
   - A start that ARM reports as failed is a refusal with ARM's reason.

### 6.13 AZV-13 — Check the delete and its absence proof

1. Run the delete tests.

   ```sh
   cargo test -p horizon-cloud --lib -- azure::tests::lifecycle::a_delete azure::tests::lifecycle::a_lost_delete azure::tests::lifecycle::a_group_that azure::tests::lifecycle::a_tagged_resource azure::tests::lifecycle::a_group_whose azure::tests::lifecycle::a_cloud_that azure::tests::lifecycle::only_a_complete azure::tests::lifecycle::a_cloud_being
   ```

   Result: 8 tests pass. These tests examine the items that follow:
   - The journal records `deleting` before the `DELETE` of the group, the group `terminated` after the 404, and `swept` after an empty tag sweep. The VM fence goes to `terminated` last. There is one `DELETE` request.
   - A delete of a proven cloud sends nothing.
   - After a lost `DELETE` answer (503), the retry reads the group, finds it `Deleting` and sends no second `DELETE`.
   - A group without the operation's tag is not deleted.
   - A tagged resource that stays after the 404 keeps the cloud retained and not proven. The error does not show the resource id. A page link is not a proof.
   - A group whose create answer was lost is trusted as absent only after 30 seconds and a second 404.
   - A failed sweep of a group that was never requested retains nothing.
   - Only a journal with `deleting`, `swept` and the group id of this operation proves absence.
   - `provision` refuses a cloud that is being deleted, before a request.

### 6.14 AZV-14 — Check the stop, resume and delete through the deployment records

1. Run the loopback lifecycle tests.

   ```sh
   cargo test -p horizon-core --lib -- cloud_runtime::deployment::azure::tests::lifecycle
   ```

   Result: 9 tests pass. These tests examine the items that follow:
   - A stop saves `stop_requested` and the stage `Stopping` before it announces the request. It sends one `POST .../worker/deallocate`. The record then has the stage `Stopped`, the same bound `vmId` and a stopped worker. A check after it gives `Inactive`, and `confirmed_stopped` is true. `retained` stays true.
   - After a stop request with a lost answer (503), the stop stays recorded and pending. The retry finds the VM deallocated, sends nothing and settles the stop.
   - A stop that ARM refuses (403) rolls the stop back to `Ready`.
   - A resume sends one `POST .../worker/start` to the same VM. The record has the stage `Readiness`, no stop and a resume time.
   - A resume of a cloud that is being deleted, and a stop or resume without a bound VM, are refused before a request.
   - A check binds a VM that it finds for a requested worker, and gives `Missing` for a bound VM that is gone. A worker hint of another VM is refused.
   - A delete sends the `GET`, `DELETE`, `GET` (404) of the group and then the tag sweep. Then `azure.json` proves absence, `retained` is false and the VM fence is `terminated`.
   - A delete that fails keeps `deleting`, the bound VM fence and `retained`.
   - Settings that name another subscription than the cloud's group are refused.

### 6.15 AZV-15 — Check the deletion proof and the reconnect admission

1. Run the deletion proof, removal and reconnect tests.

   ```sh
   cargo test -p horizon-core --lib -- cloud_runtime::tailnet::cleanup cloud_runtime::lifecycle::tests::an_azure cloud_runtime::deployment::reconnect cloud_runtime::deployment::tests::an_azure cloud_runtime::deployment::redeploy
   ```

   Result: All tests pass. These tests examine the items that follow:
   - `providers::deletion` confirms an Azure deletion only from an `azure.json` with `deleting`, `swept` and the group `terminated` with the id of this cloud's group.
   - The proof is refused for the group of another cloud, a journal without `swept`, a bound group, an unknown field, a bound VM fence, a journal above 1 MiB, and a `docker-host.json`, `hetzner.json` or `workspace-volume.json` beside it. An `azure.json` beside a Docker host journal refuses that proof too.
   - Tailnet node cleanup of an Azure receipt runs after the proof, and an unsettled `azure.json` stops it before an HTTP request.
   - `can_remove` is false while the group is bound, and after its 404 until the sweep. It is true after the proof. `can_remove_without_deployment` is false while `azure.json` exists.
   - A reconnect admits a bound Azure record. It refuses a record without a bound VM, a stopped record and a cloud whose `azure.json` has `deleting`, before a provider request.
   - A redeploy of a `Deleted` Azure record is refused before a save. The message tells the person to remove the card and create a new cloud. The record and its `azure.json` do not change. A reconnect of that record gets the same message.
   - Without an `azure` section, a deploy is refused before a deployment record is written.

### 6.16 AZV-16 — Check the companion path and the agent stop

1. Run the companion and agent stop tests.

   ```sh
   cargo test -p horizon-core --lib -- cloud_runtime::companions::lifecycle::tests::an_azure cloud_runtime::companions::lifecycle::tests::the_live_backend_needs_the_azure cloud_runtime::lifecycle::agent_stop
   ```

   Result: All tests pass. These tests examine the items that follow:
   - A companion's Ensure Ready on a bound Azure record runs. An `azure.json` with `deleting` refuses it.
   - An `azure.json` without its deployment record stops a companion before a decision.
   - Without an `azure` section, the companion's stop, resume and reconnect give "Configure this cloud provider before executing".
   - Horizon reads an agent's stop request from a ready Azure worker once, as from a Hetzner or Docker host worker.

### 6.17 AZV-17 — Check that Azure is a provider Horizon creates on

1. Run the provider, offer, worker list, selection, settings and cost tests.

   ```sh
   cargo test -p horizon-cloud --lib -- provider::tests::an_azure offers::azure offers::workers
   cargo test -p horizon-core --lib -- cloud_panel::selection cloud_runtime::settings cloud_runtime::cost::tests::a_stopped_azure
   cargo test -p horizon-ui --bin horizon -- creation::checks::tests::an_azure
   ```

   Result: All tests pass. These tests examine the items that follow:
   - `AZURE.creatable` and `offers::AZURE_DEPLOYABLE` are `true`.
   - The Azure section of `cloud_offers` has `deployable: true`. Each Azure offer has `rentable: false`.
   - A worker choice of an Azure offer gives one VM size in `cpu_types` and one region in `data_centers`. A choice without a region is refused with "Choose an exact Azure VM size and region". A VM size below the profile is refused.
   - `Workers::add_azure` lists only the Azure VM sizes and regions of the `azure` section. A size that meets the profile is listed with the matching workers, and a smaller size after them. A GPU profile gets no Azure worker.
   - `Settings::for_cloud` narrows the `azure` section to the chosen VM size and region. A placement of another provider leaves the section whole. A chosen size or region that the section does not have is refused before work starts.
   - The rate text of an Azure cloud says that Azure reports no hourly rate, and that a stop deallocates the VM and ends the compute charge while the disks and the static address bill until the cloud is deleted.
   - In New cloud, an Azure profile does not stay at a pending "Checking the Azure account" row after the settings are read. Without the `azure` section, the row fails with "No Azure settings on this computer yet" and names `token_file`, `subscription_id`, `vm_sizes` and `regions`. With a token that cannot be read, the row fails with the reason. Start stays off in these two cases, and the dialog shows no RunPod or Hetzner key form for the Azure profile. With a readable token and current prices, the row passes with "Azure token is readable and prices are current; Azure checks the token when the cloud starts".

### 6.18 AZV-18 — Check the release of a VM that keeps its workspace disk

1. Run the recorded release tests.

   ```sh
   cargo test -p horizon-cloud --lib -- azure::tests::release
   ```

   Result: 12 tests pass. These tests examine the items that follow:
   - A release sends one `DELETE .../virtualMachines/worker`. It sends no other write: no disk, address, network or group is deleted. Then the VM and `worker-os` answer 404, and `worker-workspace` has the tag and no `managedBy`.
   - The release tells its caller just before the `DELETE`, and at no other time. If the caller cannot record the release, the release sends no `DELETE`.
   - A VM in the state `Deleting` is only read. The release sends no second `DELETE`.
   - A VM that is gone is proven gone again without a write.
   - The release waits until `worker-os` is gone and `worker-workspace` is free.
   - A VM whose data disk has `deleteOption: Delete`, or that has no workspace disk, is not released, and nothing is sent.
   - A VM of another `vmId`, a VM or a workspace disk of another operation, and an invalid operation id are refused.
   - A missing workspace disk is refused. Horizon does not start a new VM on an empty disk.

### 6.19 AZV-19 — Check the rebuild records through the deployment records

1. Run the loopback rebuild tests, the dispatch test and the reconnect tests.

   ```sh
   cargo test -p horizon-core --lib -- cloud_runtime::deployment::azure::tests::rebuild cloud_runtime::deployment::replacement::live cloud_runtime::deployment::reconnect
   ```

   Result: All tests pass. These tests examine the items that follow:
   - A release sends one `DELETE .../virtualMachines/worker`. The record keeps the stage `Replace`, the bound `vmId` and no stop. The worker has no address. `azure.json` does not change, and `retained` stays true.
   - A retry after a `DELETE` that ARM finished sends no second `DELETE`. It records the release.
   - A VM whose data disk has no `deleteOption: Detach` is not released. `azure-release.json` is not written.
   - A cloud whose `azure.json` has `deleting` is not released.
   - `reopen` refuses a VM that was not released. After the release, it saves the fence `Prepared` and no worker. `released` is cleared, and `held` stays true.
   - After `reopen`, a deploy sends one `PUT` of the deployment and no delete. It binds a new `vmId` with the same address. The new VM attaches `worker-workspace` on LUN 0 with `createOption: Attach`.
   - A new-server rebuild selects the Hetzner release for Hetzner and the Azure release for Azure. A rebuild in place has no release. RunPod and a Docker host have no new-server release.
   - A reconnect of an Azure record without a bound VM is refused. With `held` in `azure-release.json`, it is admitted. With `deleting` in `azure.json`, it is refused.
   - Before the release, the rebuild checks the pull login of the next VM from the `azure` settings. It does not need a `hetzner` section. A login for another registry than the image's is refused.

### 6.20 AZV-L1 — Create one live Azure worker

Do this task on the computer that has the Azure subscription. Use a new cloud id for each run, for example `azv-l1-01`.

1. Make the token file and the settings file in a new private directory.

   ```sh
   umask 077
   export AZV="$HOME/azv-l1"
   mkdir -p "$AZV/state"
   az account get-access-token --query accessToken -o tsv > "$AZV/arm-token"
   export SUB=$(az account show --query id -o tsv)
   ```

   Result: `$AZV/arm-token` holds one token. `$SUB` is a lowercase UUID.

2. Write `$AZV/settings.json`. Use your SSH identity, which must have a `.pub` file beside it.

   ```sh
   jq -n --arg token "$AZV/arm-token" --arg sub "$SUB" --arg id "$HOME/.ssh/id_ed25519" '{
     runpod_key_file: "/nonexistent", ssh_identity_file: $id, docker_config: "/nonexistent",
     registry_pull_auth_id: null, cpu_flavors: [], gpu_types: [],
     azure: {token_file: $token, subscription_id: $sub, vm_sizes: ["Standard_D4s_v3"], regions: ["northeurope"]}
   }' > "$AZV/settings.json"
   ```

   Result: The file exists. Add `azure.registry_pull` if the image is private.

3. Run the live create.

   ```sh
   HORIZON_AZURE_LIVE_SETTINGS="$AZV/settings.json" HORIZON_AZURE_LIVE_STATE="$AZV/state" \
   HORIZON_AZURE_LIVE_CLOUD=azv-l1-01 HORIZON_AZURE_LIVE_IMAGE='<registry>/<worker>@sha256:<digest>' \
   cargo test -p horizon-core --lib -- --ignored --exact cloud_runtime::deployment::azure::tests::live_create --nocapture
   ```

   Result: The test passes in less than 15 minutes. It prints a worker with `desiredStatus` `RUNNING`, a `publicIp`, `portMappings` `{"22": 22}`, `vcpuCount` 4, `memoryInGb` 16, `containerDiskInGb` 30 and `volumeInGb` 64.

4. Examine the resources and their tags.

   ```sh
   az resource list -g horizon-ws-azv-l1-01 --query "[].{type:type,name:name,tag:tags.\"horizon-operation\"}" -o table
   az group show -n horizon-ws-azv-l1-01 --query tags
   ```

   Result: The list shows the network security group, virtual network, public address, network interface, data disk, OS disk and VM. Each has the tag `azv-l1-01`. The group has the same tag.

5. Examine the network rules and the address.

   ```sh
   az network nsg rule list -g horizon-ws-azv-l1-01 --nsg-name worker-nsg -o table
   az network public-ip show -g horizon-ws-azv-l1-01 -n worker-ip --query "{sku:sku.name,method:publicIPAllocationMethod}"
   ```

   Result: One rule allows TCP 22 inbound. The address is `Standard` and `Static`.

6. Connect to the worker. Use the address of step 3.

   ```sh
   ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -i "$HOME/.ssh/id_ed25519" root@<publicIp> 'findmnt /workspace; echo marker > /workspace/azv-l1; sha256sum /workspace/azv-l1'
   ```

   Result: The worker container answers on port 22. `/workspace` is a mount. Record the SHA-256.

7. Examine the host from the Azure run command, because host SSH is masked.

   ```sh
   az vm run-command invoke -g horizon-ws-azv-l1-01 -n worker --command-id RunShellScript --scripts \
     'ls -l /dev/disk/azure/scsi1/; findmnt /mnt/horizon-volume; iptables -S DOCKER-USER; ip6tables -S FORWARD; wc -c /var/lib/cloud/instance/user-data.txt'
   ```

   Result: `lun0` exists. `/mnt/horizon-volume` is ext4 on that disk. `DOCKER-USER` drops `169.254.0.0/16` and `168.63.129.16/32`. `FORWARD` drops IPv6 from `docker0`. Record the size of the user data.

8. Make sure that the container cannot reach the metadata addresses.

   ```sh
   az vm run-command invoke -g horizon-ws-azv-l1-01 -n worker --command-id RunShellScript --scripts \
     'docker exec horizon-worker sh -c "curl -s -m 5 -H Metadata:true http://169.254.169.254/metadata/instance?api-version=2021-02-01; echo rc=\$?; curl -s -m 5 http://168.63.129.16/?comp=versions; echo rc=\$?"'
   ```

   Result: Both requests time out (`rc=28`), or `curl` is not in the image. Record which.

9. Restart the container and examine the marker.

   ```sh
   az vm run-command invoke -g horizon-ws-azv-l1-01 -n worker --command-id RunShellScript --scripts 'systemctl restart horizon-worker'
   ssh -o BatchMode=yes -i "$HOME/.ssh/id_ed25519" root@<publicIp> 'sha256sum /workspace/azv-l1'
   ```

   Result: The SHA-256 is the same as in step 6.

10. Run the live create again with the same state.

    ```sh
    HORIZON_AZURE_LIVE_SETTINGS="$AZV/settings.json" HORIZON_AZURE_LIVE_STATE="$AZV/state" \
    HORIZON_AZURE_LIVE_CLOUD=azv-l1-01 HORIZON_AZURE_LIVE_IMAGE='<registry>/<worker>@sha256:<digest>' \
    cargo test -p horizon-core --lib -- --ignored --exact cloud_runtime::deployment::azure::tests::live_create --nocapture
    ```

    Result: The test passes and prints the same worker `id`. It prints no `mutation Pending` line, because a bound worker is only read.

11. Do a lost create with a new cloud id and a new state directory. Start the command below. Stop it with `Ctrl+C` 15 seconds after the second `mutation Pending` line, while the deployment runs. Then start the same command again.

    ```sh
    HORIZON_AZURE_LIVE_SETTINGS="$AZV/settings.json" HORIZON_AZURE_LIVE_STATE="$AZV/state2" \
    HORIZON_AZURE_LIVE_CLOUD=azv-l1-02 HORIZON_AZURE_LIVE_IMAGE='<registry>/<worker>@sha256:<digest>' \
    cargo test -p horizon-core --lib -- --ignored --exact cloud_runtime::deployment::azure::tests::live_create --nocapture
    ```

    Result: After the stop, `$AZV/state2/deployment.json` has the operation `requested`. The second run prints one `mutation Pending` line for the unresolved create, passes and prints a worker.
    If you stop the first run before ARM gets the PUT, the second run gives "Worker creation is unresolved; no second allocation was attempted". This is also correct: Horizon does not send the create again. Record which result occurred.

12. Count the deployment writes of `azv-l1-02`.

    ```sh
    az monitor activity-log list -g horizon-ws-azv-l1-02 --offset 1h \
      --query "[?operationName.value=='Microsoft.Resources/deployments/write' && status.value=='Started'] | length(@)"
    az vm list -g horizon-ws-azv-l1-02 --query "length(@)"
    ```

    Result: Both values are `1`. If step 11 gave "Worker creation is unresolved", both values are `0`.

13. Keep the worker of `azv-l1-01` and the state in `$AZV`. Task AZV-L2 stops, resumes and deletes them.

### 6.21 AZV-L2 — Stop, resume and delete the live worker

Do this task after task AZV-L1, in the same terminal, with `$AZV` and `$SUB` set. Set the variables of the live tests.

```sh
export HORIZON_AZURE_LIVE_SETTINGS="$AZV/settings.json" HORIZON_AZURE_LIVE_STATE="$AZV/state"
export RG=horizon-ws-azv-l1-01
```

1. Record the VM before the stop.

   ```sh
   az vm show -d -g "$RG" -n worker --query "{power:powerState,ip:publicIps,vmId:vmId}" -o json
   date -u +%FT%TZ
   ```

   Result: `power` is `VM running`. Record `ip`, `vmId` and the time.

2. Stop the cloud.

   > **CAUTION:** THIS STOP ENDS THE PROCESSES IN THE WORKER.

   ```sh
   cargo test -p horizon-core --lib -- --ignored --exact cloud_runtime::deployment::azure::tests::live_stop --nocapture
   ```

   Result: The test passes in less than 10 minutes. It prints the stage `Stopped`, `stop_requested true`, the outcome `Inactive` and a worker with `desiredStatus` `EXITED` and the same `id`.

3. Make sure that Azure deallocated the VM and kept its address.

   ```sh
   az vm get-instance-view -g "$RG" -n worker --query "instanceView.statuses[?starts_with(code,'PowerState')].code" -o tsv
   az vm show -d -g "$RG" -n worker --query "{power:powerState,ip:publicIps,vmId:vmId}" -o json
   az network public-ip show -g "$RG" -n worker-ip --query "{address:ipAddress,method:publicIPAllocationMethod}" -o json
   ```

   Result: The first command prints `PowerState/deallocated`. `power` is `VM deallocated`. `ip` and `vmId` are the values of step 1. The address is `Static`.

4. Make sure that Horizon sent a deallocate and no power off.

   ```sh
   az monitor activity-log list -g "$RG" --offset 1h \
     --query "[?status.value=='Started'].operationName.value" -o tsv | sort | uniq -c
   ```

   Result: The list has one `Microsoft.Compute/virtualMachines/deallocate/action`. It has no `Microsoft.Compute/virtualMachines/powerOff/action`.

5. Keep the VM deallocated for 60 minutes. Record the time when you go to the next step.

   Result: The VM stays `VM deallocated`. Azure bills no compute for it in this time. Step 13 examines the bill.

6. Resume the cloud.

   > **CAUTION:** THIS RESUME RENTS THE VM AGAIN.

   ```sh
   cargo test -p horizon-core --lib -- --ignored --exact cloud_runtime::deployment::azure::tests::live_resume --nocapture
   ```

   Result: The test passes in less than 15 minutes. It prints a worker with `desiredStatus` `RUNNING`, the same `id` and the same `publicIp` as in step 1.

7. Make sure that the same VM runs with the same workspace.

   ```sh
   az vm show -d -g "$RG" -n worker --query "{power:powerState,ip:publicIps,vmId:vmId}" -o json
   ssh -o BatchMode=yes -i "$HOME/.ssh/id_ed25519" root@<ip> 'findmnt /workspace; sha256sum /workspace/azv-l1'
   ```

   Result: `power` is `VM running`, and `ip` and `vmId` are the values of step 1. The SHA-256 is the value of task AZV-L1 step 6.

8. Delete the cloud.

   > **CAUTION:** THIS DELETE REMOVES THE RESOURCE GROUP, THE VM, BOTH DISKS AND THE ADDRESS. THE WORKSPACE FILES ARE LOST.

   ```sh
   az group show -n "$RG" --query tags
   cargo test -p horizon-core --lib -- --ignored --exact cloud_runtime::deployment::azure::tests::live_delete --nocapture
   ```

   Result: The tag is `{"horizon-operation": "azv-l1-01"}`. The test passes in less than 15 minutes. It prints a journal with `"deleting": true`, `"swept": true` and the group `terminated` with the id `/subscriptions/<SUB>/resourceGroups/horizon-ws-azv-l1-01`.

9. Make sure that the group and each tagged resource are absent.

   ```sh
   az group exists -n "$RG"
   az resource list --tag horizon-operation=azv-l1-01 -o json
   az disk list --query "[?contains(id,'horizon-ws-azv-l1-01')].id" -o json
   az network public-ip list --query "[?contains(id,'horizon-ws-azv-l1-01')].id" -o json
   ```

   Result: The first command prints `false`. The other commands print `[]`.

10. Examine the records.

    ```sh
    jq '{stage, operation}' "$AZV/state/deployment.json"
    jq . "$AZV/state/azure.json"
    ```

    Result: The stage is `Deleted`. The operation is `terminated` with the `vmId` of step 1. `azure.json` has `deleting` and `swept`.

11. Run the delete again with the same state.

    ```sh
    cargo test -p horizon-core --lib -- --ignored --exact cloud_runtime::deployment::azure::tests::live_delete --nocapture
    ```

    Result: The test passes at once.

12. Delete the cloud of task AZV-L1 step 11 the same way.

    ```sh
    HORIZON_AZURE_LIVE_STATE="$AZV/state2" cargo test -p horizon-core --lib -- --ignored --exact cloud_runtime::deployment::azure::tests::live_delete --nocapture
    az group exists -n horizon-ws-azv-l1-02
    az resource list --tag horizon-operation=azv-l1-02 -o json
    ```

    Result: The test passes. The commands print `false` and `[]`. If step 11 of task AZV-L1 gave "Worker creation is unresolved", the test also passes.

13. On the next day, examine the bill of the group for the day of the test.

    ```sh
    az consumption usage list --include-meter-details --start-date <day> --end-date <day> \
      --query "[?contains(instanceId,'horizon-ws-azv-l1-01')].{meter:meterDetails.meterName,quantity:quantity}" -o table
    ```

    Result: The quantity of the VM compute meter (`D4s v3`) is less than the hours from step 1 to step 8, by at least the time of step 5. The disk meters and the `Standard IPv4 Static Public IP` meter continue through step 5. No meter of the group has use after step 8. If the command is not available, use Cost analysis in the Azure portal with the scope of the subscription, the daily granularity and the resource group as a filter.

## 7. Pass criteria

- Tasks AZV-01 to AZV-19 pass and send no request to Azure.
- For the live acceptance, each step of tasks AZV-L1 and AZV-L2 gives its result. AZV-L2 proves each group absent.

## 8. Cleanup

Tasks AZV-01 to AZV-19 make temporary data only. Task AZV-L2 deletes the groups of task AZV-L1. If a run stopped before AZV-L2 ended, do these steps:

1. Examine the tag of each test group that remains. Then delete the group.

   ```sh
   az group show -n horizon-ws-azv-l1-01 --query tags
   az group delete -n horizon-ws-azv-l1-01 --yes
   az group show -n horizon-ws-azv-l1-02 --query tags
   az group delete -n horizon-ws-azv-l1-02 --yes
   ```

   Result: Each command ends without an error.

2. Make sure that the groups are absent.

   ```sh
   az group exists -n horizon-ws-azv-l1-01
   az group exists -n horizon-ws-azv-l1-02
   ```

   Result: Each command prints `false`.

3. Delete the token file and the test state.

   ```sh
   rm -rf "$AZV"
   ```

   Result: The directory is absent.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
