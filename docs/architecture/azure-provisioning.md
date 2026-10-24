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

## Host bootstrap

`horizon_cloud::host` renders the same `#cloud-config` for each rented host.
`Platform::Azure` adds these items to it:

- cloud-init updates the package lists and installs Ubuntu's `docker.io`.
- cloud-init formats the data disk at `/dev/disk/azure/scsi1/lun0` as ext4 only
  when it has no file system. A reboot never formats the workspace.
- The guard script drops container traffic to `168.63.129.16` and each IPv6
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
