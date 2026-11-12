# A Tailscale network with the API

One rented machine joins one network of the person. The person stores the OAuth
client of the network as three named secrets. The agent uses the client secret on
the computer of the person only, and the machine receives one auth key for one
login.

`(doc)` marks a command, a flag, a body or a name that the current public
Tailscale documentation or the source of `tailscale up` states, read on
2026-11-12. `UNVERIFIED` marks one that those documents do not state, and the
reason follows it. No command of this reference ran in this slice, so the
reviewer proves every one live.

```sh
NET=HOME                  # [A-Z0-9]{1,16}: the <NET> of the three named secrets
NAME=build-1              # machine name: [a-z0-9-]{1,40}, no leading or trailing '-'
TAG=tag:horizon           # the value of TS_${NET}_TAG
KEY=$NAME                 # the machine key name in Horizon
```

## 1. What the person stores one time

Three named secrets of Horizon, in **Settings → Secrets**, each with `Add`:

| Name | Value |
|---|---|
| `TS_<NET>_CLIENT_ID` | The client ID of the OAuth client of that network. |
| `TS_<NET>_CLIENT_SECRET` | The client secret of the same client. The field is masked. |
| `TS_<NET>_TAG` | The tag of the network that the machine takes, for example `tag:horizon`. |

If you used the older Horizon, Settings → Remote → Import from the keyring can
create these three secrets.

The three names pass the name rule of a named secret: `A-Z`, `0-9` and `_`, the
first character is a letter, at most 64 characters, and the name starts with no
prefix that Horizon owns. `TS_` is free, so every name of this recipe passes the
rule. A name that starts with `OPENAI`, for example, is refused.

One OAuth client of the network, in the admin console at **Trust credentials** →
**OAuth** (doc):

- The scopes `auth_keys` (write) and `devices:core` (read and write).
- The tag `<TAG>` as the tag of the client. The tag must exist in
  `tagOwners` of the access controls, and the client is limited to it.
- A key that the client makes must carry a tag that the client owns. The API
  refuses any other tag with 400 or 403. Another tag works only when `tagOwners`
  gives `<TAG>` to the tag of the client (doc).

The access controls of the network must also let the devices of the person reach
TCP 22 of `<TAG>`. A tagged device is not a member of the network, so a rule
whose `dst` is `autogroup:member` does not include it. Name `tag:horizon` or `*`
in `dst` (doc).

## 2. The two rules of a secret here

- **The client secret stays on the computer of the person.** The documentation
  allows `tailscale up --auth-key=<client secret>`, and this recipe does not use
  it: that form copies the secret to the machine, to its shell history and to its
  process table. The agent uses the secret for one API call here, and the machine
  receives a key that the call made.
- **The auth key arrives on standard input.** It travels in the standard input of
  `ssh` to a file under `/run` that only root reads, and the same command removes
  the file. No argument and no environment variable of the machine holds it.

## 3. The safe API calls on the computer of the person

`-d "client_secret=$TS_HOME_CLIENT_SECRET"` puts the secret in the arguments of
`curl`, where another process of the same user reads it, on Linux from
`/proc/<pid>/cmdline`. The value goes on standard input instead. `printf` writes that pipe, and it is a
builtin of `bash`, `zsh`, `dash` and of `busybox sh`, so the value is in no
argument list of its own: `command -V printf` answers `printf is a shell builtin`.

One token, which lives one hour (doc):

```sh
TS_TOKEN=$(printf '%s' "$TS_HOME_CLIENT_SECRET" | curl --silent --show-error --fail \
  --data-urlencode "client_id=$TS_HOME_CLIENT_ID" \
  --data-urlencode client_secret@- \
  https://api.tailscale.com/api/v2/oauth/token | jq -r '.access_token')
```

The endpoint takes the client credentials grant and answers `access_token` and
`expires_in` 3600 (doc). `--data-urlencode name@-` reads the value from standard
input, and `curl` encodes it (doc).

One auth key for this machine (doc):

```sh
TS_KEY=$(printf 'Authorization: Bearer %s\n' "$TS_TOKEN" | curl --silent --show-error --fail \
  --header @- --request POST \
  --data '{"capabilities":{"devices":{"create":{"reusable":false,"ephemeral":false,"preauthorized":true,"tags":["tag:horizon"]}}},"expirySeconds":600,"description":"horizon build-1"}' \
  https://api.tailscale.com/api/v2/tailnet/-/keys | jq -r '.key')
```

