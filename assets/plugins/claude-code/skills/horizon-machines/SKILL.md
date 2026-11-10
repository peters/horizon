---
name: horizon-machines
description: >-
  Rent, connect, stop and delete a cloud machine for the person from a Horizon card, with the
  provider CLI in your own shell. Use it when the person asks for a machine, a cloud, a worker, a
  GPU box, to rent, stop or delete one, or to see what is rented. The card uses block_panel
  (horizon-block) and Connect uses open_terminal with a Horizon machine key.
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

## The flow

Do the steps in this order. Stop and tell the person when a step fails.

### 1. Find the CLIs

Run one read-only command per provider. Run no create command yet.

- Hetzner: `hcloud server list --quiet`
- Azure: `az account show -o json --query "{name:name,id:id,state:state}"`
- GCP: `gcloud auth list --filter=status:ACTIVE --format="value(account)"`
- RunPod: `runpodctl user -o json`
- AWS: `aws sts get-caller-identity`, then the `--dry-run` preflight of `references/aws.md`

A missing CLI and a CLI without a login are both `warn` rows, with the provider's
own words in the subtitle. Tell the person which command to run, and wait.

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

1. Wait until SSH answers: run the form with `true` as `<command>` every 10
   seconds, for at most 3 minutes. Update the card between the tries, and never
   show `Connect` before a try answers.
2. Check the multiplexer: `command -v tmux` answers a path when it is there.
3. Install it when it is missing and the user can use sudo, with the install
   command of the provider reference. Never change the ssh configuration of the
   machine and never install more than this.
4. Prove it: `command -v tmux` answers a path after the install.

A machine that does not answer within 3 minutes is a `warn` row with the exit
code of the last try, and `Delete` stays the only offer. Do not rent a second
machine for the same card.

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
5. Call `delete_machine_key` with the machine name.
6. Update the card to a `Deleted` badge with one `Close` button, or close it.

A leftover resource means the delete did not finish. Show the provider's own
words on the card and keep `Delete it`, so the person can try again.

## Card documents

These are the values of `document` for `create` and `update`: the
choose-provider card, the machine card and the confirm card.

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

After the delete and the key removal, update the card to one `badge` with the
text `Deleted` and tone `ok`, and one `buttons` with a `close` item.

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
- One card per machine. An agent holds at most eight cards in one workspace.
- When a command fails, put the provider's own words in a `text` component and
  keep the buttons that still make sense. Do not translate an error into a guess,
  and do not retry a quota or capacity error in a loop.
- "What is rented" means the find-again command of every provider that step 1
  proves, and one card per machine that it finds: tag in heading, provider and
  region in subtitle, state in badge.
- A new session adopts a machine through its tag and `machine_keys`. With a key
  of that name, connect as in step 8; without it, say that the key is gone and
  offer `Delete` only.

## Providers

| Provider | CLI | SSH user | sudo | Stop and start | Firewall | tmux on the default image | Reference |
|---|---|---|---|---|---|---|---|
| Hetzner | `hcloud` | `root` | not needed | yes | yes, one per machine | no | [hetzner](references/hetzner.md) |
| Azure | `az` | `azureuser` | yes | yes, with `deallocate` | yes, a security group | no | [azure](references/azure.md) |
| GCP | `gcloud` | the metadata user, for example `hz` | yes | yes | yes, one VPC rule | no | [gcp](references/gcp.md) |
| RunPod | `runpodctl` | `root` | not needed | yes, and it keeps billing | no, one public port | yes, its start command installs it | [runpod](references/runpod.md) |
| AWS | `aws` | `ubuntu` | yes | yes | yes, a security group | no | [aws](references/aws.md) |

No rent test measured tmux on any default image, so you check it in step 5 and
install it when it is missing; the RunPod start command installs it itself.
`references/aws.md` marks every create and delete `UNVERIFIED`.
