# GCP with `gcloud`

One machine for one card. Proved 2026-11-10 with SDK 593.0.0: one `e2-micro` in
`europe-north1-a`, one SSH login, deleted with its boot disk and rule. `(ran)` marks
a run command; `UNVERIFIED` marks one it did not run.

```sh
NAME=build-1                 # machine name: [a-z0-9-]{1,40}, no leading or trailing '-'
CARD=$NAME                   # horizon-card label value and card title
KEY=$NAME                    # the machine key name in Horizon
ZONE=europe-north1-a         # from step 2
TYPE=e2-micro                # from step 2
PROJECT=<project id>; ACCOUNT=<account address>
TAG=$NAME; FW=$NAME-ssh      # network tag and firewall rule name
G="gcloud --account=$ACCOUNT --project=$PROJECT --quiet"
PUBKEY='ssh-ed25519 AAAA... horizon-machine-build-1'   # the public_key line from machine_key
printf '%s\n' "$PUBKEY" > "$KEY.pub"                   # public key only; delete it after the create
# Never make a key under ~/.ssh and never ask the person for one.
```

Pass `--account` and `--project` on every command; never run `gcloud config set`.

## 1. Preconditions (read-only)

```sh
gcloud version                                                 # "Google Cloud SDK <v>"   (ran)
$G auth list --filter=status:ACTIVE --format="value(account)"   # exactly one address      (ran)
$G compute zones list --filter="name=$ZONE" --format="value(name,status)"   # UP
$G compute instances list --format="value(name,zone,status)"      # what it holds now
$G compute networks list --format="value(name)"                 # is there a `default`?
```

Success: one active account and zone `UP`. Stop and tell the person on `command
not found`, `You do not currently have an active account`, `PERMISSION_DENIED` or a
billing error; the person runs `gcloud auth login` themselves. Never run it for
them, and never run `auth print-access-token` or `config list`, which expose
credentials. Always pass `--project`.

## 2. Choose the zone and the type (read-only, no price)

```sh
$G compute machine-types list --zones="$ZONE" --format="json(name,guestCpus,memoryMb,accelerators)"
$G compute machine-types describe "$TYPE" --zone="$ZONE" --format="json(name,guestCpus,memoryMb,isSharedCpu)"
$G compute images describe-from-family ubuntu-2404-lts-amd64 --project=ubuntu-os-cloud --format="json(name,diskSizeGb,architecture)"
```

`e2-micro` is the cheapest shared-core type: 2 vCPU, 1 GiB, `isSharedCpu: true`.
`f1-micro` and `g1-small` are legacy, so pin the type. Read `guestCpus`, `memoryMb`
(1024 = 1 GiB), `accelerators` and `diskSizeGb` (10 is the smallest boot disk).
`machine-types list` prints `memoryMb` as GiB while `describe` prints MB.

## 3. Rent

Reuse the `default` network when step 1 showed it. GCE gives networks, subnets and
rules no labels, so the tag text goes into `--description` and the card into a name.

```sh
CALLER_IP="$(curl -4 -fsS https://api.ipify.org)/32"
$G compute firewall-rules create "$FW" --network=default --direction=INGRESS --action=ALLOW --rules=tcp:22 \
  --source-ranges="$CALLER_IP" --target-tags="$TAG" --description="horizon-managed=true horizon-card=$CARD" \
  --format="value(name,id,sourceRanges.list(),allowed[].map().firewall_rule().list(),targetTags.list())"
$G compute instances create "$NAME" --zone="$ZONE" --machine-type="$TYPE" \
  --image-family=ubuntu-2404-lts-amd64 --image-project=ubuntu-os-cloud \
  --boot-disk-size=10GB --boot-disk-type=pd-balanced --network=default --subnet=default --tags="$TAG" \
  --labels="horizon-managed=true,horizon-card=$CARD" \
  --metadata="ssh-keys=hz:$(cat "$KEY.pub"),enable-oslogin=FALSE" --format=json
```

