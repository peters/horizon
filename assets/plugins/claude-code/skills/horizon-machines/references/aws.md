# AWS with `aws`

One machine for one card, in the default VPC. This recipe is NOT proved end to end:
the test login on 2026-11-10 was read-only, so `RunInstances` was denied. `(ran)`
marks a run command; `UNVERIFIED` marks every create and delete. Read step 1 first.

```sh
NAME=build-1                      # machine name: [a-z0-9-]{1,40}
CARD=$NAME                        # horizon-card tag value and card title
KEY=$NAME                         # the machine key name in Horizon
REGION=<region>
TYPE=t3.micro                     # from step 2; probe t3.nano first, it is smaller
PROFILE=<aws profile>             # per command; never edit ~/.aws
HZ="[{Key=horizon-managed,Value=true},{Key=horizon-card,Value=$CARD}]"
AWS="aws --region $REGION --profile $PROFILE"
PUBKEY='ssh-ed25519 AAAA... horizon-machine-build-1'   # the public_key line from machine_key
printf '%s\n' "$PUBKEY" > "$KEY.pub"                   # public key only; delete it after the create
# Never make a key under ~/.ssh and never ask the person for one.
```

## 1. Preconditions and permission preflight (read-only)

```sh
$AWS sts get-caller-identity --output json --query '{account:Account,arn:Arn}'          # (ran)
$AWS ec2 describe-instance-types --filters Name=instance-type,Values=t3.nano,t3.micro --output json \
  --query 'InstanceTypes[].{type:InstanceType,vcpu:VCpuInfo.DefaultVCpus,memMiB:MemoryInfo.SizeInMiB}'
AMI=$($AWS ec2 describe-images --owners 099720109477 \
  --filters 'Name=name,Values=ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-*' \
            'Name=state,Values=available' --output text \
  --query 'Images|sort_by(@,&CreationDate)[-1].ImageId')
```

A 12-digit `account` and an `arn` prove only that the CLI is installed and signed
in, not that it may rent. Probe the actions it needs with `--dry-run`: that creates
nothing and answers `DryRunOperation` when allowed, `UnauthorizedOperation` when not.

```sh
$AWS ec2 import-key-pair --key-name $NAME-key --public-key-material fileb://$KEY.pub --dry-run
$AWS ec2 create-security-group --group-name $NAME-sg --description probe --dry-run
$AWS ec2 run-instances --image-id "$AMI" --instance-type "$TYPE" --count 1 --dry-run
```

`UnauthorizedOperation` is a hard stop before you rent: never create a machine that
you cannot reach over SSH or firewall. Record the action from the error and stop. Do
not use EC2 Instance Connect, SSM Session Manager, user-data `ssh_authorized_keys`
or another region to reach a machine whose key or group action is denied. On
`Unable to locate credentials`, say that the CLI is not signed in and that the person
runs `aws configure sso`. Never ask for an access key.

## 2. Choose the region and the type (read-only)

List the regions with `$AWS ec2 describe-regions --all-regions --output text --query
'Regions[].RegionName'`. Take the first type that step 1 confirms: `t3.nano` (2 vCPU,
512 MiB) before `t3.micro` (2 vCPU, 1024 MiB); add `,GpuInfo` for a GPU card. Show no
price. The default VPC needs no create call, and its subnets set `MapPublicIpOnLaunch`.

## 3. Rent (UNVERIFIED)

Lead with `describe-images`: the SSM parameter path is often denied. Port 22 opens
to the caller address only, and step 7 closes it.

```sh
CALLER_IP=$(curl -4 -s https://api.ipify.org)   # or https://checkip.amazonaws.com
VPC=$($AWS ec2 describe-vpcs --filters Name=isDefault,Values=true --output text --query 'Vpcs[0].VpcId')
SUBNET=$($AWS ec2 describe-subnets --filters Name=vpc-id,Values=$VPC Name=default-for-az,Values=true \
  --output text --query 'Subnets[0].SubnetId')
$AWS ec2 import-key-pair --key-name $NAME-key --public-key-material fileb://$KEY.pub \
  --tag-specifications "ResourceType=key-pair,Tags=$HZ" --output text --query 'KeyName'      # UNVERIFIED
SG=$($AWS ec2 create-security-group --group-name $NAME-sg --description "card $CARD" \
  --vpc-id "$VPC" --tag-specifications "ResourceType=security-group,Tags=$HZ" \
  --output text --query 'GroupId')                                                            # UNVERIFIED
$AWS ec2 authorize-security-group-ingress --group-id "$SG" --protocol tcp --port 22 \
  --cidr "$CALLER_IP/32"                                                                      # UNVERIFIED
IID=$($AWS ec2 run-instances --image-id "$AMI" --instance-type "$TYPE" --count 1 \
  --key-name $NAME-key --security-group-ids "$SG" --subnet-id "$SUBNET" --associate-public-ip-address \
  --tag-specifications "ResourceType=instance,Tags=$HZ" "ResourceType=volume,Tags=$HZ" \
  --output text --query 'Instances[0].InstanceId')                                             # UNVERIFIED
```

