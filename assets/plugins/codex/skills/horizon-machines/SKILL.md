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
1 proves and that can rent; a row that cannot rent gets `tone: warn` and the
reason. Add `Continue` with tone `primary` and `Cancel`, then wait for a click.

Pick the machine name now: `[a-z0-9-]{1,40}`, with no leading or trailing `-`,
for example `build-1`. The name is the card title, the key name, the
`horizon-card` tag value and the session name. Reuse no name of a running machine.

### 3. Choose the region and the machine type

Ask in the same way on the same card: replace the list with the regions and keep
`Continue` and `Cancel`. Keep at most 8 rows, mark the sensible first choice
`selected: true`, and show no price. Then ask for the machine type, again with at
most 8 rows. The click carries the chosen rows in `selected`.

### 4. Make the key and rent

Call `machine_key` with the name before you rent. The answer is `{name,
public_key, private_key_path, known_hosts_path, created}`: Horizon keeps the key
files and returns the public key line with the two paths, never the text of the
private key. A later call returns the same answer with `created: false`. A name
outside the rules answers `invalid_key_name`; a full workspace answers
`too_many_keys`.

Then run the rent of the provider reference with that public key. Tag every
resource that the provider can tag with `horizon-managed=true` and
`horizon-card=<name>`. Put the name in the resource name when the provider has no
tag, as `references/gcp.md` and `references/runpod.md` say. Keep the returned ids
in your notes to delete later.

### 5. Wait for SSH and prepare the machine

A rent is not ready when the create command returns. Update the card to a
`progress` component that says what you wait for, such as
`{"type": "progress", "label": "Waiting for SSH", "percent": 10}`, then connect
with the machine key and the one form below. Use the `user` of the provider
reference; add `-p <port>` only where the reference names a port.

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
2. Check the multiplexer: `command -v tmux` answers a path when it is there.
3. Install it when it is missing and the user can use sudo, with the install
   command of the provider reference. Never change the ssh configuration of the
   machine and never install more than this.
4. Prove it: `command -v tmux` answers a path after the install.

When no try answers within 3 minutes, the card must not stay on `Starting`. Show
the heading with a `warn` badge `No SSH yet`, one `text` component with the last
error line that ssh printed, and the buttons `Try again` with tone `primary` and
`Delete` with tone `danger`. Do not rent a second machine for the same card.

### 6. Show the machine card

Update the card. The heading is the name, the subtitle is the provider and the
region, and the badge is the state that the CLI reports: `Running` with `ok` when
the machine answers, `Starting` with `info`, `Stopped` with `warn`, and `danger`
with the provider's own words when a command failed. Give `kv` rows for Type,
Address, Session and Rented at, and draw the address and the session name in
`mono`. Then give `Connect` with tone `primary`, `Stop`, and `Delete` in `danger`.
When the machine is stopped, draw `Start` instead of `Stop`.

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
operation:      open_terminal
ssh:            {"host": "<address>", "user": "<user from the reference>"}
key:            "build-1"
remote_command: "tmux new -A -s build-1"
title:          "build-1"
```

Pass `ssh.port` only when the reference names a port, as `references/runpod.md`
does. Step 5 proved that tmux is there and installed it when it was missing, so
pass the session command: Connect uses `remote_command`
`tmux new -A -s <name>`. Horizon adds `-t` itself. You cannot read, write or
close that terminal: the person owns it.

### 9. Stop and start

Use the stop and start command of the reference where the provider has one, and
read the state again afterwards. A stopped machine still costs money for its
disk, its static address and its other resources; only the delete ends the cost.
Say what still costs in the `text` component.

### 10. Delete

Delete needs a confirm step on the card before anything is deleted.

1. A press on `Delete` updates the card to the confirm document.
2. A press on `Keep` returns to the machine card, and nothing is deleted.
3. A press on `Delete it` runs the provider delete in the order of the reference.
4. Prove that nothing tagged with this name is left, with the find-again command.
5. Call `delete_machine_key` with the machine name; an answer of
   `key_unavailable` means that the key is already gone.
6. Update the card to a `Deleted` badge with one `Close` button, or close it.
   Tell the person in the `text` component that the terminal that Connect opened
   now shows ssh's timeout, and that they can close it.

A leftover resource means the delete did not finish. Show the provider's own
words on the card and keep `Delete it`, so the person can try again.

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
    {"id": "provider.hetzner", "title": "Hetzner", "subtitle": "hcloud 1.69.0, signed in", "selected": true},
    {"id": "provider.azure", "title": "Azure", "subtitle": "az is not installed", "tone": "warn"},
    {"id": "provider.gcp", "title": "GCP", "subtitle": "no active gcloud account", "tone": "warn"},
    {"id": "provider.runpod", "title": "RunPod", "subtitle": "runpodctl 2.14.0, signed in"},
    {"id": "provider.aws", "title": "AWS", "subtitle": "ec2:RunInstances is denied", "tone": "warn"}]},
  {"type": "text", "text": "Hetzner and RunPod can rent now. RunPod has a GPU box."},
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
    {"label": "Rented at", "value": "2026-11-10 19:53 UTC"}]},
  {"type": "text", "text": "Connect gives a shell. Port 22 is open to your address only."},
  {"type": "buttons", "items": [
    {"id": "connect", "label": "Connect", "tone": "primary"},
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
  {"type": "text", "text": "The machine and its key are gone. The terminal that Connect opened now shows ssh's timeout; close it."},
  {"type": "buttons", "items": [{"id": "close", "label": "Close"}]}]}
```

## Rules

- Show no price on a card. If the person asks for a price, answer in the terminal.
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
