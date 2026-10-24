# Azure provisioning

An Azure worker is one VM in one resource group. The group is named
`horizon-ws-<operation id>` and has the tag `horizon-operation` with that id.
The group is the scope for billing, delete and identity. One ARM deployment,
`horizon-worker`, makes the other resources in the group. Each resource has a
fixed name and the same tag.

| Resource | Name | Notes |
|---|---|---|
| Network security group | `worker-nsg` | One inbound rule: TCP 22 from the internet. |
| Virtual network and subnet | `worker-vnet`, `workers` | `10.0.0.0/16` and `10.0.0.0/24`. |
| Public address | `worker-ip` | Standard, static, IPv4. |
| Network interface | `worker-nic` | Uses the security group and the address. |
| Data disk | `worker-workspace` | Premium SSD, empty, LUN 0. The VM detaches it on delete. |
| VM | `worker` | Ubuntu 24.04. No managed identity, no password login. |
| OS disk | `worker-os` | Premium SSD, deleted with the VM. A tags resource tags it. |

`offers::AZURE_DEPLOYABLE` stays `false`, so no Horizon UI, CLI or MCP operation
creates an Azure worker in this change.

## Machine settings

The machine's cloud settings can have an `azure` section:

```json
"azure": {
  "token_file": "/absolute/path/arm-token",
  "subscription_id": "00000000-0000-4000-8000-000000000001",
  "vm_sizes": ["Standard_D4s_v3"],
  "regions": ["northeurope"],
  "registry_pull": {"server": "example.azurecr.io", "username": "reader", "password_file": "/absolute/path/pull"}
}
```

- `token_file` is a private file (`0600`) with one ARM bearer token, for example
  the token that `az account get-access-token` prints. The token stays on this
  computer. A later pull request of #1774 replaces this binding.
- `vm_sizes` and `regions` must be names in `azure::catalog`.
  `providers::cpu_flavors` and `providers::data_centers` give these lists in
  this order. Without the section, both refuse.
- `registry_pull` is optional. It has the same form as the Hetzner pull login.
- A settings file without the section keeps its encoding.

## The Compute seam

`providers::compute` gives `deployment::azure::Compute` for an Azure cloud when
the machine has an `azure` section. It provisions through
`azure::provision` and then waits for SSH readiness, as Hetzner does:

- The cloud's `azure.json` journal, beside `deployment.json`, keeps the group
  fence and the region. `providers::retained` is true when the group fence is
  `Requested` or `Bound`.
- A registry binding of the machine has priority over `registry_pull`, as on
  Hetzner: its grant must be verified for the exact digest first.
- While the worker starts, each pass reads the bound VM with
  `azure::provision::inspect_worker`, which makes the same checks with GET
  requests only.

These items do not change:

- `providers::preflight` refuses a new Azure cloud before a record is written.
  `cloud_deploy deploy`, the UI and MCP thus cannot reach the seam.
- Release refuses: an Azure profile cannot have hosted devices. Rebuild is
  `Rebuild::Unsupported`: `settle` refuses a requested image replacement.
- Only a direct caller of `deployment::azure::provision`, such as the loopback
  tests and the ignored live test `live_create`, creates a worker.

## Creation fences

`azure::provision` creates a worker with two creates. The caller keeps a fence
for each create and saves it before the request:

| Create | Fence | Saved in |
|---|---|---|
| PUT of the resource group | `Journal::group`, with the region | the caller's journal |
| PUT of the deployment | the operation (`CreateState`) | the caller's deployment record |

Each fence has these rules:

- `Prepared`: the client reads the name first. A group that it finds is bound
  only with the operation's tag. It is never written. A VM that it finds in a
  bound group is bound. A deployment that still runs is followed. If nothing is
  found, the fence goes to `Requested` before the PUT.
- `Requested`: the answer to the create was lost or did not end. The client
  sends GET requests only. It binds what it finds. If it finds nothing, it gives
  `CreationUnresolved` and the fence stays `Requested`. It never sends the
  create again.
- A deployment that ARM reports as ended, without a VM, made no VM. Only then
  does a `Requested` VM fence go back to `Prepared`.
- An answer that proves that ARM made nothing (`Failure::definite`) puts the
  fence back to `Prepared`. A capacity refusal moves to the next VM size.
- A 409, a 5xx or a transport failure keeps the fence at `Requested`.
- `Bound`: the client reads the VM only. A VM with another `vmId` is
  `WorkerLost`.

Before the worker is returned, the VM must have the operation's tag, an allowed
size, the cloud's region and one data disk: the group's workspace disk on LUN 0.
The public address comes from the deployment output `publicIp`.

## Stop, start and delete

`azure::lifecycle` changes the power of the bound VM and deletes the cloud. Each
change is proven by what ARM reports after it.

