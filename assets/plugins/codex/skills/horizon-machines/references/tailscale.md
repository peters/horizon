# A Tailscale network with the API

One rented machine joins one network of the person. The person stores the OAuth
client of the network as three named secrets. The agent uses the client secret on
the computer of the person only, and the machine receives one auth key for one
login.

`(doc)` marks a command, a flag, a body or a name that the current public
Tailscale documentation or the source of `tailscale up` states, read on
2026-11-12. `UNVERIFIED` marks one that those documents do not state, and the
reason follows it. The other statements of this page ran in no live run, so a
reviewer proves every one.

## What a live run has proven so far

- The token answer of an OAuth client that has the scope `auth_keys` only and one
tag holds `"scope": "auth_keys"` and `"tags": "tag:<name>"`. That client makes
keys. It reads, changes and deletes no device.

A statement that this list does not hold is `(doc)` or `UNVERIFIED`.

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
**OAuth** (doc). It has one of two shapes, and the person chooses it:

| Shape | Scopes | What the agent can do | The key of the machine |
|---|---|---|---|
| Narrow: enough for a machine that this set-up rents | `auth_keys` (write) | Make one key. Read, change and delete no device | `ephemeral: true`, so the node leaves the network with the machine |
| Wide | `auth_keys` (write) and `devices:core` (read and write) | Make one key and remove the device of a deleted machine | The key, and a delete through the API |

A client with a devices scope can touch every machine of the network, and a
narrow client makes the node clean itself up. So the narrow client is the
default of this page, and the wide client is a choice of the person. The agent
never asks the person for the wide client.

The rest of the client:

- The tag `<TAG>` as the tag of the client. The tag must exist in
  `tagOwners` of the access controls, and the client is limited to it.
- A key that the client makes must carry a tag that the client owns. The API
  refuses any other tag with 400 or 403. Another tag works only when `tagOwners`
  gives `<TAG>` to the tag of the client (doc).

The access controls of the network must also let the devices of the person reach
TCP 22 of `<TAG>`. A tagged device is not a member of the network, so a rule
whose `dst` is `autogroup:member` does not include it. Name `tag:horizon` or `*`
in `dst` (doc). That rule is needed only while this computer is in the same
network, as section 9 says.

### 1.1 A machine that must reach devices of the network

The machine reaches the devices of the network only through a grant, and the
grant is in the policy file. Only the person writes that file: the agent never
writes it, and the agent never asks for a client that can.

Propose a tag for the purpose, for example `tag:work`, and give the person this
block. The person adds it in the admin console, at **Access controls**, after
the preview of the change answers without an error (doc for the names of the
fields):

```json
{
  "tagOwners": {"tag:work": ["autogroup:admin"]},
  "grants": [
    {"src": ["tag:horizon"], "dst": ["tag:work"], "ip": ["tcp:443", "tcp:22"]}
  ]
}
```

- `src` is the tag or the name of the machine, or a name of a user.
- `dst` is the tag of the devices, a name, or a `100.x` address.
- `ip` is the list of ports and protocols, or omit the field to allow every port
  of that destination (doc).

UNVERIFIED: this page read no policy file of a network live, so the person
checks the preview of the console before the person saves the file.

### 1.2 A machine that two networks must reach

One machine joins one network (see section 6). When a second network must reach
the same machine, the person shares the device of the machine with a user of
that network: in the admin console, on the page of the device, with the sharing
function (doc). The device then appears in the network of that user, and a rule
of that network reaches it.

UNVERIFIED: the exact place of the sharing function in the console. This page
read no console for it.

The agent shares no device and removes no share. Two networks mean one client on
the machine and no second client, because a second client would fight the first
for the routes of the machine.

## 2. The two rules of a secret here

- **The client secret stays on the computer of the person.** The documentation
  allows `tailscale up --auth-key=<client secret>`, and this recipe does not use
  it: that form copies the secret to the machine, to its shell history and to its
  process table. The agent uses the secret for one API call here, and the machine
  receives a key that the call made.
- **The auth key arrives on standard input.** It travels in the standard input of
  `ssh` to a file under `/run` that only root reads, and the same command removes
  the file. No argument and no environment variable of the machine holds it.
- **The agent asks the person for no value.** A card names the three names and the
  place that stores them. Never ask the person to type a client id, a secret or a
  tag in the panel, and never take one from the conversation.

## 3. The safe API calls on the computer of the person

`-d "client_secret=$TS_HOME_CLIENT_SECRET"` puts the secret in the arguments of
`curl`, where another process of the same user reads it, on Linux from
`/proc/<pid>/cmdline`. The value goes on standard input instead. `printf` writes that pipe, and it is a
builtin of `bash`, `zsh`, `dash` and of `busybox sh`, so the value is in no
argument list of its own: `command -V printf` answers `printf is a shell builtin`.

