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
- Horizon watches the agents that run on each machine of your list, with no
  panel open: the sidebar shows the agents of a machine with Working, Needs you
  or Done. An agent that you start in a `tmux` session of that user on the
  machine is seen, and it needs no Horizon process and no program of its own on
  that machine. The watch runs `tmux`, `ps` and the shell alone there, and
  `machines.watch_agents` turns it off. A row says `Needs you` from the title of a
  Codex pane, or from the state that the program of the pane reports itself in the
  `tmux` pane option `@horizon_state`: read
  [the exact state](#the-exact-state-of-an-agent-on-a-machine). A pane whose
  program is no agent tells its own state in the same option, and it is then a row
  of its machine. A press on the row of an agent opens the
  terminal of its session in the workspace of that machine, a chevron folds the
  agents of a machine, and a change that no window of Horizon shows sends one
  desktop notification with the name of the machine.
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
  Several provider images still allow a login with a password, so the rent turns
  password login off and proves a fresh login with the machine key, in step 2 of
  step 5. It asks the person nothing, because the rent card says what the machine
  gets, and it asks only when that proof fails.
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

A rent is not ready when the create command returns. Update the card to the step
list of this work, with the time of each finished step and `now` for the step
that runs, as the card of `horizon-setup` draws it:

```json
{"components": [
  {"type": "heading", "text": "Renting the machine", "subtitle": "Small, Helsinki, Hetzner.",
   "badge": {"text": "Working", "tone": "info"}},
  {"type": "progress", "label": "Rent and prepare", "percent": 40, "detail": "2 of 6"},
  {"type": "steps", "steps": [
    {"label": "Make a machine key for Horizon", "state": "done", "at": "2 s"},
    {"label": "Create the machine at the provider", "state": "done", "at": "11 s"},
    {"label": "Wait for SSH with the machine key", "state": "now", "at": "now"},
    {"label": "Turn off password login"},
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
2. Turn off password login, and prove a fresh login with the machine key. This is
   part of the rent: the rent card says in one line
   `SSH: the machine key only; password login is turned off`, so ask the person
   nothing here. The first login of step 1 proved the key, so the machine
   accepts it, and the new login below proves that it still does after the
   change. Read what the ssh server accepts:

   ```
   sudo sshd -T | grep -Ei '^(passwordauthentication|kbdinteractiveauthentication|permitrootlogin)'
   ```

   > CAUTION: `sshd -T` and the next command change who can log in. A mistake
   > here closes the login of the machine. Keep the session of step 1 open until
   > the new login answered.

   ```
   printf 'PasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitRootLogin prohibit-password\n' \
     | sudo tee /etc/ssh/sshd_config.d/10-horizon.conf
   sudo sshd -t && sudo systemctl reload ssh 2>/dev/null || sudo systemctl reload sshd
   ```

   Then prove it, in this order: run the `sshd -T` line again and read
   `passwordauthentication no` and `kbdinteractiveauthentication no`, and open
   one more session with the machine key with the form of step 1 and
   `true` as `<command>`. Close the session of step 1 only after that answer,
   and mark the step `done` with its time on the card.

   When the write, the reload, the read or the fresh login fails, the card says
   what happened and asks: the heading says that the machine accepts a password,
   one `text` line holds the last line of the command that failed, and the card
   holds `Try again` with tone `primary` and `Delete the machine` with tone
   `danger`. `Try again` repeats this step only, on the same machine. Never
   leave the machine in this state without a card, and never leave port 22 open
   on a machine that accepts a password.
3. Check the multiplexer: `command -v tmux` answers a path when it is there.
4. Install it when it is missing and the user can use sudo, with the install
   command of the provider reference. Never change another part of the machine
   and never install more than this.
5. Prove it: `command -v tmux` answers a path after the install.

When no try answers within 3 minutes, the card must not stay on `Starting`. Show
the step list with the step of the first login `failed`, the heading with a
`warn` badge `No SSH yet`, one `text` component with the last error line that
ssh printed, and the buttons `Try again` with tone `primary` and `Delete` with
tone `danger`. Do not rent a second machine for the same card.

```json
{"components": [
  {"type": "heading", "text": "Renting the machine", "subtitle": "Small, Helsinki, Hetzner.",
   "badge": {"text": "No SSH yet", "tone": "warn"}},
  {"type": "progress", "label": "Rent and prepare", "percent": 40, "detail": "2 of 6"},
  {"type": "steps", "steps": [
    {"label": "Make a machine key for Horizon", "state": "done", "at": "2 s"},
    {"label": "Create the machine at the provider", "state": "done", "at": "11 s"},
    {"label": "Wait for SSH with the machine key", "state": "failed", "at": "3 m"},
    {"label": "Turn off password login"},
    {"label": "Check tmux on the machine"},
    {"label": "Add the machine to Machines"}]},
  {"type": "text", "text": "Connection timed out during banner exchange"},
  {"type": "buttons", "items": [
    {"id": "try_again", "label": "Try again", "tone": "primary"},
    {"id": "delete", "label": "Delete the machine", "tone": "danger"}]}]}
```

`Try again` runs the wait of step 1 again, on the same machine.

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
repository: "<owner>/<repository>"
```

The answer is `machine_set` with `created`, which is false when the name was
already there. `name` follows the machine key rule, `session` is 1 to 40
characters of `A-Z`, `a-z`, `0-9`, `_` and `-`, and `provider` and `repository`
are one line of at most 60 characters each. `repository` is the repository that
the machine is ready for, and Horizon draws it on the second line of the row of
the machine in the sidebar. Leave it out while the machine is ready for no
repository, and read `references/repository-machine.md` for a machine of a
repository. The record belongs to the person's Horizon and not to your
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
`Join a network`, `Exact agent state`, `Stop`, and `Delete` in `danger`. When the
machine is stopped, draw `Start` instead of `Stop`.

`Join a network` draws the network card of
[tailscale](references/tailscale.md). The card holds the stored networks as pills
and no field for a secret. A machine that is already in a network shows its rows
on the machine card and the same button says which network it joined.

`Exact agent state` opens the card of
[the exact state](#the-exact-state-of-an-agent-on-a-machine): it writes the hook
entries of Claude Code into the settings of the machine, so the row of an agent
says its exact state.

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

Two settings of this computer can name the machine that the person deletes, and
each one breaks something that no card of this delete shows afterwards:

- `browser_host_status` answers the browser host of the new browser panels. When
  its `machine` is the name of this machine, every browser panel that a create
  makes after the delete opens nothing, because its host is gone.
- `server_status` answers how this computer reaches its Horizon server. When its
  `machine` is the name of this machine, the server runs on the machine that is
  deleted, so this computer loses its link: its machine list, its keys and its
  settings stay on this computer alone, and the other computers of the person
  follow a server that is gone.

Read both answers before you draw the confirm card, and say on it what the delete
takes with it. Name the machine in the words of the card, and never guess: a
field that the answer does not hold names no machine.

1. A press on `Delete` updates the card to the confirm document, with the two
   sentences of the answers that name this machine.
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
6. Update the card to a `Deleted` badge, the line of the browser host and the
   line of the server when they named this machine, and the button `Close`. Put
   the button `Use this computer again` before `Close` only when the browser host
   named this machine. For the server, the line of the card is the whole answer:
   it has no button. Tell the person in the `text` component
   that the terminal that Connect opened keeps trying to reach the machine that
   is gone, and that they can close it.
7. A press on `Use this computer again` says where the person clears the browser
   host entry: **Settings**, **General**, **Browser host**, take the entry away.
   `browser_host_request` writes a host and never clears one, so the person's save
   in the Settings window is the confirmed operation that moves the new browser
   panels back to the browser of this computer, as the ready card of the goal
   `browser` of `horizon-setup` says. Say one line in the panel with the same
   words. For the server, the person writes the new place in **Settings**,
   **Server**, or joins another server, as `horizon-server` says.

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
`build-1` on my home network". A machine joins one network. The person can store
several networks, and each one is a choice on the rent card. A network is stored
when its three named secrets are there: `TS_<NET>_CLIENT_ID`,
`TS_<NET>_CLIENT_SECRET` and `TS_<NET>_TAG`. Read the names with `block_panel`
`secret_names`, which answers the names that a panel of this computer gets now,
and never read a value. Read [tailscale](references/tailscale.md) for the
one-time setup of the person, the commands, the cards, the proof, the delete and
the relay recipes.

When two networks must reach one machine, the person shares the device with the
other network. That share is in the Tailscale admin console, on the page of the
device. You share no device. A machine carries the client of one network only:
never install a second client and never join a machine that is already in a
network to another one.

Four rules have no exception:

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
- **The agent reads the state of this computer and changes nothing there.** The
  computer of the person can be in a network of the person's work, so you never
  run `tailscale up`, `down`, `login`, `logout`, `switch` or `set` on that
  computer. Read the state with `tailscale status --json` only.

The key follows the scope of the client. Read `scope` of the token answer before
you make the key:

- A client with a devices scope makes the key and removes the device through the
  API, as [tailscale](references/tailscale.md) says today.
- A client with the scope `auth_keys` only makes the key of a rented machine with
  `ephemeral: true`. The node leaves the network by itself after the machine is
  gone, and the card says that. A key that is not ephemeral is made with such a
  client only when the person asks for a machine that stays. The card says then
  that the person removes the node in the admin console.
- `Delete` runs `tailscale logout` on the machine before it removes the machine,
  and the card says which node stays in the console, if any.

Never ask the person for a client with a wider scope because a key or a delete is
easier with it. The page gives the answer of each call and the words of the card.

The computer of the person reads its own state and changes nothing. Read
`tailscale status --json` when the program is there, and compare the suffix of
MagicDNS with the suffix of the machine. That is how you learn whether this
computer is in the same network. A command that changes the state of this
computer moves it out of a network of the person's work.

- Same network: prove the login with the machine key over the network name, and
  then offer `Close port 22`, as section 9 of the page says.
- Another network or no network: keep the public SSH rule with the machine key and
  write on the card `This computer is not in the network <NET>; the machine is
  reached over its public address`. Offer no close.

A machine that must reach devices of the network needs two rules of policy, and
both belong to the person: a network rule for `tcp:22`, and an `ssh` rule with
`"action": "accept"` for the user name on the devices. A `check` rule never serves
a machine (doc). Section 1.1.1 of [tailscale](references/tailscale.md) gives the
block. Propose a tag for the purpose and give the person that text. The person
adds it in the admin console. Never write the policy file and never ask for a
client that can.

Step 5b writes the record with the network name as its `ssh.host`, so a click in
the Machines section connects over the network. The machine card of step 6 then
holds three more rows, and the button `Join a network`:

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

## A machine for a repository

A repository says in its own `AGENTS.md`, in the section `## Horizon machine`,
what a machine for its work needs: the resources, the system packages, the tools
with their versions, the warm-up commands, the check that proves the machine and
the way that the repository arrives. Use
[repository-machine](references/repository-machine.md) when the person asks for a
machine for a repository, or when a plan needs a place that is `rent a machine`.

That page gives the recipe, the card of the rent with the price of every machine
of the run together, one steps card for each machine, the step that writes the
trust of the folder of the repository for each agent program that the machine
has, the card that says `Ready for <repository>` with the check as evidence, the
card of a step that failed, and the rule that the repository arrives as a `git
bundle` unless the recipe asks for a clone with a credential that the person
confirms. When the repository holds no such section, ask one question on a card
and offer to write the section from what the repository says about its own
machine.

## The logins and the private files of a repository on a machine

A machine that works on a repository needs the logins of the person and the files
that Git does not hold. The person makes every login, and the person confirms
every copy.

### The logins

The person signs in, in the terminal of the machine. Each command prints a code or
an address for the browser of the person, and the login stays on the machine:

| Program | The command on the machine | What the person does |
|---|---|---|
| Claude Code | `claude` | Press `/login` and open the address that it prints. |
| Codex | `codex login` | Open the address that it prints, or type its code. |
| GitHub | `gh auth login` | Type the code that it prints in the browser. |

Tell the person the command of their program, and open the terminal of the
machine with `open_terminal` and the machine key when the person wants it from
Horizon. Run no login command yourself: it waits for the person.

Do not copy a login file, a token, a keyring, a cookie or `~/.ssh` of this
computer to a machine. A login of this computer is a credential of this computer,
and its copy on a machine is a second credential that nobody watches. A key of a
machine is its machine key from `machine_key`, and a repository that needs an SSH
key of the machine gets a key that is made on the machine, in a step that says
so.

### The private files

A folder of vendor files, a `.env` file and a package cache are not in Git. Ask
the person for each source path, and read the size of each path first. Show one
card before you copy anything:

- One `kv` row for each file: the `label` is the source path, and the `value` is
  the destination and the size.
- One `text` line says that the copy leaves this computer and stays on the
  machine.
- One `text` line names each file that holds a secret, and says that the file gets
  the mode 600 on the machine.

```json
{"components": [
  {"type": "heading", "text": "Files for the machine", "subtitle": "build-1"},
  {"type": "kv", "rows": [
    {"label": "vendor/", "value": "build-1:/srv/shop/vendor, 18 MiB"},
    {"label": ".env", "value": "build-1:/srv/shop/.env, 4 KiB, secret, mode 600"}]},
  {"type": "text", "text": "This copy leaves this computer and stays on the machine."},
  {"type": "buttons", "items": [
    {"id": "copy", "label": "Copy the files", "tone": "primary"}, {"id": "cancel", "label": "Cancel"}]}]}
```

> CAUTION: THIS STEP SENDS FILES OF THIS COMPUTER TO THE MACHINE. A file that holds
> a secret is readable on the machine by the user of the login and by everybody who
> takes the machine over.

After the press, copy each path, and set the mode of each secret file:

```sh
rsync -a --info=progress2 \
  -e "ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
      -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
      -o BatchMode=yes -o ConnectTimeout=10" \
  <source path> <user>@<address>:<destination path>
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
  -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
  -o BatchMode=yes -o ConnectTimeout=10 \
  <user>@<address> 'chmod 600 <destination path of a secret>'
```

The two paths are the fields of the `machine_key` answer, and the options are the
form of step 5 of this skill. Read the mode back with `ls -l`. The card then names
each file that arrived, and it holds the words of a command that failed. A path
that is a folder of a package cache, such as `node_modules`, is rebuilt on the
machine when the machine has the network, so ask the person before you copy one.

## A command at a time

A job that must start at a time of the night must not end with your session, and
it needs no extra program: one `tmux` session on the machine waits and then runs
the command. The session keeps running after the connection of the person ends and
after your session ends.

1. Read the time zone of the machine and the seconds until the start:

   ```sh
   ssh <options of the machine key> <user>@<address> \
     'date +"%Z %z"; echo $(( $(date -d "2026-11-16 02:00" +%s) - $(date +%s) ))'
   ```

   The `date` of macOS has no `-d`, so on such a computer its own option computes
   the seconds, or the person gives them.

2. Show one card with the command, the start, the seconds and the time zone of the
   machine:

   ```json
   {"components": [
     {"type": "heading", "text": "A command at a time", "subtitle": "build-1"},
     {"type": "kv", "rows": [
       {"label": "Start", "value": "2026-11-16 02:00 CET (+0100) on the machine"},
       {"label": "Waits", "value": "26100 s"},
       {"label": "Command", "value": "./upgrade-devices.sh", "mono": true},
       {"label": "Session", "value": "night-0200", "mono": true}]},
     {"type": "text", "text": "A restart of the machine ends the wait and the job, and nothing starts it again."},
     {"type": "buttons", "items": [
       {"id": "start", "label": "Start the wait", "tone": "primary"}, {"id": "cancel", "label": "Cancel"}]}]}
   ```

3. Start the session after the press. The body waits, reports its state in the
   pane option, runs the command, reports the answer of the command, and keeps
   the shell for the person:

   ```sh
   ssh <options of the machine key> <user>@<address> sh <<'SH'
   body='sleep 26100; tmux set-option -p @horizon_state working; ./upgrade-devices.sh; if [ $? -eq 0 ]; then tmux set-option -p @horizon_state done; else tmux set-option -p @horizon_state blocked; fi; exec "$SHELL"'
   tmux new-session -d -s night-0200 -c /srv/shop "$body"
   SH
   ```

   `tmux` runs the command of a new session with the shell of the machine itself,
   so the body is one argument and no `sh -c` is needed around it. A body with
   `sh -c` in front of it would run every word of the body after the first one in
   a second shell, and the wait and the command would come apart.

4. Read the row of the session in the sidebar of Horizon: the session is a row of
   its machine, because its program reports a state in the pane option. It says
   `Working` while the command runs, `Done` when it ended well and `Needs you`
   when it failed, and a press on the row opens its terminal. `tmux ls` on the
   machine names the same session.

A restart of the machine ends the wait and the job. `tmux` is no service, so
nothing starts the session again, and a job that must survive a restart is a
systemd timer of the machine, which is another recipe.

## The exact state of an agent on a machine

Horizon shows the state of an agent of a machine from two signals: the title rule
of the kind, which only Codex has, and one working line of the screen. A Claude
Code agent has no such title, so its row says `Working` or nothing, and it says
`Needs you` never. The exact state comes from the program of the pane itself: the
hook of the agent writes one word into the `tmux` pane option `@horizon_state`,
and the watch of Horizon reads that option.

Codex needs nothing, because its title is exact. Pi and the other programs speak
the same contract as the hook of Claude Code, in two lines:

- The program runs `tmux set-option -p -t "$TMUX_PANE" @horizon_state <word>` with
  `working`, `blocked`, `done` or `idle`, and `tmux set-option -p -u -t
  "$TMUX_PANE" @horizon_state` when it takes the state back.
- The word is exact and holds no other text. A pane that reports `working` shows
  `Working`, `blocked` shows `Needs you`, and `done` shows `Done` until the person
  looks at it.

The question of a first start in a folder, whether the person trusts that folder,
is no report, so a row of a machine says `Needs you` never for the question alone.
The step `Trust the repository folder` of [a machine for a
repository](#a-machine-for-a-repository) answers the question for the folder of
the rent, so no agent of that machine stops at it.

The person asks for this with the button `Exact agent state` on the machine card,
or with the words "the exact state of my agents". The button stands after `Join a
network` and before `Stop`, and it belongs to the work that prepares a machine.

### 1. Show the card of the change

Step 2 reads the file on the machine. The card says what the change does and
what it keeps:

- The heading is `Exact agent state` and the subtitle is the name of the machine.
- One `text` line names the file: `~/.claude/settings.json` of the SSH user of the
  machine.
- One `text` line says what the change does: nine hook entries of Claude Code run
  one `tmux` command each, and Horizon then reads the exact state of the agent
  from the pane option `@horizon_state`.
- One `text` line says what the change does not do: it puts no program of Horizon
  on the machine, it opens no network connection, and it changes no file but the
  settings of the agent.
- One `text` line says what the change keeps: every other key and every other hook
  of the file, and one backup of the file with the name
  `settings.json.horizon-bak` beside it.
- One `text` line names the limit of the change: after the person allows a tool,
  the row says `Needs you` until that tool ends, because Claude Code has no event
  for "allowed". A job that an agent starts in the background has no such limit.
- The buttons are `Set it up` with tone `primary` and `Cancel` when the file holds
  no entry of Horizon, and `Remove` with tone `primary` and `Cancel` when it does.
  A machine whose file already holds all nine entries says so in one `text` line
  and offers `Remove` only.

```json
{"components": [
  {"type": "heading", "text": "Exact agent state", "subtitle": "build-1"},
  {"type": "kv", "rows": [
    {"label": "File", "value": "~/.claude/settings.json", "mono": true},
    {"label": "Backup", "value": "settings.json.horizon-bak", "mono": true},
    {"label": "Entries", "value": "9 hooks of Claude Code"}]},
  {"type": "text", "text": "Each entry runs one tmux command in the pane of the agent. Horizon reads the word that it writes there, so the row says the exact state of the agent."},
  {"type": "text", "text": "This puts no program of Horizon on the machine, opens no network connection and changes no other file. The program reads the file first and keeps every other key and every other hook."},
  {"type": "text", "text": "After the person allows a tool, the row says Needs you until that tool ends: Claude Code has no event for an allowed tool."},
  {"type": "buttons", "items": [
    {"id": "set_up", "label": "Set it up", "tone": "primary"}, {"id": "cancel", "label": "Cancel"}]}]}
```

Wait for a press. `Cancel` ends the step, and `Set it up` continues with step 2.

### 2. Read the settings of the agent

Use the form of step 5 of this skill with the two paths that `machine_key`
answered, and read the file and the two tools of the machine:

```sh
ssh <options of the machine key> <user>@<address> \
  'ls -l ~/.claude/settings.json; command -v python3; command -v jq'
```

A line for `python3` or for `jq` is the answer that the machine has that tool, and
no line is the answer that it has none. An `ls` that names no file is the answer
that the file does not exist. The card keeps the account of those answers.

### 3. Write the entries

This is the block of the change, complete. It is the words of the turn ledger of
Horizon: a session start is `idle`, a prompt is `working`, a permission request
and a question of the agent are `blocked`, the end of a tool and of an answer are
`working`, the end of a turn is `done`, and the end of the session takes the
report back.

```json
{
  "hooks": {
    "SessionStart": [{"hooks": [{"type": "command", "command": "[ -z \"$TMUX_PANE\" ] || tmux set-option -p -t \"$TMUX_PANE\" @horizon_state idle"}]}],
    "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "[ -z \"$TMUX_PANE\" ] || tmux set-option -p -t \"$TMUX_PANE\" @horizon_state working"}]}],
    "PreToolUse": [{"matcher": "AskUserQuestion", "hooks": [{"type": "command", "command": "[ -z \"$TMUX_PANE\" ] || tmux set-option -p -t \"$TMUX_PANE\" @horizon_state blocked"}]}],
    "PermissionRequest": [{"hooks": [{"type": "command", "command": "[ -z \"$TMUX_PANE\" ] || tmux set-option -p -t \"$TMUX_PANE\" @horizon_state blocked"}]}],
    "Elicitation": [{"hooks": [{"type": "command", "command": "[ -z \"$TMUX_PANE\" ] || tmux set-option -p -t \"$TMUX_PANE\" @horizon_state blocked"}]}],
    "PostToolUse": [{"hooks": [{"type": "command", "command": "[ -z \"$TMUX_PANE\" ] || tmux set-option -p -t \"$TMUX_PANE\" @horizon_state working"}]}],
    "ElicitationResult": [{"hooks": [{"type": "command", "command": "[ -z \"$TMUX_PANE\" ] || tmux set-option -p -t \"$TMUX_PANE\" @horizon_state working"}]}],
    "Stop": [{"hooks": [{"type": "command", "command": "[ -z \"$TMUX_PANE\" ] || tmux set-option -p -t \"$TMUX_PANE\" @horizon_state done"}]}],
    "SessionEnd": [{"hooks": [{"type": "command", "command": "[ -z \"$TMUX_PANE\" ] || tmux set-option -p -u -t \"$TMUX_PANE\" @horizon_state"}]}]
  }
}
```

The command of every hook is one `sh` line. The test at its start keeps a command
that runs outside `tmux` from failing: without `$TMUX_PANE` there is no pane to
report. No entry names a path of this computer, an address, a token or a program
of Horizon.

Three cases follow the answer of step 2. Put the block in one shell word for the
program, with no newline in it:

```sh
BLOCK='<the block of this step, in one line>'
```

- **The file does not exist.** Write it with the block, and make the folder:

  ```sh
  ssh <options of the machine key> <user>@<address> \
    'mkdir -p "$HOME/.claude" && cat > "$HOME/.claude/settings.json"' <<'JSON'
  <the block of this step, with its indentation>
  JSON
  ```

- **The file exists and the machine has `python3` or `jq`.** Make one backup, and
  then merge the block into the file with one program on the machine. Copy the
  file first: `cp -p ~/.claude/settings.json ~/.claude/settings.json.horizon-bak`,
  and keep a backup that is already there. The program below writes the whole file
  again with two spaces of indentation, so the text of the file changes and every
  key stays:

  ```sh
  ssh <options of the machine key> <user>@<address> python3 - set "$BLOCK" <<'PY'
  import json, pathlib, sys

  mode, block = sys.argv[1], json.loads(sys.argv[2])
  path = pathlib.Path.home() / ".claude" / "settings.json"
  settings = json.loads(path.read_text()) if path.exists() else {}
  hooks = settings.setdefault("hooks", {})
  for event, entries in block.items():
      held = hooks.setdefault(event, [])
      if mode == "set":
          held.extend(entry for entry in entries if entry not in held)
      else:
          for entry in entries:
              while entry in held:
                  held.remove(entry)
      if not held:
          del hooks[event]
  if not hooks:
      settings.pop("hooks", None)
  path.parent.mkdir(mode=0o700, exist_ok=True)
  path.write_text(json.dumps(settings, indent=2) + "\n")
  print(path)
  PY
  ```

  A machine with `jq` and no `python3` merges the block with
  `jq --argjson block "$BLOCK" '.hooks = ((.hooks // {}) | . * $block)'` only when
  the two files hold no entry of Horizon. Prefer `python3`, and use `jq` only when
  the person asks for it.
- **The file exists and the machine has neither tool.** Change nothing. Show the
  block of this step in one `text` component, or in a second card, for the person
  to add by hand, and say that Horizon changes no file of this machine.

### 4. Say what happened

Update the card; do not close it until the person reads it.

- One `text` line names the file that changed and its backup, for example
  `Wrote ~/.claude/settings.json; the old file is settings.json.horizon-bak.`
- One `text` line says when the state shows: the next report of the watch, about
  two seconds after the next hook of the agent.
- One `text` line says what to do when the row stays wrong: read the pane option
  of the session by hand with `tmux show-options -p -t <pane> @horizon_state`, and
  read the log of the hook of the agent.
- The button is `Close`.

Do not claim a state that you did not see. A row that you did not read after the
change is `not proved`.

### 5. Remove the entries

`Remove` runs the same program with the word `remove`:

```sh
ssh <options of the machine key> <user>@<address> python3 - remove "$BLOCK" <<'PY'
<the program of step 3>
PY
```

It takes out exactly the entries of this section and keeps every other key and
hook. It keeps the backup, and it says on the card that the backup stays. A file
that held no other hook loses its `hooks` key.

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
    {"id": "join_network", "label": "Join a network"},
    {"id": "exact_state", "label": "Exact agent state"},
    {"id": "stop", "label": "Stop"}, {"id": "delete", "label": "Delete", "tone": "danger"}]}]}