`-H @-` reads the header lines from standard input (doc). The body holds the tag,
the expiry and the machine name, and no secret. `-` is the tailnet of the token
(doc). The answer holds the key in `.key` only at creation (doc), so no other
program may print that answer. `jq` is required for these three calls: prove `command -v jq` before the first.

The one body `UNVERIFIED`: the fields `capabilities.devices.create.reusable`,
`ephemeral`, `preauthorized` and `tags` are the documented ones, and the shortest
accepted `expirySeconds` is not stated. 600 is ten minutes, and the key is spent
by one login.

The device of one hostname, and its removal (doc):

```sh
TS_ID=$(printf 'Authorization: Bearer %s\n' "$TS_TOKEN" | curl --silent --show-error --fail --header @- \
  "https://api.tailscale.com/api/v2/tailnet/-/devices?hostname=$NAME&tags=$TAG" | jq -r '.devices[0].nodeId // empty')
printf 'Authorization: Bearer %s\n' "$TS_TOKEN" | curl --silent --show-error --fail --header @- \
  --request DELETE "https://api.tailscale.com/api/v2/device/$TS_ID"
```

`GET /tailnet/-/devices` takes exact filters such as `hostname` and `tags`, and a
row holds `nodeId`, `hostname` and `tags`. `DELETE /device/{deviceId}` answers
200 with an empty body. Both need `devices:core` (doc). `unset TS_TOKEN` after the
calls.

## 4. Preconditions (read-only)

1. Read the names of the networks. Never print a value:

   ```sh
   env | cut -d= -f1 | grep '^TS_.*_CLIENT_ID$'
   ```

   Result: one line for each network, such as `TS_HOME_CLIENT_ID`. A network is
   usable only when its three names are there: `TS_<NET>_CLIENT_ID`,
   `TS_<NET>_CLIENT_SECRET` and `TS_<NET>_TAG`. With more than one network and no
   word from the person, show them in a `list` with `selection: single` and wait
   for a click.