One token, which lives one hour (doc). The answer gives the token and the
scopes of the client, so keep it once in a variable of this shell:

```sh
TS_ANSWER=$(printf '%s' "$TS_HOME_CLIENT_SECRET" | curl --silent --show-error --fail \
  --data-urlencode "client_id=$TS_HOME_CLIENT_ID" \
  --data-urlencode client_secret@- \
  https://api.tailscale.com/api/v2/oauth/token)
TS_TOKEN=$(printf '%s' "$TS_ANSWER" | jq -r '.access_token')
TS_SCOPE=$(printf '%s' "$TS_ANSWER" | jq -r '.scope')
unset TS_ANSWER
```

The endpoint takes the client credentials grant and answers `access_token` and
`expires_in` 3600 (doc). `--data-urlencode name@-` reads the value from standard
input, and `curl` encodes it (doc).

`TS_SCOPE` is the scope of the client, and the key follows it. A client of
`auth_keys` only answers `auth_keys`; a client with a devices scope names that
scope in the answer (doc). The answer of the narrow client is proven live. Choose
the body of the key by that value, and never widen the scope of the client.

One auth key for this machine (doc):

```sh
TS_KEY=$(printf 'Authorization: Bearer %s\n' "$TS_TOKEN" | curl --silent --show-error --fail \
  --header @- --request POST \
  --data '{"capabilities":{"devices":{"create":{"reusable":false,"ephemeral":true,"preauthorized":true,"tags":["tag:horizon"]}}},"expirySeconds":600,"description":"horizon build-1"}' \
  https://api.tailscale.com/api/v2/tailnet/-/keys | jq -r '.key')
```

That body is the one of a narrow client: `ephemeral` is `true`, so the node
leaves the network by itself after the machine stops (doc), and the person
removes nothing in the console.

The wide client takes the same call with `"ephemeral":false` in place of
`"ephemeral":true`. Section 10 then removes the device with the two calls below.

`-H @-` reads the header lines from standard input (doc). The body holds the tag,
the expiry and the machine name, and no secret. `-` is the tailnet of the token
(doc). The answer holds the key in `.key` only at creation (doc), so no other
program may print that answer. `jq` is required for these calls: prove `command -v jq` before the first.

The one body `UNVERIFIED`: the fields `capabilities.devices.create.reusable`,
`ephemeral`, `preauthorized` and `tags` are the documented ones, and the shortest
accepted `expirySeconds` is not stated. 600 is ten minutes, and the key is spent
by one login.

The device of one hostname, and its removal (doc). Only a client with a devices
scope reaches these two calls; a narrow client answers 403:

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
   for a click. A panel keeps the environment of its start, so a network that the
   person stores while this panel runs is in no pill. Ask the person to open a new
   agent panel, or to start Set Up again, after the three names are stored.

2. Read the state of this computer, and change nothing there:

   ```sh
   tailscale status --json | jq -r '.BackendState, .CurrentTailnet.MagicDNSSuffix'
   ```

   Result: `Running`, then the suffix of the network of this computer, for
   example `tail1234.ts.net`, or an empty line when MagicDNS is off. `Stopped`,
   `NeedsLogin` or `NoState` means that this computer is in no network, and its
   second line is empty.

   **Never run a command that changes the Tailscale state of this computer:**
   `up`, `down`, `login`, `logout`, `switch` or `set`. This computer can be in a
   network of the person's work, and such a command moves it out of that
   network. The comparison of the suffix with the suffix of the machine in
   section 7 says whether this computer is in the same network.

   A client of macOS that the app owns answers only with `TAILSCALE_BE_CLI=1` in
   its environment (doc).

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

Then read the suffix of the network of the machine, and its own name:

```sh
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 root@<address> \
    'tailscale status --json | jq -r ".MagicDNSSuffix, .Self.DNSName"'
```

Result: `<suffix>`, for example `tail1234.ts.net`, then
`build-1.<suffix>.ts.net.` (doc for the two fields).

The suffix of this computer is the second line of section 4.2:

- Two equal suffixes, and `Running` here: this computer is in the network of the
  machine. Prove the login of section 9, and then close port 22.
- A different suffix, an empty line, or `Stopped`: this computer is in no network
  or in a network of the person's work. Keep the public SSH rule with the machine
  key, write the line of section 8.4 on the card, and close nothing.

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
more than step 6 of the skill draws, as section 8.4 shows.

`Internet SSH` reads `open` while the provider lets the internet reach port 22,
and `closed` after the next step. The `Address` row keeps the public address of
the rent, so the person sees both routes. A click on the row in the Machines
section of the sidebar then opens the tmux session over the network, as step 8 of
the skill says.

The card of a machine that joined a network holds the four documents below: the
card that says what the person stores, the join card, the steps card of the
join, and the machine card of a computer that is not in the network.

