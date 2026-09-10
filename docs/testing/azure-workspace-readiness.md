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
with a Standard-SKU container registry (admin user disabled), the user-assigned
identity `horizon-worker-puller` holding `AcrPull` only, and the Automation account
`horizon-spike-reaper` whose system identity holds the built-in power-only role
`Desktop Virtualization Power On Off Contributor` at subscription scope and runs the
spike reaper every 15 minutes (Basic tier; about 100 short jobs per day). The worker image built from
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
Bound: 120 minutes per sample (60 for samples 14 and 15, 90 for samples 16 to 18); actual 8 to 13 minutes for samples 1, 2, 3, 5 to 12, 14, 15, 17 and 18, 17 minutes for sample 4 (failed restart wait), 30 minutes for sample 13 (stalled guest, interrupted by hand) and 26 minutes for sample 16 (includes the 14-minute reaper wait). Journals are private.

Controller-side timings in seconds. The first four columns are offsets from `T0`
(the resource-group create call); the last three are self-anchored durations of
the deallocate call, the start call until a verified SSH session, and the
resource-group delete.

| Sample | Create ack (from T0) | Endpoint open (from T0) | SSH verified (from T0) | SSH verified excl. run-command (from T0) | Deallocate duration | Start to SSH duration | Delete duration |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 36.4 | 203.3 | 235.4 | 204.0 | 32.1 | 74.7 | 188.5 |
| 2 | 39.1 | 178.6 | 210.7 | 179.2 | 33.6 | 74.8 | 187.5 |
| 3 | 38.4 | 238.7 | 270.8 | 239.3 | 32.1 | 74.2 | 187.8 |
| 4 (revised harness, see below) | 67.9 | 241.6 | 274.2 | 242.2 | 31.5 | restart gate failed | 183.3 |
| 5 (revised harness) | 38.8 | 243.0 | 275.6 | 243.7 | 31.5 | 74.3 | 182.9 |
| 6 (final harness) | 37.6 | 180.8 | 213.5 | 181.5 | 31.9 | 105.9 | 183.1 |
| 7 (final harness) | 37.0 | 170.9 | 203.4 | 171.5 | 31.9 | 77.1 | 212.8 |
| 8 (final harness, see below) | 37.5 | 203.1 | 235.8 | 203.8 | 31.9 | 75.3 | 122.4 |
| 9 (final harness, see below) | 66.5 | 245.4 | 278.0 | 246.0 | 32.5 | 75.0 | 183.0 |
| 10 (final harness) | 38.1 | 210.6 | 243.7 | 211.2 | 32.4 | 74.4 | 183.4 |
| 11 (single deployment) | 35.9 | 188.4 | 221.0 | 189.0 | 32.9 | 75.3 | 182.9 |
| 12 (exact PR head 436fb9ed) | 37.5 | 184.0 | 247.7 | 184.7 | 32.0 | 105.6 | 242.8 |
| 13 (early-boot timer, interrupted) | 40.0 | never | never | never | not reached | not reached | 243.4 |
| 14 (guest lifetime bound) | 56.4 | 191.8 | 224.4 | 192.5 | 32.5 | 75.8 | 243.1 |
| 15 (timer verified before firing) | 38.7 | 194.2 | 226.8 | 194.8 | 32.5 | 75.5 | 243.0 |
| 16 (reaper proof, `--prove-reaper`) | 52.1 | 189.4 | 222.5 | 190.0 | 32.5 | 74.6 | 212.8 |
| 17 (harness head fb5ed0a3, `--prove-reaper`) | 40.3 | 186.7 | 219.8 | 187.4 | 32.0 | 75.7 | 243.2 |
| 18 (exact final harness head b3b3d926, subscription-pinned reaper, `--prove-reaper`) | 39.9 | 189.3 | 222.4 | 189.9 | 32.9 | 74.8 | 182.8 |

Guest-side stamps, in seconds after `T0` using the VM's own clock (Azure guests
sync to host time, but treat sub-second differences between the two tables as
cross-clock noise):