## 4. Wait and read

```sh
$AWS ec2 wait instance-running --instance-ids "$IID"                                          # UNVERIFIED
$AWS ec2 describe-instances --instance-ids "$IID" --output json \
  --query 'Reservations[0].Instances[0].{state:State.Name,type:InstanceType,az:Placement.AvailabilityZone,ip:PublicIpAddress,launchTime:LaunchTime,rootDeleted:BlockDeviceMappings[0].Ebs.DeleteOnTermination}'
```

Card fields: provider constant `aws` and `az` for the subtitle; `type` for Type;
`ip` for Address in `mono`; `$NAME` for Session in `mono`; `launchTime` for Rented
at; `state` for the badge. `ip` is null for seconds after `running`: repeat until it
is set, and read `rootDeleted`.

## 5. Prepare the machine and connect

Wait for SSH with the machine key, from your own shell. The user is `ubuntu` for
Canonical AMIs, port 22, and `sudo` works without a password:

```sh
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 ubuntu@<ip> 'true'
```

Retry it every 10 seconds, at most 3 minutes, and update the card between the
tries. Without `IdentitiesOnly` and `IdentityAgent=none`, an agent with six or more
keys exhausts `MaxAuthTries`.

The Canonical image ships no tmux. Run the same form with `command -v tmux`; an
empty answer means the package is missing, and then install it with the same form:

```sh
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 ubuntu@<ip> \
    'sudo apt-get update && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y tmux'
```

Only now show the machine card with `Connect`:

```
operation: open_terminal
ssh:       {"host": "<ip>", "user": "ubuntu"}
key:       "build-1"
remote_command: "tmux new -A -s build-1"
```

## 6. Stop and start

```sh
$AWS ec2 stop-instances  --instance-ids "$IID"                                                # UNVERIFIED
$AWS ec2 start-instances --instance-ids "$IID"                                                # UNVERIFIED
$AWS ec2 wait instance-running --instance-ids "$IID"
```

The id, the key pair, the group and the disk stay. Compute hours stop and the root
EBS volume does not. The address is released on stop.

## 7. Delete and proof (this order, all UNVERIFIED)

With the default VPC there is no VPC, subnet, gateway or route table to delete.

```sh
$AWS ec2 terminate-instances --instance-ids "$IID"
$AWS ec2 wait instance-terminated --instance-ids "$IID"
$AWS ec2 delete-security-group --group-id "$SG"      # DependencyViolation until the ENI is gone
$AWS ec2 delete-key-pair --key-name $NAME-key
$AWS ec2 describe-instances --filters "Name=tag:horizon-card,Values=$CARD" \
  --output json --query 'Reservations[].Instances[].{id:InstanceId,state:State.Name}'
$AWS ec2 describe-tags --filters "Name=key,Values=horizon-card" "Name=value,Values=$CARD" \
  --output json --query 'Tags[]'
$AWS ec2 describe-security-groups --filters "Name=group-name,Values=$NAME-sg*" \
  --output json --query 'SecurityGroups[].GroupId'
$AWS ec2 describe-key-pairs --filters "Name=key-name,Values=$NAME-key" --output text --query 'KeyPairs[].KeyName'
$AWS ec2 describe-instances --filters "Name=tag:horizon-managed,Values=true" --output json --query 'Reservations[].Instances[].{id:InstanceId,card:Tags[?Key==`horizon-card`].Value|[0],state:State.Name,ip:PublicIpAddress}'
```

`[]` is the pass. A `terminated` instance stays visible for minutes, so read `state`,
not the row; delete any resource that still prints. Then call `delete_machine_key`.
The last command finds every rented machine in a new session. Tags are per region, and
a volume or snapshot that a person added needs its own delete.

## 8. Traps

- A read-only profile passes `GetCallerIdentity` and denies every create: probe first.
- `VcpuLimitExceeded` is per region and family; `InsufficientInstanceCapacity` means
  no free hardware now. Report both; do not retry in a loop.
- A new group or key pair is not always visible at once: wait and repeat the call.
  `--key-name` and `--group-name` are unique per region and VPC.
- A failed `wait` exits non-zero and 255 is a catch-all: read `State.Name` first.
- Keep the machine no longer than the agreed cap, and start step 7 after step 5.
- Never create a resource without both tags, or find-again misses it.
