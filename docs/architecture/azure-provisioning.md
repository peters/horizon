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