```

The person pressed `Delete`. Ask before you delete, and say on this card what the
delete takes with it:

```json
{"components": [
  {"type": "heading", "text": "build-1", "subtitle": "Hetzner, hel1",
   "badge": {"text": "Delete?", "tone": "danger"}},
  {"type": "kv", "rows": [
    {"label": "Type", "value": "cpx02, 1 vCPU, 1 GB"},
    {"label": "Address", "value": "203.0.113.10", "mono": true}]},
  {"type": "text", "text": "Delete removes the machine and its disk. This cannot be undone."},
  {"type": "text", "text": "New browser panels follow the browser on build-1. After the delete they open nothing until this computer uses its own browser again."},
  {"type": "text", "text": "This computer follows the Horizon server on build-1. Delete takes the server with the machine, so this computer loses its link."},
  {"type": "buttons", "items": [
    {"id": "delete_now", "label": "Delete it", "tone": "danger"}, {"id": "keep", "label": "Keep"}]}]}
```

Draw the line of the browser host only when `browser_host_status` names this
machine in `machine`, and the line of the server only when `server_status` does. A
machine that names neither setting gets neither line.

After the delete and the key removal, update the same card to this document:

```json
{"components": [
  {"type": "heading", "text": "build-1 is deleted",
   "badge": {"text": "Deleted", "tone": "ok"}},
  {"type": "text", "text": "The machine and its key are gone. The terminal that Connect opened keeps trying to reach it; close it."},
  {"type": "text", "text": "New browser panels open nothing until this computer uses its own browser again: Settings, General, Browser host, take the entry away."},
  {"type": "text", "text": "This computer follows no server now. Settings, Server, takes the place of another server."},
  {"type": "buttons", "items": [
    {"id": "use_this_computer", "label": "Use this computer again"},
    {"id": "close", "label": "Close"}]}]}
```

This document is of a machine that both settings named. Leave out the line of a
setting that did not name the machine, and leave out the button
`Use this computer again` when the browser host did not name it.

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