| Sample | Boot | Docker installed | Data disk ready | Registry login | Image pulled | Container started |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 23.5 | 67.4 | 69.3 | 70.4 | 160.3 | 201.9 |
| 2 | 25.0 | 73.2 | 75.0 | 76.0 | 169.2 | 174.0 |
| 3 | 26.9 | 76.8 | 79.1 | 80.1 | 229.1 | 235.0 |
| 4 | 33.4 | 101.5 | 103.7 | 104.6 | 221.4 | 237.3 |
| 5 | 28.7 | 77.1 | 79.3 | 80.2 | 216.2 | 239.6 |
| 6 | 27.9 | 84.3 | 86.6 | 87.5 | 173.5 | 178.1 |
| 7 | 21.3 | 73.8 | 76.1 | 76.9 | 162.1 | 167.8 |
| 8 | 25.9 | 85.0 | 87.4 | 88.5 | 195.4 | 200.5 |
| 9 | 35.4 | 90.0 | 92.3 | 93.1 | 220.8 | 243.3 |
| 10 | 25.9 | 83.9 | 86.1 | 87.0 | 180.4 | 206.0 |
| 11 | 27.8 | 84.5 | 86.8 | 87.8 | 176.5 | 183.1 |
| 12 | 26.2 | 82.4 | 84.7 | 85.6 | 173.9 | 179.7 |
| 14 | 25.9 | 79.2 | 81.5 | 82.4 | 175.0 | 191.2 |
| 15 | 27.3 | 79.4 | 81.6 | 82.4 | 180.5 | 189.4 |
| 16 | 39.7 | 92.9 | 95.2 | 96.0 | 175.4 | 184.5 |
| 17 | 27.5 | 86.4 | 89.5 | 94.4 | 174.6 | 183.7 |
| 18 | 35.1 | 87.1 | 89.4 | 90.3 | 179.9 | 184.9 |

Sample 1's 42 s between pull and container start did not recur once the harness
split `docker create` from `docker start` (4 to 6 s and 0.4 s in samples 2 and 3);
it is recorded as an unexplained one-off. Raising the registry from Basic to
Standard before sample 2 did not change pull time, and sample 3 pulled 60 s slower
than the others, so pull time is dominated by image size and VM-side extraction,
not registry tier.