### 8.1 The card that says what the person stores

Draw this card when the panel holds no stored network, or when one of the three
names of a network is missing. It names the three names and the place of the
OAuth client, and it asks for no value.

```json
{"components": [
  {"type": "heading", "text": "Store a network in Settings",
   "subtitle": "A machine joins a network that this computer stores.",
   "badge": {"text": "Needs you", "tone": "warn"}},
  {"type": "kv", "rows": [
    {"label": "Where", "value": "Settings, Secrets"},
    {"label": "TS_<NET>_CLIENT_ID", "value": "The client id of the OAuth client", "mono": true},
    {"label": "TS_<NET>_CLIENT_SECRET", "value": "The client secret of the same client", "mono": true},
    {"label": "TS_<NET>_TAG", "value": "The tag of the machine, for example tag:horizon", "mono": true}]},
  {"type": "text", "text": "Make one OAuth client in the Tailscale admin console, at Trust credentials, OAuth. Add the three names in Settings, Secrets, with Add. Then open a new agent panel, or start Set Up again, and press Join a network."},
  {"type": "buttons", "items": [{"id": "close", "label": "Close"}]}]}
```

`<NET>` is the name of the person: `[A-Z0-9]{1,16}`. Say in one `text` line
which of the three names is missing. Never ask for a client id, a secret or a
tag in the panel, and never take one from the conversation.

### 8.2 The join card

The button `Join a network` of a machine card draws this card. The pills are the
stored networks of section 4.1, and the row `network.none` is first.

```json
{"components": [
  {"type": "heading", "text": "Join build-1 to a network",
   "subtitle": "One machine joins one network.", "badge": {"text": "Network"}},
  {"type": "caption", "text": "Network"},
  {"type": "list", "selection": "single", "layout": "pills", "rows": [
    {"id": "network.none", "title": "None", "selected": true},
    {"id": "network.home", "title": "home"},
    {"id": "network.factory", "title": "factory"}]},
  {"type": "text", "text": "The machine takes the tag of the network and its own name. The node leaves the network by itself when this machine is deleted."},
  {"type": "buttons", "items": [
    {"id": "join", "label": "Join home", "tone": "primary"},
    {"id": "cancel", "label": "Cancel"}]}]}
```

- The label of the button names the selected network: `Join home`.
- The row `network.none` and `Cancel` join nothing.
- A machine that is already in a network gets the card of its state and no
second join: the heading names the network, the card says that one machine joins
one network, and it holds `Close` only.

### 8.3 The steps card of the join

```json
{"components": [
  {"type": "heading", "text": "Joining the network home",
   "subtitle": "build-1, a machine of Hetzner.",
   "badge": {"text": "Working", "tone": "info"}},
  {"type": "progress", "label": "Join the network", "percent": 40, "detail": "2 of 5"},
  {"type": "steps", "steps": [
    {"label": "Read the state of this computer", "state": "done", "at": "1 s"},
    {"label": "Make one auth key", "state": "done", "at": "2 s"},
    {"label": "Install the client on the machine", "state": "now", "at": "now"},
    {"label": "Join the network home"},
    {"label": "Prove the login over the network"}]},
  {"type": "text", "text": "The key arrives on standard input and leaves no file on the machine. Port 22 stays open until the proof answers."},
  {"type": "buttons", "items": [{"id": "stop", "label": "Stop", "tone": "danger"}]}]}
```

The steps card of a machine that a recipe prepares holds the step
`Join the network <NET>` in its own list, as `references/repository-machine.md`
says.

### 8.4 The machine card, and a computer that is not in the network

The machine card of step 6 keeps its rows and adds the three rows of this page.
The line below the rows is the one of a computer that is not in the network of
the machine.

```json
{"components": [
  {"type": "heading", "text": "build-1", "subtitle": "Hetzner, hel1",
   "badge": {"text": "Running", "tone": "ok"}},
  {"type": "kv", "rows": [
    {"label": "Network", "value": "home", "mono": true},
    {"label": "Network name", "value": "build-1.tail1234.ts.net", "mono": true},
    {"label": "Internet SSH", "value": "open"},
    {"label": "Address", "value": "203.0.113.10", "mono": true}]},
  {"type": "text", "text": "This computer is not in the network home; the machine is reached over its public address."},
  {"type": "buttons", "items": [
    {"id": "connect", "label": "Connect", "tone": "primary"},
    {"id": "join_network", "label": "Join a network"},
    {"id": "delete", "label": "Delete", "tone": "danger"}]}]}
```

