# RunPod with `runpodctl`

One machine for one card. Proved 2026-11-10 with runpodctl 2.14.0-dd55bcf: one CPU
pod, one SSH login with an own key, deleted with its template, account unchanged.
`(ran)` marks a run command; `UNVERIFIED` marks one it did not run.

RunPod rents a container, not a virtual machine:

- No network object: no VPC, firewall, security group or key pair. A published port
  is reachable from the internet and there is no source-IP allowlist, so publish
  `22/tcp` only.
- No persistent root disk. `--container-disk-in-gb` is scratch space.
- `stop` is not `off`: the pod, its disk and its bill stay (step 6).
- No labels. `pod create` has no `--label`: put the tag in the name and in `--env`,
  and filter on the name.

```sh
NAME=build-1                      # machine name: [a-z0-9-]{1,40}
CARD=$NAME                        # horizon-card tag value and card title
KEY=$NAME                         # the machine key name in Horizon
PODNAME=horizon-managed-$NAME     # tag 1: the name
TPLNAME=$PODNAME-tpl
PUBKEY='ssh-ed25519 AAAA... horizon-machine-build-1'   # the public_key line from machine_key
# Never make a key under ~/.ssh and never ask the person for one.
```

## 1. Preconditions (read-only)

```sh
runpodctl version               # "runpodctl <version>"
runpodctl user -o json          # read-only: clientBalance, currentSpendPerHr, spendLimit
runpodctl pod list --all -o json | jq -c '[..|objects|select(has("desiredStatus"))|{id,name,runtimeStatus}]'
```

Record the pods that exist first, so you can tell yours from the account's. If
`user` fails on auth, say that no API key is configured and that the person sets
`RUNPOD_API_KEY` or runs `runpodctl doctor`; that command prompts for a key and
saves it. Never print the key.

## 2. Choose the datacenter and the type

```sh
runpodctl datacenter list -o json                 # {id, location, name, gpuAvailability[]}
runpodctl gpu list -o json                        # gpu types with stock and price
runpodctl gpu list --include-unavailable -o json  # UNVERIFIED
```

`datacenter list` gives `id`, `location` and `name` only: no vCPU, no memory and no
price. There is no CPU-flavor catalog and no `--vcpu` flag, so `--compute-type cpu`
with no `--gpu-id` rents the smallest CPU pod that the datacenter has. Show no
price. An earlier pod was 2 vCPU, 4 GB, `gpuCount` 0.

RunPod answers no cores and no memory of a CPU pod, so a size row of Set up cannot
say what the place offers: draw the row with `disabled: true` and the subtitle
`RunPod publishes no vCPU and memory for a CPU pod.`, and rent only when the person
chooses a GPU type by its own name and price.

## 3. Rent

Do not add a key to the account: account keys are shared with every pod and appear
in `runpodctl ssh list-keys -o json`. Put the machine key into the pod itself,
through a template whose start command writes `authorized_keys`:

```sh
SCRIPT='mkdir -p /root/.ssh; chmod 700 /root/.ssh; printf "%s\n" "$HZ_PUBKEY" > /root/.ssh/authorized_keys; chmod 600 /root/.ssh/authorized_keys; export DEBIAN_FRONTEND=noninteractive; apt-get update && apt-get install -y openssh-server tmux; mkdir -p /run/sshd; exec /usr/sbin/sshd -D -e'
ENVJ=$(jq -nc --arg p "$PUBKEY" --arg c "$CARD" '{HZ_PUBKEY:$p,HORIZON_MANAGED:"true",HORIZON_CARD:$c}')
TPL=$(runpodctl template create --name "$TPLNAME" --image ubuntu:24.04 --ports 22/tcp \
  --container-disk-in-gb 10 --env "$ENVJ" --docker-start-cmd "bash,-c,$SCRIPT" -o json | jq -r '.id')
POD=$(runpodctl pod create --name "$PODNAME" --template-id "$TPL" --compute-type cpu \
  --cloud-type SECURE --ports 22/tcp --container-disk-in-gb 10 --env "$ENVJ" -o json | jq -r '.id')
```

`--docker-start-cmd` splits on commas, so the script must hold no comma. Use a
private env name (`HZ_PUBKEY`): the platform writes its own `PUBLIC_KEY`, so writing
`authorized_keys` from that one is not reliable. `--ssh` is the default.
`--global-networking` fails on a CPU pod with `global networking requires compute
type GPU`; the pod needs no such flag. The create returned in about 1 s with the
id, `runtimeStatus: initializing`, `costPerHr`, `vcpuCount` and `machine`.

