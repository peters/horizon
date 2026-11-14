---
name: horizon-machines
description: >-
  Rent, connect, stop and delete a cloud machine for the person from a Horizon card, with the
  provider CLI in your own shell. Use it when the person asks for a machine, a cloud, a worker, a
  GPU box, to rent, stop or delete one, or for "what is rented", "show my machines" and "overview".
  The card uses block_panel (horizon-block) and Connect uses open_terminal with a Horizon machine
  key.
---

# Horizon machines

Use this skill to give the person a machine on a cloud provider, and to keep it
until they delete it. `block_panel` draws the card, `horizon-block` holds its
contract, and `open_terminal` gives the shell.

## The model

- Horizon holds no provider code and no provider credential.
- You run the provider CLI in your own shell, with the person's own login.
- A card shows the machine and the state that you read from the CLI.
- The person's clicks come back to you through `next_click`.
- Connect is `open_terminal` with a machine key that Horizon makes and keeps.
- The same key lets you run your own `ssh` for the commands that prepare the
  machine, with the one form in step 5.
- Every machine that is rented appears in one overview card, with one row for
  each machine. The overview rents nothing.
- Every machine that you finish setting up is also a record of this Horizon,
  written with `machine_set`: the person sees it in the Machines section of the
  sidebar and connects with one click, without you. The same machines are in the
  overview card and in the sidebar.
- The person can put a workspace on a machine from the sidebar, and the new
  terminals of that workspace then run there in tmux. You do nothing for it.
- A terminal that attaches a tmux session reconnects by itself when its link was
  lost: its titlebar says `Reconnecting`, it keeps its last screen, and it runs
  its command again after 3, 6, 12 and 24 seconds, then every 30 seconds, until
  it reaches the session again. You do nothing for it, and a terminal that the
  person closed tries nothing.

## The flow

Do the steps in this order. Stop and tell the person when a step fails.

A request for "what is rented", "show my machines" or "overview" is the
overview card after step 1. Show it and stop; it rents nothing.

### 1. Find the CLIs

Run one read-only command per provider. Run no create command yet.

- Hetzner: `hcloud server list --quiet`
- Azure: `az account show -o json --query "{name:name,id:id,state:state}"`
- GCP: `gcloud auth list --filter=status:ACTIVE --format="value(account)"`
- RunPod: `runpodctl user -o json`
- AWS: `aws sts get-caller-identity`, then the `--dry-run` preflight of `references/aws.md`

A missing CLI and a CLI without a login are both `warn` rows, with the provider's
own words in the subtitle. Tell the person which command to run, and wait.

### After step 1: the overview card

A new session can find the machines that an earlier session registered with
`{"operation": "machines"}`: each record has its name, route, key, tmux session
and provider line, and `terminal_open` says whether a terminal of it is open now.
`key_missing` says that the record binds a key that this computer does not hold,
because a person deleted it here or on another computer. Tell the person that the
same machines are in the Machines section of the sidebar, where one click
connects.

The same answer also says whether a change waits for the server of the person and
what the link to that server does now: `pending` on a record and `server` on the
answer, `off` when this Horizon keeps its machine list alone. A machine that
another computer added is in this list too, and its key arrives with the server, so
a click on it connects as soon as the key is here. A machine whose key is missing
opens nothing here: ask the person before you call `machine_key` for a new key,
then put the new public line in the `authorized_keys` of the machine. Do not work
around a missing key with a key of the person for the second computer.

A computer reaches the server over the SSH login of the server computer, or with a
link that a logged-in computer shows as a QR code. The person joins it in
**Settings → Server**: one field takes the whole link, one takes the name of the
computer, and a successful join writes the address of the server. You cannot make
an invitation yourself: the tab button **Add a computer** and the mode
`horizon --server-invite` belong to the person. The state line of that tab and
the sidebar say what the link does now, and `removed` or `not_logged_in` mean that
the server no longer accepts this computer: it deleted its token and stops until
the person joins it again. A computer that the person removed keeps the copies of
the machine keys and the values that it already had.