When the two suffixes of section 7 are equal, draw the rows `Network` and
`Network name` with `Internet SSH: open` until section 9 closes the port, and draw
no line about this computer. A client of `auth_keys` only leaves the node of an
ephemeral key out of the console after the machine goes, and the card says so.
A card of a machine with a non-ephemeral key says that the person removes the
node in the console.

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
| Hetzner | Inbound TCP 22 from every address, on `$NAME-fw` | `hcloud firewall delete-rule --direction in --protocol tcp --port 22 --source-ips 0.0.0.0/0 $NAME-fw` and the same line with `--source-ips ::/0` | The rule is gone; the firewall keeps its other rules (doc). |
| Azure | The rule `$RG-ssh` of the group `$RG-nsg` | `az network nsg rule delete -g "$RG" --nsg-name "$RG-nsg" -n "$RG-ssh"` | The group keeps its other rules (doc). |
| GCP | The rule `$NAME-ssh` of the rent | The delete of step 7 of [gcp](gcp.md) | The rule is gone. |
| AWS | Inbound TCP 22 of the group `$SG` | `$AWS ec2 revoke-security-group-ingress --group-id "$SG" --protocol tcp --port 22 --cidr 0.0.0.0/0` | The group keeps its other rules. |
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

2. Remove the device through the API with a wide client: the two calls of
   section 3. A device that the API removes is gone from the network at once, so
   the network keeps no dead device, no address and no row in the machines list.
   The delete answers 200 with an empty body. A narrow client answers 403 here,
   and that is not a fault: the node of an ephemeral key leaves the network by
   itself after the machine stops (doc).

3. Then delete the machine, its record and its key as step 10 of the skill says.
   The card says which node stays in the console, if any.

## 11. Find it again

A new session does not remember the network of a machine. A wide client finds it
through the device list; a narrow client asks the machine itself.

With a wide client, read the names of the networks as section 4 says, and ask
each of them for the device of that hostname and its own tag:

```sh
GET https://api.tailscale.com/api/v2/tailnet/-/devices?hostname=<machine name>&tags=<that network tag>
```

The network that answers a device is the network of the machine, and its row
holds `nodeId`, `hostname`, `tags` and `name`, the MagicDNS name. The description
`horizon <machine name>` is on the key of the join, and a device filter does not
match on it.

With a narrow client, the machine answers for its own device. Prove the login of
section 7 with the machine key and read `tailscale status --json` on the machine:
`.Self.DNSName` is the MagicDNS name and `.Self.Tags` holds the tag. Ask the
person which network that name belongs to, because the panel holds no device
list.

A device that is left in the network costs nothing and holds the machine name, so
a new machine of that name is the second device of that name. Remove the device
of a deleted machine before a new join of the same name. With a narrow client the
node of a machine that is gone leaves by itself (doc).

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

- **This computer of the person is not in the network of the machine.** The two
  suffixes of section 7 differ, or this computer answers `Stopped` or `NeedsLogin`.
  Then the machine is unreachable through the network, so port 22 stays open and the
  machine is reached over its public address. Write the line of section 8.4, and
  offer no close. Do not join this computer to a network: section 4 forbids it.
- **The machine is already in a network.** One machine joins one network. Say which
  network it joined, and offer no second join. A second network reaches the machine
  through the sharing function of section 1.2, and the person does that.
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
- **The client of `auth_keys` only answers 403 on a device call.** That is the
  scope of the client and no fault. Read the device on the machine, as section 11
  says, and remove nothing.

## 15. What can go wrong

| Symptom | Reason and next step |
|---|---|
| `tailscale up` answers `invalid key` | The key is spent, older than ten minutes, or from another network. Make one more key and run section 6 again. |
| `tailscale up` answers a login URL | The key did not reach the file, or the file is empty. Read the trap, and write the key again. Never open the URL. |
| The machine is `active` and the login over the network fails | The access controls do not let this computer reach TCP 22 of `<TAG>`. Ask the person for the grant. |
| `tailscale ip -4` answers nothing | The join did not finish. Read `tailscale status` on the machine, and do not close port 22. |
| The name of MagicDNS does not resolve here | MagicDNS is off, or this computer has `--accept-dns=false`. Use the `100.x` address in the record. |
| The device list holds two rows of one name | A machine of that name joined twice, or an old device stayed. Remove the row that does not answer. |
| `403 Forbidden` on the devices call | A narrow client, or its token is older than one hour. With a narrow client the scope is the reason and no fault: read the device on the machine, as section 11 says. |
| The card says that this computer is not in the network | The two suffixes of section 7 differ. Port 22 stays open and the public address is the route. That is the normal state when the computer is in a network of the person's work. |
| The node is still in the console after the delete | The key was not ephemeral, or the machine did not answer the logout. The card says which. The person removes the node in the console. |
| The exit node stays `false` | The home computer did not advertise itself, or the network did not approve it. Both steps are the person's. |
| The machine answers `100.x` but no internet | The exit node went away, or its forwarding is off. Tell the person, and offer `Stop using exit`. |
| A second `DELETE` answers no 200 | The device is already gone. Go on with the delete of the machine. |