## 4. Wait and read

```sh
runpodctl pod get "$POD" --include-machine -o json
runpodctl ssh info "$POD" -o json          # {"ip":..., "port":..., "ssh_command":...}
runpodctl pod logs "$POD" --source system --tail 100   # why a start stalls
```

`runtimeStatus` reads `initializing` until the container reports, then `running`
(about 44 s here). While it initializes, `pod get` holds no address and its `ssh`
key is an error. Card fields: provider constant `runpod` and `machine.dataCenterId`
for the subtitle; `vcpuCount`, `memoryInGb` and `gpuCount` for Type (no
`computeType` key); `.ip` and `.port` of `ssh info` for Address in `mono`;
`$NAME` for Session in `mono`; `createdAt` for Rented at (`lastStatusChange` is a
sentence, not a time); `runtimeStatus` for the badge.

## 5. Prepare the machine and connect

The user is `root`, so no command needs `sudo`. Port 22 is remapped to a public
port (`port` above), so every form carries `-p`; `pod get` gives `publicIp: null`
for a CPU pod. Wait for SSH with the machine key, from your own shell:

```sh
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 -p <ssh info .port> root@<ssh info .ip> 'true'
```

Retry it every 10 seconds, at most 3 minutes, and update the card between the
tries. A pod reports `running` before its sshd answers, so the wait is the proof.
Without `IdentitiesOnly` and `IdentityAgent=none` the person's ssh agent offers
its whole key list and the login fails on `MaxAuthTries`.

The start command of step 3 installs tmux. Prove it with the same form and
`command -v tmux`; an empty answer means the install did not run, and then
install it with the same form:

```sh
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 -p <ssh info .port> root@<ssh info .ip> \
    'apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y tmux'
```

Only now show the machine card with `Connect`:

```
operation: open_terminal
ssh:       {"host": "<ssh info .ip>", "user": "root", "port": <ssh info .port>}
key:       "build-1"
remote_command: "tmux new -A -s build-1"
```

## 6. Stop and start

```sh
runpodctl pod stop "$POD"      # UNVERIFIED
runpodctl pod start "$POD"     # UNVERIFIED
```

A stopped pod releases compute and keeps billing its volume and its container disk.
Its address can change on a start, a start can fail when the datacenter has no free
capacity, and the container disk does not survive. Only `pod delete` ends the cost.
Read `runpodctl ssh info` again after a start, and repeat step 5 before the card
shows `Connect` again.

## 7. Delete and proof

Delete the pod first, then the template it came from. Both answer
`{"deleted":true,"id":"<id>"}`.

```sh
runpodctl pod delete "$POD"          # (ran)
runpodctl template delete "$TPL"     # (ran)
runpodctl pod list --all -o json | jq -e --arg c "$CARD" '([..|objects|select(has("desiredStatus"))|select((.name // "")|contains($c))]|length) == 0'
runpodctl template list --type user -o json | jq -e --arg c "$CARD" '([..|objects|select((.name? // "")|contains($c))]|length) == 0'
runpodctl pod list --all -o json | jq -c '..|objects|select(has("desiredStatus"))|select((.name // "")|startswith("horizon-managed-"))|{id,name,runtimeStatus}'
```

`jq -e` prints `true` and exits 0 only at a zero count. `--all` is required, because
`pod list` hides stopped pods. Deletes are eventually consistent: on a count of 1,
wait about 3 s and read again. Do not delete account keys. Then call
`delete_machine_key`. The last line is the find-again command for a new session; for
templates use `template list --type user` the same way.

## 8. Traps

- A `--wait` timeout keeps the pod and its bill, and the error carries its id.
  Recover that id; never run `pod create` twice, or the person pays for two.
- The default compute type is GPU. A CPU rent needs `--compute-type cpu` and no
  `--gpu-id`.
- Pod names are not unique. Keep the card in the name, or you delete a stranger's pod.
- New accounts have pod and GPU quotas. The error names the limit: report it.
- `runtimeStatus: running` means the container reported, not that SSH answers. Poll
  `ssh info`; create no second pod.
- `--public-ip` is community-cloud only and a CPU pod needs none.
- Never skip the name tag or the two env tags, or find-again misses the pod.