Samples 4 and 5 ran the harness after the independent review: `EXIT`-trapped
cleanup, negative results journaled instead of aborting, the host key re-read and
compared after restart, a docker `RequiresMountsFor` drop-in, and the worker's
authoritative qualifier executed over SSH. Sample 4's first iteration also removed
`nofail` from the data-disk fstab entry and added `x-systemd.required-by=docker.service`;
after `az vm start` the worker endpoint never returned within the 10-minute restart
bound, the sample exited 7 with the retention gate failed, and it was still deleted
with an unchanged inventory. The root cause was not captured (the harness now records
guest diagnostics out of band when the restart gate fails); restoring `defaults,nofail`
while keeping the docker drop-in restarted cleanly in sample 5. Sample 6 ran the
final harness (hosted-review fixes: hard lifetime bound on every CLI call, overlay
control gated, unproven deletion outranking other exit codes) and passed every gate,
as did sample 7 after the next review round (bounded SSH, verified-absent group
name, JSON-forced CLI output, journaled deallocate/start failures). Sample 8 added
the platform-side safety net (Azure VM auto-shutdown scheduled one minute after the
lifetime deadline, journaled as scheduled) and passed every functional gate, but
exited 6 because the subscription inventory was not byte-identical afterwards: an
unrelated registry had been created concurrently by another lane. The proof rule was
then corrected to "no pre-existing resource removed and nothing left under the sample
group, additions by other actors counted", which sample 8's snapshots satisfy.
Sample 9 (all functional gates held, verified SSH at 278.0 s) then exposed the
symmetric case: the same unrelated registry was deleted by its own lane during the
run. Because #474 asks for unchanged pre-existing resources and the inventory cannot
attribute a disappearance, the final rule reports such a sample as **unverified**
(exit 6, `unverified_concurrent_removal_outside_group`) rather than proven, and the
sample is rerun. Sample 10, run with that final rule, passed every gate with a fully
proven deletion (`leftover` empty, `removed_outside_group` empty); it was invoked as
`--sample 1` because the harness then accepted single digits, so its tags and journal
directory carry sample number 1 (`sample-1-20260910T174441Z`). Samples 1 to 10 predate
the `--preflight-report` requirement. Samples 11 and 12 ran with a digest-bound live
`vm` preflight report (observed 2026-09-10T18:29:37Z, `no_blockers_observed`) journaled
in their `start` events, and created the network, VM and DevTestLab auto-shutdown
schedule in one server-side deployment (36 to 37 s). Hosted review then pointed out
that ARM deployments are not transactional and the schedule depends on the VM, so
co-location is not a crash-safe bound; a probe confirmed Azure rejects a schedule for a
VM that does not exist yet (`ComputeVmNotFound`). Two defense-in-depth layers were
added next: a built-in power-only role for the worker identity on the sample group,
granted before the deployment, and a guest systemd timer armed by cloud-init as its
first `runcmd` step (54 s after boot in sample 14, after the package module has
installed Docker but before the workspace bootstrap and the pull) that deallocates
the VM through ARM at the deadline. Neither is a pre-creation bound: the
role only authorizes, and the timer depends on cloud-init reaching `runcmd`. Sample 13
tried to arm that timer from `bootcmd`; the early-boot `systemctl` call deadlocked the
guest (no endpoint, no run-command response), the run was interrupted by hand and
deleted cleanly with a proven inventory (exit 130, 30 minutes).
Sample 14 armed the timer from the first `runcmd` step and passed every gate,
including the new one: firing the guest service once took the VM to
`PowerState/deallocated`. Sample 15 additionally verified through run-command that the
timer unit was `enabled` and `active` (waiting) before the service was fired, which
the gate now requires together with the observed deallocation.

The bound that genuinely exists **before any spike compute** is the subscription-side
reaper created once by `scripts/azure-workspace-spike/setup-spike-reaper.sh`: an Azure
Automation runbook (`horizon-spike-deadline-reaper` in account `horizon-spike-reaper`,
resource group `horizon-worker-registry`) that runs every 15 minutes with a
power-only system identity and deallocates every VM tagged
`purpose=horizon-azure-vm-spike` whose `deadline` tag has passed. Each spike VM
carries both tags in the same create call, the harness refuses to create anything
unless that schedule is present and enabled, and a manual job run on 2026-09-10
completed with `reaper done: 0 deallocation request(s)` on a subscription with no
expired spike VMs. Sample 16 proved it end to end: after the restart checks the
harness moved the VM's `deadline` tag two minutes into the past and waited; the
reaper alone deallocated the VM 866 s later (one 15-minute interval), after which the
sample restarted the VM, proved the guest timer as well, and deleted the group with a
proven inventory. Sample 17 repeated the full path on harness commit `fb5ed0a3`: all
gates held, the reaper deallocated the VM 56 s after the tag change because a
scheduled run was imminent, the guest timer was verified and fired, and the deletion
was proven. Hosted review then asked for the reaper to be pinned to the configured
subscription: the runbook now takes a mandatory `SubscriptionId`, connects and sets
its Az context to it and asserts the result, the job link supplies it, and the
harness requires that pinned link (reading links individually, because the list API
omits parameters and Azure re-cases the key to `SubscriptionID`). Sample 18 ran the
full path with `--prove-reaper` on the final harness commit `b3b3d926` (journaled with
no uncommitted changes; every later commit on the branch touches only this document):
all gates held and the scheduled, subscription-pinned reaper deallocated the VM 103 s
after the tag change. Maximum compute exposure after a lost controller is
therefore the deadline plus one reaper interval plus the deallocation time. The run-command host-key read took 63 s in sample 12
(31 to 32 s in every other sample).

