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

- Candidate: a pull request of #1774 that changes `horizon_cloud::host` or `horizon_cloud::azure::template`.
- Platform: Linux with the repository Rust toolchain.
- The tests use synthetic specs only. They need no network and no Azure account.
- The trust boundary is in [the Azure provisioning architecture](../../architecture/azure-provisioning.md).
- This procedure does not test these items:
  - A live Azure VM. The ARM template is examined as data only.
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
   - The guard drops container traffic to the Azure platform address, `168.63.129.16`.
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

## 7. Pass criteria

- All tasks pass.
- No step sends a request to Azure.

## 8. Cleanup

No cleanup is necessary. The tests make temporary data only.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
