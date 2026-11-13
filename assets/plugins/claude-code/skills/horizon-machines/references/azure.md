# Azure with `az`

One machine for one card, in one resource group. Proved 2026-11-10 with `az`
2.90.0: one `Standard_B1ls` in `norwayeast`, one SSH login, deleted. `(ran)` marks a
run command; `UNVERIFIED` marks one it did not run.

```sh
NAME=build-1        # machine name: [a-z0-9-]{1,40}, no leading or trailing '-'
CARD=$NAME          # horizon-card tag value and card title
KEY=$NAME           # the machine key name in Horizon
RG=$NAME            # one resource group per card: the cleanup boundary
LOC=norwayeast      # from step 2
SIZE=Standard_B1ls  # from step 2
SUB=<subscription id>
PUBKEY='ssh-ed25519 AAAA... horizon-machine-build-1'   # the public_key line from machine_key
printf '%s\n' "$PUBKEY" > "$KEY.pub"                   # public key only; delete it after the create
# Never make a key under ~/.ssh and never ask the person for one.
```

Every command carries `--subscription $SUB`, and every read carries `-o json`.
Never run `az account set` and never read, echo or copy a credential.

## 1. Preconditions (read-only)

Run `az account show -o json --query "{name:name,id:id,state:state}"`.
Ready: exit 0, `"state": "Enabled"` and a non-empty `id`, which becomes `SUB`;
`az account list` shows the others. Tell the person that `az` is not installed, or
that `az login` is needed.

## 2. Choose the region and the type (read-only)

```sh
az account list-locations -o json --query "[?metadata.regionType=='Physical'].{name:name,displayName:displayName}"
az vm list-skus -l $LOC --resource-type virtualMachines --subscription $SUB -o json \
  --query "[?starts_with(name,'Standard_B')].{name:name,vCPUs:capabilities[?name=='vCPUs']|[0].value,memGB:capabilities[?name=='MemoryGB']|[0].value,restriction:restrictions[0].reasonCode}"
az vm image show -l $LOC --urn Canonical:ubuntu-24_04-lts:server:latest --subscription $SUB -o json \
  --query "{arch:architecture,os:osDiskImage.operatingSystem,version:version}"
```

Show `displayName` for the region and pass `name`. Take the first size whose
`restriction` is null, sorted by vCPUs then `memGB`. **There is no `Ubuntu2404`
alias**: use the URN above, where `server` is Gen2. Show no price. Before a large
`SIZE`, read the quota row with `az vm list-usage -l $LOC --subscription $SUB -o
json --query "[?contains(localName,'CPUs')]"`. `az account list-locations` rejects
`--subscription`.

## 3. Rent

```sh
az group create -n $RG -l $LOC --subscription $SUB --tags horizon-managed=true horizon-card=$CARD -o json
az network vnet create -g $RG -n $RG-vnet -l $LOC --address-prefixes 10.0.0.0/16 \
  --subnet-name $RG-subnet --subnet-prefixes 10.0.0.0/24 \
  --subscription $SUB --tags horizon-managed=true horizon-card=$CARD -o json
az network nsg create -g $RG -n $RG-nsg -l $LOC --subscription $SUB --tags horizon-managed=true horizon-card=$CARD -o json
az network nsg rule create -g $RG --nsg-name $RG-nsg -n $RG-ssh --priority 100 \
  --protocol Tcp --direction Inbound --access Allow --source-address-prefixes '*' \
  --destination-port-ranges 22 --subscription $SUB -o json
az network public-ip create -g $RG -n $RG-pip -l $LOC --sku Standard --allocation-method Static \
  --subscription $SUB --tags horizon-managed=true horizon-card=$CARD -o json
az network nic create -g $RG -n $RG-nic -l $LOC --vnet-name $RG-vnet --subnet $RG-subnet \
  --public-ip-address $RG-pip --network-security-group $RG-nsg \
  --subscription $SUB --tags horizon-managed=true horizon-card=$CARD -o json
az vm create -g $RG -n $RG-vm -l $LOC --image Canonical:ubuntu-24_04-lts:server:latest --size $SIZE \
  --admin-username azureuser --ssh-key-values @$KEY.pub --nics $RG-nic --nsg-rule NONE \
  --os-disk-delete-option Delete --subscription $SUB --tags horizon-managed=true horizon-card=$CARD -o json
```