"What is rented", "show my machines" and "overview" mean one overview card, not
one card per machine. It is a list of every machine that the providers hold, in
the spirit of a machine list in a sidebar. It rents nothing and deletes nothing.

- The heading is `Machines`. The subtitle says how many machines you found at
  how many providers, for example `3 machines at 2 providers`. Add a badge only
  when something needs the person, for example a provider whose CLI is not
  signed in: `{"text": "1 provider skipped", "tone": "warn"}`.
- Make one `list` with `selection: single`: one row per machine that the
  find-again command of a signed-in provider returns for the tag
  `horizon-managed=true`. The row id is `<provider>.<machine name>`, for
  example `hetzner.build-1`. The title is the machine name. The subtitle is the
  provider, the region and the state, for example `Hetzner, hel1, running`. Use
  tone `warn` for a machine that is stopped but still costs, and tone `danger`
  for a machine that a command reports as an error state. Mark the first row
  `selected: true`.
- Show at most 32 rows, the limit of one `list`. When more machines exist, show
  the 31 newest and a last row with the id `more` and the title `and 3 more`.
  That row is no machine: it is never the `selected: true` row, and you answer a
  click on it with a `text` line that says how many machines the overview leaves
  out, and do nothing else.
- Add the buttons `Connect` with tone `primary`, `Open card`, `Refresh` and
  `Close`. `Delete` is not on the overview: it stays on the machine card with
  its confirm step.
- Add one `text` line for each provider that step 1 skipped, with the reason in
  the provider's own words, for example `GCP skipped: gcloud is not signed in.`.
- End with a `text` line that says when you read the state, for example
  `Read at 2026-11-10 20:04 UTC. Press Refresh to read it again.`, and then the
  buttons.
- The overview is your reading at the time, not Horizon state. A press on
  `Refresh` runs the find-again command of every signed-in provider again and
  updates the card. Run the same read after each rent, stop, start and delete
  that you do, and update the card.
- Call `machine_keys` with the overview. A machine without a key of its name in
  this workspace gets `, no key here` added to its subtitle. `Connect` on that
  row answers with a `text` line that says that no key of this machine is in
  this workspace, and opens no terminal.
- A press carries the selected row in `selected`. `Connect` opens the terminal
  of step 8 with the key of that machine. `Open card` opens the machine card of
  step 6 for that machine, or shows the card when it still exists, and leaves
  the overview open. `Close` closes the overview.
- When no provider holds a machine, show the empty overview: the subtitle
  `Nothing is rented`, one `text` line that says that every signed-in provider
  answered with an empty list, and the buttons `Rent a machine` with tone
  `primary` and `Close`.