2. Read whether this computer is a member of a network:

   ```sh
   tailscale status --json | jq -r '.BackendState'
   ```

   Result: `Running`. `Stopped`, `NeedsLogin` or `NoState` means that this
   computer has no network, and then the machine would be unreachable after the
   port closes: stop at [Refusals](#refusals). A client of macOS that the app
   owns answers only with `TAILSCALE_BE_CLI=1` in its environment (doc).

## 5. Install the client on the machine

Use the form of step 5 of the skill, with the address of the rent:

```sh
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 root@<address> \
    'curl -fsSL https://tailscale.com/install.sh | sh'
```

The script is the documented install for a distribution with `apt`, `yum` or
`zypper`, and Ubuntu and Debian are in its list (doc). Then prove it with the
same form and `command -v tailscale`: it answers the path of the client. When the
user is not root, put `sudo -n` before the install command.

## 6. Join: the key arrives on standard input

Create the key of [section 3](#3-the-safe-api-calls-on-the-computer-of-the-person),
then pipe it into the machine. The user is root at Hetzner and at RunPod:

```sh
printf '%s\n' "$TS_KEY" | ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 root@<address> '
  set -eu
  umask 077
  trap "rm -f /run/horizon-tailscale.key" EXIT HUP INT TERM
  cat > /run/horizon-tailscale.key
  tailscale up --auth-key=file:/run/horizon-tailscale.key --hostname=build-1 \
      --advertise-tags=tag:horizon --timeout=120s
'
```

A user with `sudo` takes the same script with `sudo -n` on its three commands:

```sh
printf '%s\n' "$TS_KEY" | ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 ubuntu@<address> '
  set -eu
  trap "sudo -n rm -f /run/horizon-tailscale.key" EXIT HUP INT TERM
  sudo -n sh -c "umask 077; cat > /run/horizon-tailscale.key"
  sudo -n tailscale up --auth-key=file:/run/horizon-tailscale.key --hostname=build-1 \
      --advertise-tags=tag:horizon --timeout=120s
'
```

- `--auth-key=file:<path>` reads the file and trims it (source of `tailscale up`).
  The machine makes its login from that file, and the trap removes the file also
  when `tailscale up` fails.
- `--hostname` is the machine name, so the device carries the name that Horizon
  shows. `--advertise-tags` names the tag of the network, and the key already
  carries it (doc).
- `--timeout=120s` bounds the wait; without it `tailscale up` blocks until it
  succeeds (doc).
- Never write the key as `--auth-key=<key>`: the argument is in the process table
  of the machine and in its shell history. Never write it to a file of the
  computer of the person.
- `tailscale up` prints `Success.` and exits 0. A refusal prints the reason and
  the command ends when the bound passes; the file is gone in both cases.

## 7. Proof of the join

On the machine, with the same form as section 6:

```sh
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 root@<address> 'tailscale ip -4'
```

Result: one line, a `100.x.y.z` address, and nothing else.

On the computer of the person, which is the place that must reach the machine:

```sh
tailscale status | grep "$NAME"
tailscale status --json | jq -r '.Peer[] | select(.HostName == "'"$NAME"'") | .DNSName'
tailscale status --json | jq -r '.CurrentTailnet.MagicDNSEnabled'
```

Result: a line with the `100.x` address of the machine, the name `build-1` and
`active` or `idle`; then `build-1.<suffix>.ts.net.`; then `true` (doc).

- `.DNSName` ends with a dot: write the name without that dot.
- MagicDNS is off when `.DNSName` is empty or `MagicDNSEnabled` is `false`. Then
  the name of the machine is its `100.x` address.
- A name of MagicDNS resolves only when this computer accepts the DNS of the
  network. Prove it with `getent hosts <name>` on Linux or `dscacheutil -q host -a
  name <name>` on macOS. An empty answer means `--accept-dns=false` here, and
  then the record holds the `100.x` address.

## 8. The record and the card of Horizon

```text
operation:  machine_set
name:       "build-1"
ssh:        {"host": "build-1.tail1234.ts.net", "user": "root"}
key:        "build-1"
session:    "build-1"
provider:   "Hetzner, hel1"
```

The route is `100.x.y.z` when MagicDNS is off. Then draw the card with three rows
more than step 6 of the skill draws:

```json
{"type": "kv", "rows": [
  {"label": "Network", "value": "home", "mono": true},
  {"label": "Network name", "value": "build-1.tail1234.ts.net", "mono": true},
  {"label": "Internet SSH", "value": "open"}]}
```

`Internet SSH` reads `open` while the provider lets the internet reach port 22,
and `closed` after the next step. The `Address` row keeps the public address of
the rent, so the person sees both routes. A click on the row in the Machines
section of the sidebar then opens the tmux session over the network, as step 8 of
the skill says.

## 9. Prove the login over the network, then close port 22

The order is the whole of this step. First prove that the machine key reaches the
machine through the network, from the computer of the person:

```sh
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 root@build-1.tail1234.ts.net 'true'
```

Exit 0 is the proof. The known-hosts file of the key pins the host key of the new
name on this first contact, and the entry of the public address stays beside it.
A failure means that this computer cannot reach the machine through the network:
then leave port 22 open, write the error line on the card, and stop. Do not close
a port that a proof did not cover.

When the proof answered, close the rule of the rent. The command is the one of
the provider reference that made the rule:

| Provider | The rule of the rent | Command that closes port 22 | Result |
|---|---|---|---|
| Hetzner | Inbound TCP 22 from the address of the person, on `$NAME-fw` | `hcloud firewall delete-rule --direction in --protocol tcp --port 22 --source-ips "$MY_IP/32" $NAME-fw` | The rule is gone; the firewall keeps its other rules (doc). |
| Azure | The rule `$RG-ssh` of the group `$RG-nsg` | `az network nsg rule delete -g "$RG" --nsg-name "$RG-nsg" -n "$RG-ssh"` | The group keeps its other rules (doc). |
| GCP | The rule `$NAME-ssh` of the rent | The delete of step 7 of [gcp](gcp.md) | The rule is gone. |
| AWS | Inbound TCP 22 of the group `$SG` | `$AWS ec2 revoke-security-group-ingress --group-id "$SG" --protocol tcp --port 22 --cidr "$CALLER_IP/32"` | The group keeps its other rules. |
| RunPod | No firewall exists | None | The published port stays reachable from the internet. |

None of those commands ran in this slice. The Hetzner, Azure and GCP forms are
the current documented ones, and the GCP form ran in the run of
[gcp](gcp.md). The AWS form is `UNVERIFIED`: this slice read no AWS document for
it. RunPod publishes `22/tcp` with no source filter and `runpodctl` has no
command that unpublishes it: say this on the card before the join, and join a
RunPod pod only when the person accepts it.

Then prove the closed port from a computer outside the network, or read the rule
list of the provider again. The login of the network still answers, which is the
reason for the order above. Draw `Internet SSH: closed` on the card.

## 10. Delete the machine of a network

The provider delete of step 10 of the skill runs last. Before it:

1. Log the machine out of the network while it still answers, with the form of
   section 6:

   ```sh
   ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
       -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
       -o BatchMode=yes -o ConnectTimeout=10 root@<network name> 'tailscale logout'
   ```

   The command expires the login of the machine (doc). A machine that does not
   answer takes no logout; go on.

2. Remove the device through the API, always, with the two calls of section 3.
   A device that the API removes is gone from the network at once, so the network
   keeps no dead device, no address and no row in the machines list. The delete
   answers 200 with an empty body.

3. Then delete the machine, its record and its key as step 10 of the skill says.

## 11. Find it again

A new session does not remember the network of a machine. Read the names of the
networks as section 4 says, and ask each of them for the device of that hostname
and its own tag:

```sh
GET https://api.tailscale.com/api/v2/tailnet/-/devices?hostname=<machine name>&tags=<that network tag>
```

The network that answers a device is the network of the machine, and its row
holds `nodeId`, `hostname`, `tags` and `name`, the MagicDNS name. The description
`horizon <machine name>` is on the key of the join, and a device filter does not
match on it.

A device that is left in the network costs nothing and holds the machine name, so
a new machine of that name is the second device of that name. Remove the device
of a deleted machine before a new join of the same name.

## 12. Use a machine of the network as a relay

The home computer is a computer of the person that is always on and is a member
of the same network. The agent never changes it. Each step on the home computer
or in the access controls belongs to the person: show it as a short list on the
card with a `Done` button, and go on only after the proof command answers. The
agent changes only the rented machine. No command line of a relay carries a
secret of Horizon.

| What the person sees | Recipe |
|---|---|
| The sites of a browser machine refuse the address of a data centre. | 12.1 Exit node |
| `tailscale ping <machine>` answers through a DERP server, because the network of the person blocks a direct connection. | 12.2 Peer relay |
| A machine must be reached, and it cannot or must not join the network. | 12.3 Jump host |

### 12.1 Exit node: the internet through the home computer

The rented machine sends its internet traffic through the home computer, so a
site sees the address of the home and not the address of the data centre.

First show the two one-time steps of the person on the card, and wait for `Done`:

```json
{"components": [
  {"type": "heading", "text": "build-1", "subtitle": "Steps on the home computer",
   "badge": {"text": "Exit node", "tone": "info"}},
  {"type": "text", "text": "These steps belong to you, on the home computer. Nothing changes there until you do them."},
  {"type": "list", "selection": "none", "rows": [
    {"id": "step-forwarding", "title": "1 Turn IP forwarding on", "subtitle": "sudo sysctl -w net.ipv4.ip_forward=1"},
    {"id": "step-advertise", "title": "2 Offer the home computer as an exit node", "subtitle": "sudo tailscale set --advertise-exit-node, then sudo tailscale up"},
    {"id": "step-approve", "title": "3 Approve it in the admin console", "subtitle": "Machines, Edit route settings, Use as exit node"}]},
  {"type": "buttons", "items": [
    {"id": "done", "label": "Done", "tone": "primary"}, {"id": "cancel", "label": "Cancel"}]}]}
```

One-time steps of the person on the home computer (doc):

```sh
sudo sysctl -w net.ipv4.ip_forward=1
sudo tailscale set --advertise-exit-node
sudo tailscale up
```

Result: the admin console lists the home computer with the badge `Exit Node`.

The `sysctl` line is `UNVERIFIED`: the documentation names IP forwarding as a
requirement and its snippet is missing from the page that this slice read. Keep
the line across a restart with `net.ipv4.ip_forward=1` in a file of
`/etc/sysctl.d/`, and note that a firewall of the distribution (`ufw`,
`firewalld`) can refuse forwarding.

One-time step of the person in the access controls (doc): open **Machines**, find
the home computer, open its menu, select **Edit route settings**, and select
**Use as exit node**. Only an Admin can do this. A policy that names a `dst` for
every grant also needs `autogroup:internet` as `dst`, or the network refuses the
use of an exit node (doc).

The proof on the computer of the person, before the agent goes on (doc):

```sh
tailscale status --json | jq -r '.Peer[] | select(.HostName == "<home name>") | .ExitNodeOption'
```

Result: `true`, which means the home computer offers itself as an exit node and
the network approved it. `false` means that a step above did not finish.

Then the rented machine takes the exit node:

```sh
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 root@<network name> \
    'tailscale set --exit-node=<home name> --exit-node-allow-lan-access=true'
```

The `ssh ...` of this part is the form of section 6. `--exit-node` takes the
name or the address of the exit node, and `--exit-node-allow-lan-access` keeps
the local network of the machine reachable, which holds the DNS of the provider
(doc). Put `sudo -n` before `tailscale set`
when the user of the machine is not root. Before the call, read the internet
address of the machine with `curl -4 -s https://api.ipify.org` and keep it.

The proof, on the machine:

```sh
ssh ... root@<network name> 'curl -4 -s https://api.ipify.org'
```

Result: the address of the home network, which differs from the address that the
machine had. The person reads the same command on the home computer, and the two
answers agree.

The card shows the state in the row `Internet through`, and it holds these
buttons:

```json
{"components": [
  {"type": "kv", "rows": [
    {"label": "Internet through", "value": "<home name>", "mono": true}]},
  {"type": "text", "text": "Internet traffic leaves through <home name>. The ssh session over the network and a browser tunnel keep working."},
  {"type": "buttons", "items": [
    {"id": "stop_exit", "label": "Stop using exit", "tone": "primary"},
    {"id": "close", "label": "Close"}]}]}
```

The value of the row is `this machine` while no exit node is set, and the button
is then `Use <home name> as exit` with the id `use_exit`.

Two things keep working with an exit node set: the ssh session of Horizon over
the network, and the tunnel of a browser machine. The exit node carries the
internet traffic of the machine (doc), and both of those carry the traffic of the
network itself.

Undo, and the state of the card, when the person wants the address of the data
centre again:

```sh
ssh ... root@<network name> 'tailscale set --exit-node='
```

The card returns to `Internet through: this machine`.

When the home computer is off, the machine has no internet: the exit node is the
only route out. Detect it with a bounded call and tell the person on the card:

```sh
ssh ... root@<network name> 'curl --max-time 8 --silent --show-error --output /dev/null https://api.ipify.org'
```

A non-zero exit, and exit code 28 for the bound, is the answer. Draw a `warn`
badge `No internet`, one `text` line that says that the home computer is away and
that internet traffic of the machine stops with it, and the button
`Stop using exit`, so the person returns the machine to its own route. The ssh
session of Horizon over the network still answers, so the agent can run this call
and the undo. `curl` is missing on a small image: install it once with
`apt-get install -y curl` and the form of section 5.

### 12.2 Peer relay

Use it when a direct connection is impossible. Peer relay is a new feature, and
it needs Tailscale 1.86 or later on every device of the path (doc), so update the
home computer and the machine first.

1. The person, on the home computer: `sudo tailscale set --relay-server-port=40000`,
   then open UDP 40000 inbound in its firewall (doc).
2. The person, in the access controls: add a grant with
   `"app": {"tailscale.com/cap/relay": []}` of `tag:horizon` as `dst`, and the
   devices that need the relay as `src`; name no `*` in `src` (doc).
3. The proof, on the computer of the person: `tailscale status | grep peer-relay`
   shows `peer-relay <ip>:<udp port>:vni:<id>` for the machine (doc), and
   `tailscale ping <machine>` names the peer relay.

The agent runs no command of this recipe on the home computer. The undo is
`sudo tailscale set --relay-server-port=""` on that computer (doc).

### 12.3 Jump host

Use it when the machine must stay off the network, or when its firewall allows
only the address of the home. The record of Horizon keeps the public address, and
the person adds one block to `~/.ssh/config`:

```text
Host 203.0.113.10
    ProxyJump home-gw
```

Horizon reads that file to list the host aliases of the sidebar, and a record may
name an alias as its host. The `ssh` that Horizon starts carries no `-F` and no
`ProxyJump`, so a `Host` block applies to the destination, and the options that
Horizon sets on the command line win over the file. A directive of the command
line applies to the destination and not to the jump host (doc, `ssh(1)`), so the
hop uses the login, the keys and the agent of the person. The known-hosts file of
the machine key pins the host key of the hop at the first contact, because Horizon
sets `UserKnownHostsFile` for the whole connection.

The provider firewall then allows TCP 22 from the address of the home only:
replace the source of the rule of the rent, and prove the login of the machine key
through the hop before the person relies on it.

## 13. Reach the Horizon server of the home computer over the same network

The server of Horizon can listen inside the network, and then no port of the home
router is open to the internet.

On the home computer, start it on the address of the network:

```sh
horizon --server --listen <100.x address of the home computer>:7000 --name <its network DNS name>
```

`--listen` takes one address and one port, and it accepts only loopback, an
address of `100.64.0.0/10` or an address of `fd7a:115c:a1e0::/48`. A wildcard
address and the address of the local network are refused. Horizon encrypts
nothing on this link, so the network must. `--name` gives the name that a link of
that server holds; use the network name when MagicDNS is on, and no name when the
other computer reaches the address directly.

Then ask for the first invitation on that computer, `horizon --server-invite`,
which prints the link `horizon://<name>:7000/join#...`, the code and a QR code.
On another computer of the same network, **Settings → Server** takes the whole
link in one field and the join goes over the network. The state line of that tab
says what the link does now.

## 14. Refusals

Write each of these as a `text` line on the card, and do nothing else.

- **This computer of the person is not a member of the network.** Section 4 shows
  `Stopped` or `NeedsLogin`, or the machine is not in `tailscale status` after the
  join. Then the machine is unreachable when port 22 closes, so the port stays
  open. Say why, and offer to join this computer to the network or to keep the
  public address.
- **The tag is not owned by the client.** The key call answers 400 or 403
  (`UNVERIFIED`: this slice made no such call). Quote the code and the tag only,
  never the body in full, and never a header. Then the person adds the tag to
  `tagOwners` and gives it to the client at **Trust credentials**, or uses the tag
  of the client.
- **One of the three names is missing.** A network without all three names is not
  a network of this recipe. Say which name of the three is missing, and never
  offer to create it with a value that the agent does not hold.
- **A machine of the same name is already a device.** Remove that device first,
  as section 11 says, or the new machine is the second device of the name.

## 15. What can go wrong

| Symptom | Reason and next step |
|---|---|
| `tailscale up` answers `invalid key` | The key is spent, older than ten minutes, or from another network. Make one more key and run section 6 again. |
| `tailscale up` answers a login URL | The key did not reach the file, or the file is empty. Read the trap, and write the key again. Never open the URL. |
| The machine is `active` and the login over the network fails | The access controls do not let this computer reach TCP 22 of `<TAG>`. Ask the person for the grant. |
| `tailscale ip -4` answers nothing | The join did not finish. Read `tailscale status` on the machine, and do not close port 22. |
| The name of MagicDNS does not resolve here | MagicDNS is off, or this computer has `--accept-dns=false`. Use the `100.x` address in the record. |
| The device list holds two rows of one name | A machine of that name joined twice, or an old device stayed. Remove the row that does not answer. |
| `403 Forbidden` on the devices call | The client lacks `devices:core`, or its token is older than one hour. Make a token again. |
| The exit node stays `false` | The home computer did not advertise itself, or the network did not approve it. Both steps are the person's. |
| The machine answers `100.x` but no internet | The exit node went away, or its forwarding is off. Tell the person, and offer `Stop using exit`. |
| A second `DELETE` answers no 200 | The device is already gone. Go on with the delete of the machine. |