- A stop sends `POST .../virtualMachines/worker/deallocate`. It never sends
  `powerOff`. A VM that is only powered off keeps its compute allocation, and
  Azure bills it. The stop ends when the VM reports `PowerState/deallocated`.
  The disks and the static address stay.
- A start sends `POST .../virtualMachines/worker/start` to the same VM. It ends
  when the VM reports `PowerState/running`.
- Before a power change, the client reads the VM. It refuses a VM that is
  absent, another instance (another `vmId`) or without the operation's tag. The
  size and region policy does not apply, so a changed setting cannot block a stop.
- A change that is still under way is awaited. The request is not sent again.
- The worker status `EXITED` (stopped) is given only for `PowerState/deallocating`
  and `PowerState/deallocated`. A VM that is powered off but still allocated gets
  `STOPPED_ALLOCATED`, which is neither running nor stopped.

A delete removes the resource group, which holds each resource of the worker.
The journal in `azure.json` records each step before the next:

| Step | Journal | Request |
|---|---|---|
| 1 | `deleting: true`, never cleared | none |
| 2 | none | `DELETE` of the group, only while it is present and not `Deleting` |
| 3 | `group: Terminated` with the group id | `GET` of the group answers 404 |
| 4 | `swept: true` | `GET /subscriptions/<id>/resources` with the `horizon-operation` filter lists nothing |

- A group with the name but without the operation's tag is never deleted.
- A lost answer to the `DELETE` is reconciled with a `GET` of the group.
- A group whose create answer was lost (`Requested`) is trusted as absent only
  after a second 404, 30 seconds after the first.
- The sweep is read again for about 30 seconds, because ARM can list the
  resources of a deleted group for a short time. A page link (`nextLink`) is not
  a proof.
- After the proof, the VM fence goes to `Terminated` with its `vmId`, or to
  `Prepared` when no VM was bound.
- `Journal::retains` is true while the group is `Requested` or `Bound`, and
  after its 404 until the sweep lists nothing. `Journal::proves_absent` is true
  only with `deleting`, `swept` and the group id of this operation.
- `provision` refuses a journal with `deleting`, before a request.

### The lifecycle of an Azure cloud

`providers::azure::Azure` drives `deployment::azure` for check, stop, resume
and delete, with the VM fence in `deployment.json` and the group in `azure.json`:

- The client is made from the `azure` settings. The settings must name the
  subscription of the cloud's group, which the group id in `azure.json` names.
- Check reads the bound VM by ownership only and reports `Found`, `Inactive` or
  `Missing`. A VM that it finds for a requested worker is bound.
- Stop follows `RunPod`'s contract (`request_stop`): the stop is saved first,
  the request is announced, and the stage is `Stopped` only when ARM reports the
  VM deallocated. A definite refusal rolls the stop back.
- Resume starts the same VM (`request_resume`). The reconnect after it waits
  for readiness. A cloud that is being deleted is not resumed.
- Delete (`deletion::delete_azure`) runs the delete above with an uncancellable
  token once it starts. The caller then records the cloud as deleted.

## Host bootstrap

`horizon_cloud::host` renders the same `#cloud-config` for each rented host.
`Platform::Azure` adds these items to it:

- cloud-init updates the package lists and installs Ubuntu's `docker.io`.
- cloud-init formats the data disk at `/dev/disk/azure/scsi1/lun0` as ext4 only
  when it has no file system. A reboot never formats the workspace.
- The guard script drops container traffic to ports 80 and 32526 of
  `168.63.129.16` — the WireServer's own ports, not the whole address, because
  that address is also the region's DNS resolver — and each IPv6
  packet that the host forwards from `docker0`.

A `Platform::Prepared` host, for example a Hetzner host, gets the same user data as before.

## Trust boundary

The custom data is secret data that leaves Horizon. The VM and the Azure
platform can read it for the life of the VM. It contains the pull login when the
image is private. Thus these items apply:

- The deployment sends the custom data as a `securestring` parameter, so ARM
  keeps it out of the deployment history.
- The host reads the custom data from the Azure platform address (WireServer,
  `168.63.129.16`). The instance metadata service at `169.254.169.254` serves
  the VM's identity and user data. The guard drops container traffic to both
  addresses before each container start. If a rule fails, the container does not
  start. The host itself keeps access to both addresses.
- The worker has no IPv6 address. The guard drops forwarded IPv6 traffic from
  `docker0`, so a later IPv6 bridge cannot open a path that the IPv4 rules do
  not see.
- Azure accepts at most 64 KiB of custom data after base64. The host first holds
  the bootstrap to 32 KiB, its smallest provider limit, so the base64 form is at
  most 43,692 bytes. The plan also examines the base64 length. Both checks occur
  before the first request.

These items are not guaranteed:

- The custom data passes through `serde_json` once, in memory that is not wiped.
- The data disk path comes from the Azure Linux agent's udev rules. A live run
  must show `/dev/disk/azure/scsi1/lun0` on the VM.