Functional results, identical in every sample unless stated:

- Storage: `/workspace` is the bound ext4 data disk (`/dev/sdc`); kernel options
  include `rw`, `barrier`, exactly one `data=ordered` and no `ro`/`nobarrier`, so a
  shell mirror of the Rust qualifier passes. In samples 4 to 12 and 14 to 18 the **authoritative
  qualifier ran on the worker**: `horizon-repository setup-status` against a fresh
  0700 retained root on the data disk returned `status: absent` (qualified storage,
  no claim), and the same request against a 0700 root on the container's overlay
  filesystem returned `status: error` with "retained setup requires supported
  journaled storage and confinement", so the check discriminates as intended.
- Detach independence: a tmux heartbeat kept writing while no client was connected.
- Explicit Stop with retained data: `az vm deallocate` reached
  `PowerState/deallocated`; after `az vm start` the marker file was present, the
  public IP was unchanged, the host key re-read from the restarted worker equalled
  the pinned key (it lives on the data disk), the data disk was mounted inside the
  container, and the tmux session was gone because the container restarted. Data
  survives Stop; in-container sessions do not. Sample 4's failed restart is described
  above.
- Exact deletion: the resource group was absent after every sample; the subscription
  inventory was byte-identical to the pre-sample snapshot in samples 1 to 7 and 10 to 18;
  sample 8 differed only by an unrelated concurrent addition (proof still holds) and
  sample 9 by that resource's later removal by its own lane (proof unverified).
- No provider was registered and no Container Apps Job was created. From sample 8 on,
  every VM carried a DevTestLab auto-shutdown schedule one minute after its lifetime
  deadline; from sample 11 on it was created in the same deployment as the VM; from
  sample 14 on the guest self-deallocate timer (with the power-only role) was proven
  by firing it; from sample 16 on the subscription-side reaper is the primary,
  pre-existing bound and the others are defense in depth.

Cost actually incurred: eighteen VMs for 8 to 30 minutes each plus 32 GiB disks, well
under the $10 bound; the persistent registry costs about $0.67 per day at Standard.

### Verdict against the 180-second boundary

**Not met with this configuration.** Across the seventeen samples that reached an
endpoint, the slowest verified key-only SSH session came 278.0 s after `T0` (246.0 s
excluding the out-of-band host-key read); even the fastest sample needed 203.4 s.
Fifteen of seventeen also missed the boundary for endpoint-open alone (sample 6 by
0.8 s, sample 12 by 4.0 s), and the best endpoint time was 170.9 s. The measured budget splits
into roughly 25 to 33 s VM boot, 45 to 70 s Docker installation from apt, 90 to 150 s
image pull and extraction, and 31 s for `run-command`. Levers that the measurements point to, in order of impact:

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
detach independence, Stop with retained data, exact deletion) held in samples 1,
2, 3, 5 to 8, 10 to 12 and 14 to 18; sample 9 held every gate except that its deletion proof
is unverified because of concurrent activity, and sample 13 never reached its gates,
as described above. Sample 4 held every gate up to the restart, then failed the retention
gate because the worker endpoint never returned, so its retained marker, host key
and mount were not verified; the harness change responsible was reverted before
sample 5.

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
- [x] Kernel ext4 option lines recorded from the worker; shell mirror passes; the
      authoritative Rust qualifier accepted the data disk and rejected the overlay
      control (samples 4 to 12 and 14 to 18).
- [x] Independent execution across a disconnect.
- [x] Explicit Stop with retained data, endpoint retention recorded.
- [x] Exact deletion with absence check and inventory proof (group gone, nothing left
      under it, no pre-existing resource missing). Proven in samples 1 to 8 and 10 to 18;
      sample 9 is recorded as unverified because another lane deleted its own registry
      during the run.
- [x] Slowest sample compared with the 180-second boundary: **not met** (278.0 s).
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