The user name is the part before `:` in `ssh-keys`, so it is `hz` here.
`--metadata` splits on commas, so the key comment must hold no comma. Put
`enable-oslogin=FALSE` on the instance only, never in project metadata. The create
returned in 11 s already `RUNNING` with `natIP` set. Without a `default` network,
create `$NAME-net` and `$NAME-subnet` with the tag text in `--description`, then
delete both with `compute networks subnets delete` and `compute networks delete`
(UNVERIFIED).

## 4. Wait and read

```sh
$G compute instances describe "$NAME" --zone="$ZONE" --format="json(name,id,machineType,zone,status,creationTimestamp,labels,networkInterfaces[0].accessConfigs[0].natIP)"
```

Poll only while `.status` is not `RUNNING`. Card fields: `.zone` minus its last
letter and the constant `gcp` for the subtitle; `.machineType` last segment for
Type; `.networkInterfaces[0].accessConfigs[0].natIP` for Address in `mono`; `$NAME`
for Session in `mono`; `.creationTimestamp` for Rented at; `.status` for the badge
(`PROVISIONING` and `STAGING` are starting, `RUNNING` is ready, `STOPPING` and
`TERMINATED` are stopped).

## 5. Prepare the machine and connect

Wait for SSH with the machine key, from your own shell. The user is the metadata
user, `hz` here, and `sudo` works on this image:

```sh
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 hz@<natIP> 'true'
```

Retry it every 10 seconds, at most 3 minutes, and update the card between the
tries. Without `IdentitiesOnly` a running ssh agent hits `MaxAuthTries` and every
attempt answers `Too many authentication failures`; the same key authenticated at
once with it. `Permission denied (publickey)` in the first minute after `RUNNING`
means the guest agent is not ready: retry the same form.

The image ships no tmux. Run the same form with `command -v tmux`; an empty answer
means the package is missing, and then install it with the same form again:

```sh
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 hz@<natIP> \
    'sudo apt-get update && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y tmux'
```

Only now show the machine card with `Connect`:

```
operation: open_terminal
ssh:       {"host": "<natIP>", "user": "hz"}
key:       "build-1"
remote_command: "tmux new -A -s build-1"
```

## 6. Stop and start

```sh
$G compute instances stop "$NAME" --zone="$ZONE"      # UNVERIFIED
$G compute instances start "$NAME" --zone="$ZONE"     # UNVERIFIED
```

While stopped you still pay for the boot disk, snapshots and any reserved static
address; vCPU and memory stop. Re-read `.natIP` after a start.

## 7. Delete and proof (this order)

```sh
$G compute instances delete "$NAME" --zone="$ZONE" --delete-disks=boot   # (ran)
$G compute firewall-rules delete "$FW"                                   # (ran)
# --delete-disks=boot removed the boot disk too. Prove that nothing is left:
$G compute instances list --filter="labels.horizon-card=$CARD" --format="value(name,zone,status)"
$G compute disks list --filter="name~$NAME" --format="value(name,zone,status)"
$G compute firewall-rules list --filter="name~$TAG" --format="value(name)"
```

Each prints no rows. With zero matches the filters also print `WARNING: The
following filter keys were not present in any resource` on stderr: that is the empty
result, not an error. Then call `delete_machine_key`, and find again in a new
session with the same filters:

```sh
$G compute instances list --filter="labels.horizon-managed=true" --format="json(name,zone,status,labels,networkInterfaces[0].accessConfigs[0].natIP)"
$G compute firewall-rules list --filter="name~hz-" --format="value(name)"
$G compute networks list --filter="name~hz-" --format="value(name)"
$G compute disks list --filter="name~hz-" --format="value(name,zone,status)"
```

## 8. Traps

- Quota: a new project has low vCPU quota and often no GPU; `QUOTA_EXCEEDED` needs a
  person.
- `ZONE_RESOURCE_POOL_EXHAUSTED` means no stock of that type in that zone: try one
  other zone in the region, then report. Never loop.
- Instances are unique per zone, subnets per region, networks and rules per
  project; sanitize the name to RFC1035.
- Image families are per architecture (`-amd64`, `-arm64`); a mismatch fails.
- Every delete prompts; `--quiet` (inside `$G`) suppresses it.
- `SPOT` can be reclaimed at any moment, and `--no-address` makes the machine
  unreachable. Never skip the label or the tag text.
