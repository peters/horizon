# Azure workspace readiness: preflight contract and spike checklist

Permanent operator guidance for the Azure CPU lane of
[#474](https://github.com/peters/horizon/issues/474) under
[#383](https://github.com/peters/horizon/issues/383). It records the dated
official-source comparison behind `scripts/azure-workspace-preflight/preflight.py`,
the qualification gates that no API read can close, and the **future**
three-sample spike checklist. Nothing in this document is cloud evidence. No
Azure resource has been created, started, stopped or deleted for it, and no
candidate has been selected. Container Apps Jobs are excluded from this lane by
the issue contract and are not compared.

Status legend used below: **documented** means stated by the linked official
page on the date given; **not verified in this pass** means the author did not
locate an official statement and the spike must measure or confirm it;
**not executed** marks spike steps that have never been run.

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

## Future three-sample spike checklist (not executed)

Every item below is **not executed** as of 2026-09-10. Paid creation is approved
by #474 only within a bounded cost and lifetime, after the read-only preflight
above, with explicit approval before any provider registration.

Before the first sample:

- [ ] Candidate revalidation note referencing this comparison and the live
      preflight reports, signed off by the lead.
- [ ] Exact candidate image: registry, repository, tag and immutable digest of
      the compact worker image; confirm it is x64 and includes `sshd`.
- [ ] Bounded cost and lifetime: maximum hourly cost, maximum wall-clock lifetime
      per sample, and the hard deadline for cleanup, written down before creation.
- [ ] Resource ownership journal: one task-owned resource group per sample with a
      unique name, tags recording issue, sample number and creation time, plus a
      pre-creation inventory snapshot of the subscription so "unchanged
      pre-existing resources" can be proven afterward.
- [ ] Fresh client key pair generated for the spike only; the public key is the
      only identity installed; no password authentication anywhere.
- [ ] User-assigned managed identity with pull-only registry rights created under
      the same ownership journal; no registry password or admin user enabled.

Per sample (three samples, sequential, one exact resource each):

- [ ] Record `T0` before the create call; record create acknowledged, image pull
      complete, endpoint published, first successful key-only SSH handshake, and
      delete complete, each as UTC timestamps and elapsed seconds from `T0`.
- [ ] Verify the SSH host key out of band and pin it; reject any prompt-based
      trust.
- [ ] Run the on-worker storage qualification against the intended repository
      root and record the raw kernel option lines; a failure here is a candidate
      blocker regardless of timing.
- [ ] Prove independent execution: start a long-running process, disconnect the
      client, wait, reconnect and observe it still running.
- [ ] Prove explicit Stop with retained data: write a marker file, perform the
      candidate's documented stop, start again, confirm the marker survives and
      record whether the endpoint changed.
- [ ] Prove exact deletion: delete only the sample's resources, then confirm
      absence by identity and confirm the pre-creation inventory is unchanged.

After the three samples:

- [ ] Compare all three timing records against the 180-second boundary; report
      the slowest sample, not the average.
- [ ] Record cost actually incurred against the bound.
- [ ] Attach cleanup proof (absence checks and unchanged inventory) to the journal.
- [ ] Only then update #474 with the revalidated compute and storage decision.
      Until that update exists, no Azure adapter, identity integration or UI work
      should assume a platform.

## Maintenance

Re-check the linked pages and API versions when the tool's pinned
`api-version` values change or when Microsoft retires one. Keep the tool's
allowlist and this document in step: a new check needs a documented read-only
operation and an entry in the table above.