With `--nics`, pass no `--vnet-name`, `--subnet`, `--nsg` or `--public-ip-address`.
A rule accepts no `--tags`; it goes with its tagged group. The top level holds
nulls, so read ids from `newVNet.id`, `NewNSG.id`, `publicIp.id` and `NewNIC.id`.

## 4. Wait and read

`az vm create` blocks until the machine runs and returns `powerState` and
`publicIpAddress`, but `name`, `vmSize` and `provisioningState` are null with
`--nics`. Read the card fields with one call:

```sh
az vm show -g $RG -n $RG-vm -d --subscription $SUB -o json \
  --query "{name:name,location:location,vmSize:hardwareProfile.vmSize,powerState:powerState,publicIps:publicIps,privateIps:privateIps,admin:osProfile.adminUsername,diskSku:storageProfile.osDisk.managedDisk.storageAccountType}"
az resource list -g $RG --resource-type Microsoft.Compute/virtualMachines --subscription $SUB -o json \
  --query "[].{name:name,createdTime:createdTime}"
```

Card fields: provider constant `azure` and `location` for the subtitle;
`hardwareProfile.vmSize` for Type; `publicIps` for Address in `mono`; `$NAME` for
Session in `mono`; `createdTime` for Rented at; `powerState` for the badge.

## 5. Prepare the machine and connect

Wait for SSH with the machine key, from your own shell. The user is `azureuser`,
and `sudo` works without a password on this image:

```sh
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 azureuser@<publicIps> 'true'
```

Retry it every 10 seconds, at most 3 minutes, and update the card between the
tries. Without `IdentitiesOnly` and `IdentityAgent=none`, a local ssh agent with
six or more keys exhausts `MaxAuthTries` and the client shows only `Disconnected
from <address> port 22`, exit 255 (3 minutes and 18 attempts here).

The image ships no tmux. Run the same form with `command -v tmux`; an empty answer
means the package is missing, and then install it with the same form again:

```sh
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 azureuser@<publicIps> \
    'sudo apt-get update && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y tmux'
```

Only now show the machine card with `Connect`:

```
operation: open_terminal
ssh:       {"host": "<publicIps>", "user": "azureuser"}
key:       "build-1"
remote_command: "tmux new -A -s build-1"
```

## 6. Stop and start

```sh
az vm deallocate -g $RG -n $RG-vm --subscription $SUB    # state: VM deallocated   UNVERIFIED
az vm start -g $RG -n $RG-vm --subscription $SUB                                 # UNVERIFIED
```

Use `deallocate`, not `stop`: after `az vm stop` the compute keeps costing. A
deallocated machine still costs for its OS disk, its static IP and its group.

## 7. Delete and proof (two calls)

The group is the boundary: delete no network object one by one.

```sh
az vm delete -g $RG -n $RG-vm --yes --subscription $SUB   # 11 s; the disk goes with it   (ran)
az group delete -n $RG --yes --subscription $SUB          # 138 s                            (ran)
az group exists -n $RG --subscription $SUB                            # false
az resource list --tag horizon-card=$CARD --subscription $SUB -o json # []
az graph query -q "Resources | where name startswith '$CARD' | project name,type" \
  --subscriptions $SUB -o json --query "count"                        # 0
az disk list -g $RG --subscription $SUB                               # []
```

A non-empty list means repeat the delete. Then call `delete_machine_key` and find
again with `az vm show -d` for the state:

```sh
az resource list --tag horizon-managed=true --subscription $SUB -o json \
  --query "[].{card:tags.\"horizon-card\",name:name,type:type,region:location,rg:resourceGroup,createdTime:createdTime}"
az vm show -g $RG -n $RG-vm -d --subscription $SUB -o json --query "{powerState:publicIps}"
```

## 8. Traps

- A quota error needs the person to raise the quota. Do not change the region
  silently. `SkuNotAvailable`, `AllocationFailed` and `ZonalAllocationFailed` name
  the size and the region: tell the person which pair you used.
- An unknown image alias fails the whole create with `Invalid image`, and the group
  name is unique per subscription.
- `az vm create` picked `Premium_LRS` for the 0.5 GB machine; `--storage-sku
  Standard_LRS` should cost less but is UNVERIFIED.
- A new group, security group, public IP or interface can answer
  `ResourceNotFound` for seconds. Retry the same command; create no second one.
- Every delete is final, and `az group delete` also deletes what a person put in
  the group. The group belongs to the card, so this is intended.
- Never create a resource without both tags, or find-again misses it.
