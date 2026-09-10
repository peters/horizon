# Azure workspace readiness: preflight contract and spike checklist

Permanent operator guidance for the Azure CPU lane of
[#474](https://github.com/peters/horizon/issues/474) under
[#383](https://github.com/peters/horizon/issues/383). It records the dated
official-source comparison behind `scripts/azure-workspace-preflight/preflight.py`,
the qualification gates that no API read can close, the maintainer's
revalidation decision, and the results of the three-sample Linux VM spike executed
on 2026-09-10 with `scripts/azure-workspace-spike`. Spike resources were created
under a stated bound and deleted with inventory proof; the persistent registry and
pull identity are listed below. Container Apps Jobs are excluded from this lane by
the issue contract and are not compared.

Status legend used below: **documented** means stated by the linked official
page on the date given; **not verified in this pass** means the author did not
locate an official statement and the spike must measure or confirm it;
**not executed** marks steps that have never been run.

## Product contract the candidates are measured against

From #383 and #474: remote execution and durable data are independent of the
client PC; closing panels, exiting Horizon or losing connectivity only detaches;
reconnect is non-creating; Stop preserves promised durable data; Delete is a
separate explicit action; the measured create, pull, endpoint and SSH-ready path
must fit the 180-second boundary; image access uses a managed identity without
registry passwords; and the worker's repository storage must pass the existing
on-worker qualifier before any repository publication.

The qualifier in `crates/horizon-core/src/repository_overlay/storage.rs` accepts
only an ext4 filesystem whose kernel-reported options, read from
`/proc/fs/ext4/<device>/options` through trusted sysfs metadata, contain `rw`,
`barrier` and exactly one `data=` entry that is `data=ordered` or `data=journal`,
with no `ro` or `nobarrier` line; other non-contradictory options are permitted.
This is an **on-worker qualification gate**. An Azure disk SKU, an SMB or NFS share, an
`emptyDir` volume or any "persistent" marketing claim does not satisfy it, and the
qualifier must not be relaxed or duplicated to make a candidate pass.

## Official source comparison (checked 2026-09-10)

| Requirement | Container Instances container group | Linux virtual machine with managed disk | Container Apps long-running app |
| --- | --- | --- | --- |
| Direct key-only SSH | Not verified in this pass. A container group can expose TCP ports on a public IP, so SSH depends on the worker image running `sshd`; the stop/start page notes the IP address can change on restart. | Not verified in this pass for exact CLI flags; VM SSH is the standard access path and key material would be supplied at creation. Public IP retention across deallocate must be measured. | Documented: external TCP ingress "is only supported for Container Apps environments that use a virtual network" ([ingress overview](https://learn.microsoft.com/en-us/azure/container-apps/ingress-overview)); extra TCP ports need the `containerapp` CLI extension. |
| Independent sessions while the PC is off | Documented as compute that keeps running independently of a client; whether the worker's own sessions survive depends on the image and must be measured. | Same; depends on the worker image and must be measured. | Replica lifetime is controlled by revisions and scale rules; not verified in this pass whether a single long-lived replica can be guaranteed without scale-to-zero. |
| Explicit Stop with retained data | Documented gap: "When a container group enters the Stopped state, it terminates and recycles all the containers in the group. It doesn't preserve container state." ([stop/start](https://learn.microsoft.com/en-us/azure/container-instances/container-instances-stop-start)). `emptyDir` "isn't persisted" across stop ([emptyDir](https://learn.microsoft.com/en-us/azure/container-instances/container-instances-volume-emptydir)). Retained data would have to live on an Azure Files share, which is CIFS-only, key-authenticated and requires root ([Azure Files volume](https://learn.microsoft.com/en-us/azure/container-instances/container-instances-volume-azure-files)). | Documented: a deallocated VM "has released the lease on the underlying hardware" and is not billed for compute while "Disks and Networking continue to incur charges" ([states and billing](https://learn.microsoft.com/en-us/azure/virtual-machines/states-billing)), so disk data is retained across the explicit deallocate. | Not verified in this pass. Container and replica storage are ephemeral ("Data is available until container/replica shuts down"); only Azure Files (SMB or NFS) persists ([storage mounts](https://learn.microsoft.com/en-us/azure/container-apps/storage-mounts)). |
| Restart or replacement behavior | Documented: start "begins a new deployment with the same container configuration" and a new image is pulled if updated; the IP can change. | Documented: deallocate/start cycles keep the same VM resource; provisioning and power states are separate ([states and billing](https://learn.microsoft.com/en-us/azure/virtual-machines/states-billing)). | Revision-based replacement; not verified in this pass. |
| Image access through managed identity | Documented: user-assigned identity only; "Azure Container Instances doesn't support system-assigned managed identity-authenticated image pulls with ACR" ([ACR with managed identity](https://learn.microsoft.com/en-us/azure/container-instances/using-azure-container-registry-mi)). | Documented: user-assigned or system-assigned identity with `AcrPull` or `Container Registry Repository Reader`, then `az login --identity` and `az acr login` on the VM ([ACR managed identity auth](https://learn.microsoft.com/en-us/azure/container-registry/container-registry-authentication-managed-identity)). The worker image must be pulled by a runtime on the VM, which adds a step to the timing budget. | Documented at the platform level (managed identity is the documented registry path), not re-verified here. |
| 180-second measured readiness | No official startup figure. Must be measured. | No official startup figure. Must be measured (create, boot, identity token, pull, `sshd` ready). | No official startup figure. Must be measured. |
| Storage qualification (ext4 + kernel options) | Not satisfiable by any documented ACI volume type by description: `emptyDir` is ephemeral, Azure Files is CIFS. Only an on-worker check can decide. | Plausible with an ext4-formatted data disk, but only the on-worker qualifier decides. | Not satisfiable by documented volume types by description (ephemeral or Azure Files SMB/NFS). |
| Quota and capability reads | [Location - List Usage](https://learn.microsoft.com/en-us/rest/api/container-instances/location/list-usage) and [List Capabilities](https://learn.microsoft.com/en-us/rest/api/container-instances/location/list-capabilities) (`api-version=2026-07-01`); defaults of 100 standard container groups and 100 cores per region per subscription, x64 images only, regional capacity failures are documented as possible even within quota ([quota limits](https://learn.microsoft.com/en-us/azure/container-instances/container-instances-resource-and-quota-limits)). | [az vm list-usage](https://learn.microsoft.com/en-us/cli/azure/vm?view=azure-cli-latest#az-vm-list-usage) and [az vm list-skus](https://learn.microsoft.com/en-us/cli/azure/vm?view=azure-cli-latest#az-vm-list-skus) (restricted SKUs are hidden unless `--all`). | [Usages - List](https://learn.microsoft.com/en-us/rest/api/resource-manager/containerapps/usages/list?view=rest-resource-manager-containerapps-2025-07-01) (`api-version=2025-07-01`; a live subscription-scoped read on 2026-09-10 returned `ManagedEnvironmentCount`, `SessionPools`, `ExpressEnvironmentCount` and `SandboxCores` but no `ManagedEnvironmentCores`, so per-environment core quota remains a spike-time check). |

Shared operations: [az account show](https://learn.microsoft.com/en-us/cli/azure/account?view=azure-cli-latest#az-account-show),
[az provider show](https://learn.microsoft.com/en-us/cli/azure/provider?view=azure-cli-latest#az-provider-show),
[az rest](https://learn.microsoft.com/en-us/cli/azure/reference-index?view=azure-cli-latest#az-rest) and
[Subscriptions - List Locations](https://learn.microsoft.com/en-us/rest/api/resources/subscriptions/list-locations)
(`api-version=2022-12-01`). Lifecycle verbs that the tool forbids are the ones
documented under [az container](https://learn.microsoft.com/en-us/cli/azure/container?view=azure-cli-latest)
(`create`, `delete`, `start`, `stop`, `restart`, `exec`, `attach`), plus
`az provider register`, `az account set`, `az login` and `az extension add`.

What this comparison does not do: it does not pick a winner. Advertised startup
time and quota availability are not architecture evidence. The documented Stop
semantics of Container Instances and the CIFS-only durable volume are the most
important facts to carry into the revalidation discussion, but the decision
belongs to the spike results and the lead.

Excluded from comparison: Container Apps Jobs (forbidden by #474), Azure
Kubernetes Service (cluster overhead is out of scope for a single CPU worker),
dynamic sessions and other ephemeral sandboxes (no persistent identity).

## Preflight before any paid step

1. Run the focused tests, then the offline plan for the exact candidate, region
   and subscription that the spike will use. Read every planned command.
2. Run `--live` once per candidate under consideration with `--report` to a
   private path. Keep reports private; they are redacted but still describe the
   account's quota posture.
3. Resolve every `blocked` result before proceeding. `unregistered_provider`
   requires explicit approval for provider registration; the tool never registers.
4. Treat every `unknown` result as a question for the operator, not as headroom.
5. Record the report path, `observed_at`, tool version and commit in the resource
   journal that the spike will use.

## Revalidation decision (2026-09-10)

Maintainer decision, taken after the official-source comparison above and the live
read-only preflight of the maintainer's subscription: the Azure CPU candidate for
the spike and the adapter is a **Linux virtual machine with an ext4-formatted
managed data disk**, running the compact worker image under Docker with the data
disk bound at `/workspace`. Reasons recorded at decision time:

- Container Instances: Stop does not preserve container state, the only durable
  volume type is CIFS (cannot satisfy the ext4 qualifier by construction), and
  `Microsoft.ContainerInstance` is not registered in the subscription (live
  preflight: `blocked`, `unregistered_provider`).
- Container Apps: storage is ephemeral or Azure Files; external TCP ingress needs a
  VNet-integrated environment; core quota is per environment.
- Linux VM: deallocate retains disks, needs no new provider registration, and the
  live preflight reported no blockers for `Standard_D4s_v3` in `northeurope`.

Supporting infrastructure created under the same decision (persistent, outside any
sample resource group): resource group `horizon-worker-registry` in `northeurope`
with a Standard-SKU container registry (admin user disabled) and the user-assigned
identity `horizon-worker-puller` holding `AcrPull` only. The worker image built from
commit `22674061` was pushed as `horizon-remote-worker:0.1.0-22674061` with digest
`sha256:20cc03ef2530336b7374cc35412c8583b1422726c630ec6e6cd1450d690a74f6`
(1.66 GB uncompressed, 31 layers). No automated publication workflow exists yet.

## Three-sample spike (executed 2026-09-10)

Harness: [`scripts/azure-workspace-spike/run-vm-spike.sh`](../../scripts/azure-workspace-spike/README.md).
Configuration for all samples: `northeurope`, `Standard_D4s_v3`,
`Canonical:ubuntu-24_04-lts:server:latest`, 32 GiB data disk, Standard static public
IP, VM port 2222 opened only from the operator's egress address, key-only SSH with a
fresh Ed25519 key per sample, image pulled by digest through the managed identity's
IMDS token exchanged at the registry (no registry password), host key read out of band
through ARM-authenticated `az vm run-command` and pinned before the first SSH.
Bound: 120 minutes per sample; actual 9 to 10 minutes each. Journals are private.

Timings in seconds from `T0` (the resource-group create call), controller side:

| Sample | Create ack | Endpoint open | SSH verified | SSH verified excl. run-command | Deallocate | Start to SSH | Delete |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 36.4 | 203.3 | 235.4 | 204.0 | 32.1 | 74.7 | 188.5 |
| 2 | 39.1 | 178.6 | 210.7 | 179.2 | 33.6 | 74.8 | 187.5 |
| 3 | 38.4 | 238.7 | 270.8 | 239.3 | 32.1 | 74.2 | 187.8 |

Guest-side stamps (seconds from `T0`):

| Sample | Boot | Docker installed | Data disk ready | Registry login | Image pulled | Container started |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 23.5 | 67.4 | 69.3 | 70.4 | 160.3 | 201.9 |
| 2 | 25.0 | 73.2 | 75.0 | 76.0 | 169.2 | 174.0 |
| 3 | 26.9 | 76.8 | 79.1 | 80.1 | 229.1 | 235.0 |

Sample 1's 42 s between pull and container start did not recur once the harness
split `docker create` from `docker start` (4 to 6 s and 0.4 s in samples 2 and 3);
it is recorded as an unexplained one-off. Raising the registry from Basic to
Standard before sample 2 did not change pull time, and sample 3 pulled 60 s slower
than the others, so pull time is dominated by image size and VM-side extraction,
not registry tier.

Functional results, identical in all three samples:

- Storage: `/workspace` is the bound ext4 data disk (`/dev/sdc`, sysfs path
  resolvable from inside the container); kernel options include `rw`, `barrier`,
  exactly one `data=ordered` and no `ro`/`nobarrier`, so a shell mirror of the Rust
  qualifier passes. **The authoritative Rust qualifier has not been executed on the
  worker yet**; that remains a gate for the adapter's first real repository setup.
- Detach independence: a tmux heartbeat kept writing while no client was connected.
- Explicit Stop with retained data: `az vm deallocate` reached
  `PowerState/deallocated`; after `az vm start` the marker file was present, the
  public IP was unchanged, the pinned host key matched (it lives on the data disk),
  and the tmux session was gone because the container restarted. Data survives Stop;
  in-container sessions do not.
- Exact deletion: the resource group was absent and the subscription resource
  inventory was byte-identical to the pre-sample snapshot after every sample.
- No provider was registered and no Container Apps Job was created.

Cost actually incurred: three VMs for about 10 minutes each plus 32 GiB disks, well
under the $10 bound; the persistent registry costs about $0.67 per day at Standard.

### Verdict against the 180-second boundary

**Not met with this configuration.** The slowest sample reached a verified key-only
SSH session 270.8 s after `T0` (239.3 s excluding the 31 s out-of-band host-key read);
even the fastest sample needed 210.7 s. Two of three samples also missed the boundary
for endpoint-open alone. The measured budget splits into roughly 25 s VM boot, 45 to
50 s Docker installation from apt, 90 to 150 s image pull and extraction, and 31 s
for `run-command`. Levers that the measurements point to, in order of impact:

1. A smaller worker image or a VM image with the worker image pre-pulled (removes
   most of the 90 to 150 s pull).
2. A prebaked VM image (Azure Compute Gallery) with Docker installed (removes the
   45 to 50 s apt step).
3. A faster authenticated host-key channel than `run-command`, for example the
   worker publishing its bootstrap record to the serial console read through boot
   diagnostics, or the adapter accepting the retained key from the data disk on
   reconnect (removes up to 31 s from the first connection only).

None of these are approved or implemented; they are the next decision for #474.
The functional contract (key-only SSH, managed-identity pull, ext4 storage,
detach independence, Stop with retained data, exact deletion) held in every sample.

## Spike checklist status

Executed on 2026-09-10 unless marked otherwise.

- [x] Candidate revalidation note (above).
- [x] Exact candidate image by digest (above).
- [x] Bounded cost and lifetime (120 minutes per sample, $10 total).
- [x] Resource ownership journal: one tagged resource group per sample, pre- and
      post-inventory snapshots, private journals.
- [x] Fresh client key per sample; key-only SSH; no password authentication.
- [x] User-assigned managed identity with pull-only registry rights.
- [x] `T0`, create, pull, endpoint, SSH-ready and delete timing per sample.
- [x] Host key verified out of band and pinned before the first connection.
- [x] Kernel ext4 option lines recorded from the worker; shell mirror passes.
      **Not executed:** the Rust qualifier itself on the worker.
- [x] Independent execution across a disconnect.
- [x] Explicit Stop with retained data, endpoint retention recorded.
- [x] Exact deletion with absence check and unchanged inventory.
- [x] Slowest sample compared with the 180-second boundary: **not met** (270.8 s).
- [x] Cost recorded against the bound.
- [ ] **Not executed:** PC-off task progress over hours, three independent panels on
      one worker, verified-loss recovery, opt-in budget policy, and the adapter's own
      create-once and reconcile behaviour. These are #474 acceptance items for the
      adapter and integration PRs, not for this spike.

## Maintenance

Re-check the linked pages and API versions when the tool's pinned
`api-version` values change or when Microsoft retires one. Keep the tool's
allowlist and this document in step: a new check needs a documented read-only
operation and an entry in the table above.
