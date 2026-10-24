---
procedure: azure-provisioning
feature: The Azure worker bootstrap, its ARM deployment and the creation of an Azure worker
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Azure provisioning test procedure

## 1. Purpose

This procedure makes sure that `horizon-cloud` plans an Azure worker correctly before it sends a request.
It examines the host bootstrap, the metadata guard, the custom data limit and the ARM deployment template.

## 2. Applicability

- Candidate: a pull request of #1774 that changes `horizon_cloud::host`, `horizon_cloud::azure::template`, `horizon_cloud::azure::provision`, the `azure` cloud settings or the Azure provider arms.
- Platform: Linux with the repository Rust toolchain.
- The tests use synthetic specs only. They need no network and no Azure account.
- The trust boundary is in [the Azure provisioning architecture](../../architecture/azure-provisioning.md).
- This procedure does not test these items:
  - A live Azure VM. The ARM template is examined as data only. Recorded ARM answers stand in for Azure.
  - The stop, resume, delete and rebuild of an Azure cloud. They stay refused.
  - Azure in the New cloud dialog. `offers::AZURE_DEPLOYABLE` stays `false`.

## 3. Safety

No step rents compute, deletes data, sends a secret or changes access.

## 4. Equipment and preconditions

- A checkout of the candidate commit.
- The repository's supported Rust toolchain.
- No Azure account, Azure credential or provider key.

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
   - With an `azure` section, `cpu_flavors` and `data_centers` give the VM sizes and regions in the settings order.
   - `preflight` still gives "Horizon cannot create clouds on Azure yet", and `offers::AZURE_DEPLOYABLE` is `false`.
   - Each lifecycle operation still refuses with "Horizon cannot manage Azure clouds yet".

## 7. Pass criteria

- All tasks pass.
- No step sends a request to Azure.

## 8. Cleanup

No cleanup is necessary. The tests make temporary data only.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
