# Hetzner with `hcloud`

One machine for one card. Proved 2026-11-10 with `hcloud` 1.69.0: one machine in
`hel1`, one SSH login, deleted, list empty. `(ran)` marks a command of that run;
`UNVERIFIED` marks one it did not run.

```sh
NAME=build-1        # machine name: [a-z0-9-]{1,40}, no leading or trailing '-'
CARD=$NAME          # horizon-card tag value and card title
KEY=$NAME           # the machine key name in Horizon
PUBKEY='ssh-ed25519 AAAA... horizon-machine-build-1'   # the public_key line from machine_key
printf '%s\n' "$PUBKEY" > "$KEY.pub"                   # public key only; delete it after the create
# Never make a key under ~/.ssh and never ask the person for one.
```

## 1. Preconditions (read-only)

```sh
hcloud version                # "hcloud <version>", exit 0 = installed   (ran)
hcloud context active         # the active context name                  (ran)
hcloud server list --quiet    # exit 0 = logged in and authorized
```

`--quiet` still prints the header row when no server exists. A non-zero exit with
a message on stderr means no usable token. Tell the person to install the CLI and
run `hcloud context create <name>`, or to re-run it and paste a token with write
access. Never read, echo or copy the token yourself. The project may hold other
resources: filter by label and touch no unlabelled leftover.

## 2. Choose the region and the type (read-only)

```sh
hcloud location list -o noheader -o columns=name,city,country,network_zone
hcloud server-type list -o noheader -o columns=name,cores,memory,disk,architecture,cpu_type,category,location,location_available
hcloud image list -t system -o noheader -o columns=name,os_flavor,os_version,architecture
```

Region: `name`, for example `hel1`. Type: the cheapest that `location_available`
names for that region, because `location` only says it is offered there. Image: the
newest Ubuntu LTS `name` with `os_flavor=ubuntu`, for example `ubuntu-24.04`. The
columns carry no price, and no column names a GPU.

## 3. Rent

```sh
MY_IP=$(curl -4 -s https://api.ipify.org)     # agrees with https://checkip.amazonaws.com
TYPE=<cheapest available>; LOCATION=<from step 2>; IMAGE=ubuntu-24.04
hcloud ssh-key create --name $NAME-key --public-key-from-file "$KEY.pub" \
  --label horizon-managed=true --label horizon-card=$CARD -o json     # .ssh_key.id
hcloud firewall create --name $NAME-fw \
  --label horizon-managed=true --label horizon-card=$CARD -o json     # .firewall.id
hcloud firewall add-rule --direction in --protocol tcp --port 22 \
  --source-ips $MY_IP/32 --description horizon-ssh $NAME-fw
hcloud server create --name $NAME --type $TYPE --image $IMAGE --location $LOCATION \
  --ssh-key $NAME-key --firewall $NAME-fw \
  --label horizon-managed=true --label horizon-card=$CARD -o json      # .server.*
```

`-o json` on a create nests the object under the resource key and sends progress
text to stderr. A new firewall has `rules: []` and allows nothing inbound: add the
SSH rule before you connect. The provider injects the public key into `root`'s
`authorized_keys`, so keep `--firewall`. Create no Network; if you must, label it
and delete it in step 7. A later attach needs
`hcloud firewall apply-to-resource --type server --server $NAME $NAME-fw`
(UNVERIFIED).

## 4. Wait and read

`server create` waits for `create_server`, `apply_firewall` and `start_server` and
returns only when the server is `running`, so polling is normally unnecessary;
poll only with `--start-after-create false` and
`hcloud server list -l horizon-card=$CARD --status running -o noheader`.

Card fields, flat in `server describe -o json` and under `.server` in
`server create -o json`: `.status` for the badge; provider constant `hetzner` and
`.location.name` for the subtitle; `.server_type.name` for Type;
`.public_net.ipv4.ip` for Address in `mono`; `$NAME` for Session in `mono`;
`.created` for Rented at.

## 5. Prepare the machine and connect

The user is `root`, the Hetzner Cloud default, so no command needs `sudo`. Wait
for SSH with the machine key, from your own shell:

```sh
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 root@<.public_net.ipv4.ip> 'true'
```

Retry it every 10 seconds, at most 3 minutes, and update the card between the
tries. `IdentitiesOnly` and `IdentityAgent=none` are required: 18 of 18 attempts
failed without them here, because the ssh agent offered its whole key list.

The image ships no tmux. Run the same form with `command -v tmux`; an empty answer
means the package is missing, and then install it with the same form again:

```sh
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 root@<.public_net.ipv4.ip> \
    'apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y tmux'
```

Only now show the machine card with `Connect`:

```
operation: open_terminal
ssh:       {"host": "<.public_net.ipv4.ip>", "user": "root"}
key:       "build-1"
remote_command: "tmux new -A -s build-1"
title:     "build-1"
```

## 6. Stop and start

```sh
hcloud server shutdown --wait $NAME   # graceful ACPI shutdown, wait 30 s   UNVERIFIED
hcloud server poweron $NAME                                                 # UNVERIFIED
```

A stopped server keeps its full price and its IPv4 stays billed; only
`server delete` stops the cost. Set the badge to `Stopped`, tone `warn`.

## 7. Delete and proof (this order)

Server first: a firewall that is still applied to a server cannot be deleted.

```sh
hcloud server delete $NAME        # waits for delete_server   (ran)
hcloud firewall delete $NAME-fw   # returns at once           (ran)
hcloud ssh-key delete $NAME-key   # returns at once           (ran)
```

Test for data rows: `-o noheader` prints one newline for an empty list.

```sh
hcloud all list -l horizon-card=$CARD -o json     # every kind empty, for example "servers": []
hcloud server list -l horizon-card=$CARD -o noheader
hcloud server describe $NAME -o json              # exit 1: "Server not found"
```

`all list` covers servers, firewalls, networks, volumes, primary IPs, floating
IPs, load balancers, placement groups, certificates, images, ISOs, storage boxes,
zones and ssh keys. A name that still shows means the delete did not finish: wait
about 10 s and read again. Then call `delete_machine_key`.

Find again:

```sh
hcloud all list -l horizon-managed=true -o json
hcloud all list -l horizon-managed=true --paid -o json    # only what costs money
```

Both are accepted by 1.69.0. Each item carries its `horizon-card` label. One card
per machine found.

## 8. Traps

- `Too many authentication failures` is a key-offering problem, not a wrong key.
- `--datacenter` is removed and a server has no `.datacenter` key; use
  `--location`. The `hcloud datacenter` commands return HTTP 410.
- A retry with the same `$NAME` fails while the old resource exists.
- Deleting an attached firewall fails. Attach it at create with `--firewall`.
- A reused IPv4 can carry a new host key; Horizon keeps one known-hosts file per key.
- Quota and capacity errors exit non-zero and name the limit. Report them and do
  not retry in a loop. Check `location_available` before the create.
- Never create a resource without both labels, or
  `all list -l horizon-managed=true` misses it.