The exact documents are under [Card documents](#card-documents).

### 2. Choose the provider on a card

Make one card. Put a `list` with `selection: single`, one row per provider, and
mark the best choice with `selected: true`. The best row is a provider that step
1 proves and that can rent: it carries the badge `Ready` with tone `ok`. A
provider that cannot rent is a row with `disabled: true`, whose second line names
the command that signs in, in the same words as the card of `horizon-setup`. Add
`Continue` with tone `primary` and `Cancel`, then wait for a click.

Pick the machine name now: `[a-z0-9-]{1,40}`, with no leading or trailing `-`,
for example `build-1`. The name is the card title, the key name, the
`horizon-card` tag value and the session name. Reuse no name of a running machine.

### 3. Choose the region and the machine type

Ask in the same way on the same card: replace the list with the regions and keep
`Continue` and `Cancel`. Keep at most 8 rows, mark the sensible first choice
`selected: true`, and draw the regions as `layout: pills` under the caption
`Place` when they are short names. Then ask for the machine type, again with at
most 8 rows under the caption `Size`, and put the list price of each type in its
`value` as the provider CLI answered it in this run, such as
`0.0079 EUR / hour`. A type whose price the CLI does not answer holds
`price not read`. The click carries the chosen rows in `selected`.

A row names the type, its true cores and its memory, as the provider answered
them, and never only what the name of the row suggests: `cpx21, 3 vCPU, 4 GB`
says what the machine is. When the person takes a size of Set up, Small is the
cheapest x86 type with shared cores that holds at least 2 vCPU and 4 GB, Medium
at least 4 vCPU and 8 GB, and Large at least 8 vCPU and 16 GB. A size that the
place does not offer is a row with `disabled: true` that names the reason, and it
is never the selected row.

Read the types, their cores, their memory and their prices from the provider in
this run, at the place that the person chose. The three sizes are the cheapest
types of that provider that hold their pair at that place and at that moment, so
two runs of the same card at the same moment give the same three rows: never
keep a size from an earlier card, a document or the name of a type. When two
types cost the same, take the one with more vCPU, then the one with more memory,
and then the type whose name sorts first.

### 4. Make the key and rent

Call `machine_key` with the name before you rent. The answer is `{name,
public_key, private_key_path, known_hosts_path, created}`: Horizon keeps the key
files and returns the public key line with the two paths, never the text of the
private key. A later call returns the same answer with `created: false`. A name
outside the rules answers `invalid_key_name`; a home that holds the maximum
answers `too_many_keys`.

A name is one key on every computer of the person that follows the same server: a
key that this call makes is on the server within a few seconds, and the other
computers log in with it. Do not make a second key for a second computer, and do
not copy the private key file to another computer yourself.

Then run the rent of the provider reference with that public key. Tag every
resource that the provider can tag with `horizon-managed=true` and
`horizon-card=<name>`. Put the name in the resource name when the provider has no
tag, as `references/gcp.md` and `references/runpod.md` say. Keep the returned ids
in your notes to delete later.

### The SSH rule of a rented machine

Open SSH to every address, and let the machine key be the protection:

- The machine holds no password of the person. The image has no root password,
  and the provider puts the public key of the machine key in `authorized_keys`.
  Step 5 proves that the key logs in and that the ssh server refuses a password.
- A rule that names the address of this computer at this moment locks the person
  out when the address changes, when the person travels, and for every other
  computer of the person. Item 4 of the work of Set up asked for this.
- An image with no provider firewall allows every port to the internet. Give the
  machine a firewall with one inbound rule for TCP 22 and no other, as each
  provider reference shows, and keep its other ports closed.

A rule narrowed to one address stays a choice of the person: when the person asks
for it in words, set the source of the TCP 22 rule to the address of this computer
as `/32`, and say on the card that another network address and another computer
cannot reach the machine after that. The section "A narrow SSH rule" gives the
commands.

### 5. Wait for SSH and prepare the machine

### 5. Wait for SSH and prepare the machine

A rent is not ready when the create command returns. Update the card to the step
list of this work, with the time of each finished step and `now` for the step
that runs, as the card of `horizon-setup` draws it:

```json
{"components": [
  {"type": "heading", "text": "Renting the machine", "subtitle": "Small, Helsinki, Hetzner.",
   "badge": {"text": "Working", "tone": "info"}},
  {"type": "progress", "label": "Rent and prepare", "percent": 40, "detail": "2 of 5"},
  {"type": "steps", "steps": [
    {"label": "Make a machine key for Horizon", "state": "done", "at": "2 s"},
    {"label": "Create the machine at the provider", "state": "done", "at": "11 s"},
    {"label": "Wait for SSH with the machine key", "state": "now", "at": "now"},
    {"label": "Check tmux on the machine"},
    {"label": "Add the machine to Machines"}]},
  {"type": "buttons", "items": [{"id": "stop", "label": "Stop and delete the machine", "tone": "danger"}]}
]}
```

Then connect with the machine key and the one form below. Use the `user` of the
provider reference; add `-p <port>` only where the reference names a port.

```
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 <user>@<address> '<command>'
```

`<private_key_path>` and `<known_hosts_path>` are the fields of the `machine_key`
answer. `IdentitiesOnly` and `IdentityAgent=none` offer that one key, so a long
ssh agent of the person cannot exhaust `MaxAuthTries`. `BatchMode=yes` fails at
once instead of asking for a password, and `ConnectTimeout=10` keeps a dead
address from hanging.

`-i` takes the path as it is, and the path holds no percent sign in a normal
home. If the path holds a `%`, give the key as
`-o IdentityFile=<path with every % written twice>` instead of `-i`, and write
the `%` twice in `<known_hosts_path>` too.

1. Wait until SSH answers: run the form with `true` as `<command>` every 10
   seconds, for at most 3 minutes. Update the card between the tries, and never
   show `Connect` before a try answers.
2. Prove that the login is key-only, which is the protection of the rule that
   lets every address in. The key answered step 1 with `BatchMode=yes`, so this
   asks the ssh server what it accepts:

   ```
   sudo sshd -T | grep -Ei '^(passwordauthentication|kbdinteractiveauthentication|permitrootlogin)'
   ```

   `passwordauthentication no` and `kbdinteractiveauthentication no` are the
   answer of every Canonical, Debian and Hetzner cloud image. A machine that
   answers `yes` accepts a password from every address, so tell the person, and
   turn it off only when the person agrees:

   > CAUTION: `sshd -T` and the next command change who can log in. A mistake
   > here closes the login of the machine. Keep the shell of step 1 open until
   > the new login answered.

   ```
   printf 'PasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitRootLogin prohibit-password\n' \
     | sudo tee /etc/ssh/sshd_config.d/10-horizon.conf
   sudo sshd -t && sudo systemctl reload ssh 2>/dev/null || sudo systemctl reload sshd
   ```

   Run the `sshd -T` line again, and open one more SSH session with the machine
   key before you close the first.
3. Check the multiplexer: `command -v tmux` answers a path when it is there.
4. Install it when it is missing and the user can use sudo, with the install
   command of the provider reference. Never change another part of the machine
   and never install more than this.
5. Prove it: `command -v tmux` answers a path after the install.

When no try answers within 3 minutes, the card must not stay on `Starting`. Show
the heading with a `warn` badge `No SSH yet`, one `text` component with the last
error line that ssh printed, and the buttons `Try again` with tone `primary` and
`Delete` with tone `danger`. Do not rent a second machine for the same card.

### 5b. Register the machine

After step 5 proves that SSH answers and tmux is there, write the record of this
Horizon, so the person can connect without you:

```
operation:  machine_set
name:       "build-1"
ssh:        {"host": "<address>", "user": "<user from the reference>"}
key:        "build-1"
session:    "build-1"
provider:   "<Provider>, <region>"
```

The answer is `machine_set` with `created`, which is false when the name was
already there. `name` follows the machine key rule, `session` is 1 to 40
characters of `A-Z`, `a-z`, `0-9`, `_` and `-`, and `provider` is one line of at
most 60 characters. The record belongs to the person's Horizon and not to your
workspace, so every agent panel of this Horizon sees it and it survives a
restart. The same name then appears in the Machines section of the sidebar, and
a click on it opens the tmux session `build-1` with the machine key: the person
never needs your agent panel for that.

### 6. Show the machine card

Update the card. The heading is the name, the subtitle is the provider and the
region, and the badge is the state that the CLI reports: `Running` with `ok` when
the machine answers, `Starting` with `info`, `Stopped` with `warn`, and `danger`
with the provider's own words when a command failed. Give `kv` rows for Type,
Address, Session, SSH from, Rented at and Cost, draw the address and the session
name in `mono`, and give the cost as the provider's list price that you read.

The row `SSH from` says which rule is in force: `any address, machine key only`
for the default, or `<address of this computer>/32, machine key only` for a rule
that the person asked for. Then give
`Connect` with tone `primary`, `Put the Horizon server here`, `Add a browser`,
`Stop`, and `Delete` in `danger`. When the machine is stopped, draw `Start`
instead of `Stop`.

The two buttons that name Horizon hand the work to the skill `horizon-setup`,
with that goal and this machine as its place:

- `Put the Horizon server here` continues with the goal `server` on this machine.
- `Add a browser` continues with the goal `browser` on this machine.

Read that skill before you draw them, and let it write the cards of those flows.
Tell the person in a `text` line that the two buttons belong to Set up.

### 7. Listen

Run the listen loop with any-card `next_click`: leave out `panel_id`.

1. Call `next_click` with `timeout_millis: 300000`.
2. Read `component_id`, and `selected` when the click came from a row.
3. Do the action of that button, then send the result with `update`.
4. Call `next_click` again; on `timed_out`, call it again while the card matters.

A click with `stale: true` names an older document: read the state again, show
it, and let the person press again. A click on `cancel` or `keep` ends the step.

### 8. Connect

A press on `Connect` gives a terminal. Call `open_terminal` with the machine key,
the user from the provider reference and the session command:

```
operation: machine
machine:   "build-1"
title:     "build-1"
```

That is the record that `machine_set` wrote, so the panel it opens is the panel
that a click on the machine row opens. Pass no `ssh`, `key` or `remote_command`
with `machine`: the record holds them, and any of them is `invalid_request`.
Without a record, the long form is still there:

```
operation:      open_terminal
ssh:            {"host": "<address>", "user": "<user from the reference>"}
key:            "build-1"
remote_command: "tmux new -A -s build-1"
title:          "build-1"
```

Pass `ssh.port` only when the reference names a port, as `references/runpod.md`
does. In the long form the titlebar carries `opened by <agent>`, so the person
knows that you opened it. Step 5 proved that tmux is there and installed it when it was missing, so
pass the session command: Connect uses `remote_command`
`tmux new -A -s <name>`. Horizon adds `-t` itself. You cannot read, write or
close that terminal: the person owns it.

### 9. Stop and start

Use the stop and start command of the reference where the provider has one, and
read the state again afterwards. A stopped machine still costs money for its
disk, its static address and its other resources; only the delete ends the cost.
Say what still costs in the `text` component.

### 10. Delete

Delete needs a confirm step on the card before anything is deleted. Say on the
confirm card that a workspace the person put on this machine loses its terminals
there, and that their tmux sessions end with the machine.

1. A press on `Delete` updates the card to the confirm document.
2. A press on `Keep` returns to the machine card, and nothing is deleted.
3. A press on `Delete it` runs the provider delete in the order of the reference.
   A machine of a Tailscale network first loses its login there and then its
   device, as [tailscale](references/tailscale.md) says.
4. Prove that nothing tagged with this name is left, with the find-again command.
5. Call `machine_delete` with the machine name, so the record and its row in the
   sidebar go; an answer of `machine_unavailable` means that the record is
   already gone. Then call `delete_machine_key` with the machine name, which
   removes the key on every computer of the same server; an answer of
   `key_unavailable` means that the key is already gone.
6. Update the card to a `Deleted` badge with one `Close` button, or close it.
   Tell the person in the `text` component that the terminal that Connect opened
   keeps trying to reach the machine that is gone, and that they can close it.

A leftover resource means the delete did not finish. Show the provider's own
words on the card and keep `Delete it`, so the person can try again.

## A narrow SSH rule

The default rule accepts TCP 22 from every address and the key is the protection.
When the person asks for a rule that accepts SSH only from this computer, set its
source to the address of this computer as `/32`, and say on the card that another
network address and another computer cannot reach the machine after that.

```sh
MY_IP=$(curl -4 -s https://api.ipify.org)/32
```

| Provider | Command that narrows the rule of the rent | Command that opens it again |
|---|---|---|
| Hetzner | `hcloud firewall delete-rule --direction in --protocol tcp --port 22 --source-ips 0.0.0.0/0 $NAME-fw` and then `hcloud firewall add-rule --direction in --protocol tcp --port 22 --source-ips "$MY_IP" --description horizon-ssh $NAME-fw` | The add-rule line with `0.0.0.0/0` instead of `$MY_IP` |
| GCP | The `firewall-rules create` of `references/gcp.md` with `--source-ranges="$MY_IP"` | The same line with `--source-ranges=0.0.0.0/0` |
| AWS | `$AWS ec2 revoke-security-group-ingress --group-id "$SG" --protocol tcp --port 22 --cidr 0.0.0.0/0` and then the `authorize-security-group-ingress` of `references/aws.md` with `--cidr "$MY_IP"` | The authorize line with `--cidr 0.0.0.0/0` |
| Azure | The `nsg rule create` of `references/azure.md` with `--source-address-prefixes "$MY_IP"` | The same line with `--source-address-prefixes '*'` |
| RunPod | No firewall exists, and the published port cannot be narrowed | None |

None of those commands ran in this slice. Read the rule list of the provider
after the change, and let the person confirm the new login with the machine key
from this computer before the address changes.

## A machine on a Tailscale network

The person can ask to put a machine on one of their Tailscale networks: "put
`build-1` on my home network". The networks of this computer are the three named
secrets of each one, in the environment of your panel:
`TS_<NET>_CLIENT_ID`, `TS_<NET>_CLIENT_SECRET` and `TS_<NET>_TAG`. Read
[tailscale](references/tailscale.md) for the one-time setup of the person, the
commands, the proof, the delete and the relay recipes.

Three rules have no exception:

- **The client secret stays on this computer.** You make one API token here and
  one auth key for one machine. Never send the secret to a machine and never run
  `tailscale up --auth-key=<client secret>` there, although the Tailscale
  documents allow that form.
- **The auth key reaches the machine on standard input.** Pipe it into `ssh`: the
  remote script writes a root-only file under `/run`, `tailscale up` reads it
  with `--auth-key=file:<path>`, and the same command removes the file. No
  argument, no environment variable and no file of this computer holds the key.
- **Prove the login before port 22 closes.** A login over the network with the
  machine key answers first, and only then does the provider close inbound port
  22. When the proof fails, the port stays open and the card says why.

Step 5b writes the record with the network name as its `ssh.host`, so a click in
the Machines section connects over the network. The machine card of step 6 then
holds three more rows:

```json
{"type": "kv", "rows": [
  {"label": "Network", "value": "home", "mono": true},
  {"label": "Network name", "value": "build-1.tail1234.ts.net", "mono": true},
  {"label": "Internet SSH", "value": "closed"}]}
```

When the person asks for the internet through a home computer of the network, the
card also holds the row `Internet through` with `this machine` or the name of
that computer, and the buttons `Use <home name> as exit` and `Stop using exit`.
The reference gives the steps that belong to the person, the proof and the undo.

## Card documents

These are the values of `document` for `create` and `update`: the overview card,
the empty overview, the choose-provider card, the machine card, the confirm card
and the deleted card.

The overview: three machines at two providers, and GCP skipped in step 1.

```json
{"components": [
  {"type": "heading", "text": "Machines", "subtitle": "3 machines at 2 providers",
   "badge": {"text": "1 provider skipped", "tone": "warn"}},
  {"type": "list", "selection": "single", "rows": [
    {"id": "hetzner.build-1", "title": "build-1", "subtitle": "Hetzner, hel1, running", "selected": true},
    {"id": "hetzner.build-2", "title": "build-2", "subtitle": "Hetzner, hel1, stopped, still billed, no key here", "tone": "warn"},
    {"id": "runpod.gpu-1", "title": "gpu-1", "subtitle": "RunPod, EU-RO-1, running"}]},
  {"type": "text", "text": "GCP skipped: gcloud is not signed in."},
  {"type": "text", "text": "Read at 2026-11-10 20:04 UTC. Press Refresh to read it again."},
  {"type": "buttons", "items": [
    {"id": "connect", "label": "Connect", "tone": "primary"},
    {"id": "open_card", "label": "Open card"}, {"id": "refresh", "label": "Refresh"},
    {"id": "close", "label": "Close"}]}]}
```

The empty overview: no provider holds a machine.

```json
{"components": [
  {"type": "heading", "text": "Machines", "subtitle": "Nothing is rented"},
  {"type": "text", "text": "Every signed-in provider answered with an empty list for the tag horizon-managed=true."},
  {"type": "text", "text": "Read at 2026-11-10 20:11 UTC."},
  {"type": "buttons", "items": [
    {"id": "rent", "label": "Rent a machine", "tone": "primary"}, {"id": "close", "label": "Close"}]}]}
```

Choose the provider:

```json
{"components": [
  {"type": "heading", "text": "New machine", "subtitle": "Which cloud?"},
  {"type": "list", "selection": "single", "rows": [
    {"id": "provider.hetzner", "title": "Hetzner", "subtitle": "Signed in with hcloud",
     "badge": {"text": "Ready", "tone": "ok"}, "selected": true},
    {"id": "provider.azure", "title": "Azure", "subtitle": "Not signed in. Run az login", "disabled": true},
    {"id": "provider.gcp", "title": "Google Cloud", "subtitle": "Not signed in. Run gcloud auth login", "disabled": true},
    {"id": "provider.runpod", "title": "RunPod", "subtitle": "GPU machines. No RUNPOD_API_KEY is stored",
     "disabled": true},
    {"id": "provider.aws", "title": "AWS", "subtitle": "No profile found. Run aws configure sso", "disabled": true}]},
  {"type": "text", "text": "Hetzner can rent now. The other providers need the command of their line first."},
  {"type": "buttons", "items": [
    {"id": "continue", "label": "Continue", "tone": "primary"}, {"id": "cancel", "label": "Cancel"}]}]}
```

The machine runs. The person may connect, stop or delete it:

```json
{"components": [
  {"type": "heading", "text": "build-1", "subtitle": "Hetzner, hel1",
   "badge": {"text": "Running", "tone": "ok"}},
  {"type": "kv", "rows": [
    {"label": "Type", "value": "cpx02, 1 vCPU, 1 GB"},
    {"label": "Address", "value": "203.0.113.10", "mono": true},
    {"label": "Session", "value": "build-1", "mono": true},
    {"label": "SSH from", "value": "any address, machine key only"},
    {"label": "Rented at", "value": "2026-11-10 19:53 UTC"}]},
  {"type": "text", "text": "Connect gives a shell. Port 22 is open to every address and the machine key is the only login. The two buttons below belong to Set up."},
  {"type": "buttons", "items": [
    {"id": "connect", "label": "Connect", "tone": "primary"},
    {"id": "server_here", "label": "Put the Horizon server here"},
    {"id": "browser_here", "label": "Add a browser"},
    {"id": "stop", "label": "Stop"}, {"id": "delete", "label": "Delete", "tone": "danger"}]}]}
```

The person pressed `Delete`. Ask before you delete:

```json
{"components": [
  {"type": "heading", "text": "build-1", "subtitle": "Hetzner, hel1",
   "badge": {"text": "Delete?", "tone": "danger"}},
  {"type": "kv", "rows": [
    {"label": "Type", "value": "cpx02, 1 vCPU, 1 GB"},
    {"label": "Address", "value": "203.0.113.10", "mono": true}]},
  {"type": "text", "text": "Delete removes the machine and its disk. This cannot be undone."},
  {"type": "buttons", "items": [
    {"id": "delete_now", "label": "Delete it", "tone": "danger"}, {"id": "keep", "label": "Keep"}]}]}
```

After the delete and the key removal, update the card to this document:

```json
{"components": [
  {"type": "badge", "text": "Deleted", "tone": "ok"},
  {"type": "text", "text": "The machine and its key are gone. The terminal that Connect opened keeps trying to reach it; close it."},
  {"type": "buttons", "items": [{"id": "close", "label": "Close"}]}]}
```

## Rules

- A price on a card is the provider's value read in this run, in the row `value`
  of the machine type or in the `Cost` row of the machine card. Never write a
  price from memory or from a reference file, and write `price not read` when the
  CLI answers none. A price that you did not read is a false claim.
- Your own connections use the form of step 5, with the two paths that
  `machine_key` answered. Put no credential, token or private key text in a
  command line, a log or a card.
- Tell the person on the card what you do while you work: a `progress` component
  with a label, such as `Waiting for SSH`.
- Never show a credential: no token, no private key, no access key, no password,
  and no public key.
- Tag every resource that the provider can tag with `horizon-managed=true` and
  `horizon-card=<name>`. Tag disks and network objects too.
- Touch no resource without both tags and this machine name. Other machines in
  the account are not yours.
- One card per machine that you act on. An agent holds at most eight cards in one
  workspace; the overview makes that limit rarely matter, because the person
  reads every machine in one card.
- When a command fails, put the provider's own words in a `text` component and
  keep the buttons that still make sense. Do not translate an error into a guess,
  and do not retry a quota or capacity error in a loop.
- "What is rented", "show my machines" and "overview" mean the one overview card:
  one row per machine that the find-again commands return, one `text` line per
  provider that was skipped, no delete, and a `Refresh` press for a new read.
  Run that read again after every rent, stop, start and delete that you do.
- A new session adopts a machine through its tag and `machine_keys`. With a key
  of that name, connect as in step 8; without it, say on the overview and on the
  machine card that the key is gone, and offer `Delete` only.

## A machine without a browser

A rented Linux machine has no graphical session and usually no browser, so its
browser panels need a browser that Horizon can drive. When the person wants a
browser panel of such a machine, use [browser-machine](references/browser-machine.md):
one Chromium in a container on that machine, started by `setup.sh` with one
profile name, and reached by the ordinary browser host tunnel of Horizon. The
reference holds the four files, the steps, the proven lanes and the limits. Use
it only when the machine has no real Chrome or Chromium: a real browser is the
better browser host.

## Providers

| Provider | CLI | SSH user | sudo | Stop and start | Firewall | tmux on the default image | Reference |
|---|---|---|---|---|---|---|---|
| Hetzner | `hcloud` | `root` | not needed | yes | yes, one per machine | yes, `/usr/bin/tmux` | [hetzner](references/hetzner.md) |
| Azure | `az` | `azureuser` | yes | yes, with `deallocate` | yes, a security group | no | [azure](references/azure.md) |
| GCP | `gcloud` | the metadata user, for example `hz` | yes | yes | yes, one VPC rule | no | [gcp](references/gcp.md) |
| RunPod | `runpodctl` | `root` | not needed | yes, and it keeps billing | no, one public port | yes, its start command installs it | [runpod](references/runpod.md) |
| AWS | `aws` | `ubuntu` | yes | yes | yes, a security group | no | [aws](references/aws.md) |

A real run measured tmux on the Hetzner default Ubuntu 24.04 image, and
`/usr/bin/tmux` answered at once, so no install step is needed there. The other
providers were not measured in a rent run: keep the check of step 5 and install
tmux only when the check answers nothing. The RunPod start command installs it
itself. `references/aws.md` marks every create and delete `UNVERIFIED`.
