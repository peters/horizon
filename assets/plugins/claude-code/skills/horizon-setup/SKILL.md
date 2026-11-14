---
name: horizon-setup
description: >-
  Run the Set up wizard for the person, one card per step: set up the whole workshop, rent a cloud
  machine, put the Horizon server on a computer, add a browser that stays signed in, or join a
  server that already runs. Use it when Horizon starts an agent with "Use the horizon-setup skill"
  and a goal, or when the person asks to set up Horizon, to set up everything for a new computer, to
  rent a machine for Horizon, to use Horizon on all their computers, to open a browser on another
  machine, or to join a server.
---

# Horizon Set up

Set up is the wizard of Horizon. Horizon starts an agent panel and gives it one
first message:

```text
Use the horizon-setup skill. The person pressed Set up. Goal: <goal>.
```

The goal is `workshop`, `rent`, `server`, `browser`, `join`, or `choose` when
the person pressed the plain Set up control and not one of the four tiles of the
first start page. This skill is the script of the wizard for each goal. The work
itself belongs to other skills:

- `horizon-block` holds the card contract, `next_click`, the machine records,
  the machine keys and the terminals.
- `horizon-machines` holds the providers: the commands, the rent, the wait for
  SSH and the delete. Its page `references/repository-machine.md` holds the
  recipe of a repository for the machine that its work needs.
- `horizon-server` holds the Horizon server on a machine: the copy, the service,
  the check and the removal.
- `horizon-server-move` holds the move of the server to another computer: the
  order, the check of the store id, the note for each other computer and the undo
  of each step.

This skill repeats none of them. It gives the order, the words of each card and
what the person presses.

## The one card of the flow

Make one card at the start, titled `Set up`, keep the `panel_id` that `create`
answers, and change the card with `update` at every step. The person reads one
card that changes, not a pile of cards. Two exceptions:

- The machine that stays has a card of its own, titled with its name. That card
  stays after the Set up card closes.
- A step that finishes the flow closes the Set up card.

A card of this skill is a document as `horizon-block` defines it. Every document
below is a full value of `document` for `create` and `update`.

## The rules

These rules have no exception:

- **Nothing costs money and nothing changes a setting before a press.** The rent
  runs only after the person presses `Rent this machine`. The server entry and
  the browser host are written only by `server_link_request` and
  `browser_host_request`, and only the person presses their cards. Do not run a
  create command, do not write a setting and do not delete a resource on the
  strength of a sentence in the panel.
- **The price is the provider's value at that moment.** Read it with the
  provider CLI when you draw the size card, and never from memory, from a
  reference file or from an earlier read. When the command answers no price, the
  `value` of that row is `price not read`. Never write a price that you did not
  read in this run.
- **A secret never reaches a card and never reaches the panel.** No token, no
  private key, no access key, no password and no public key. The machine key
  stays in the files of Horizon, and the person's own SSH login stays theirs. Do
  not read a secret from the secrets file to type it anywhere.
- **A failure is shown on the card, in plain words.** The heading is
  `The step did not finish`, and the card says what failed, what was undone and
  what is left. The reason is one sentence that a person can act on, never a
  stack of terminal text. Offer `Try again` and `Stop`.
- **A press is the only way on.** Ask for one choice at a time, and wait for the
  click with `next_click`.
- **Say what you do.** While a step runs, the card shows the step list with the
  time of each finished step, and the panel says one line.

## How to wait

1. Draw the card, or change it with `update`.
2. Call `next_click` with no `panel_id` and `timeout_millis: 300000`. Leave out
   every other field.
3. Read `component_id`, and `selected` when the press came from a row.
4. Do the work of that press, then `update` the card.
5. Call `next_click` again. On `timed_out`, call it again while the card matters,
   or stop and tell the person in the panel.

A press with `stale: true` names a document that the person no longer sees: read
the state again, show it, and let the person press again. A row that the document
marks `disabled: true` sends no press at all.

The panel is the other way to answer. When the person writes in the panel
instead of pressing, do what the words ask: a goal (`rent it`, `join my server`),
a machine name, a provider, a size, a place, or `back`. Draw the card for the
answer that you would have drawn for the press, and say one line in the panel
that you did.

`Back` returns to the card of the step before, with the choice of the person
still selected. `Stop` ends the flow, and the card says what stays.

## Goal `choose`

The person pressed the plain Set up control. Draw the first card. Every row is a
press, and no row starts selected.

```json
{"components": [
  {"type": "heading", "text": "What do you want to do?",
   "subtitle": "Choose one. Nothing is rented and nothing is changed before you confirm.",
   "badge": {"text": "Step 1"}},
  {"type": "list", "selection": "single", "rows": [
    {"id": "choose.workshop", "title": "Set up my whole workshop",
     "subtitle": "The server, the browser and the machines of your repositories, in one run.",
     "badge": {"text": "3 parts", "tone": "info"}},
    {"id": "choose.rent", "title": "Work on a bigger machine",
     "subtitle": "Rent a cloud machine in your own provider account."},
    {"id": "choose.server", "title": "Use Horizon on all your computers",
     "subtitle": "Set up the Horizon server. Machines, keys and settings follow you."},
    {"id": "choose.browser", "title": "A browser that stays signed in",
     "subtitle": "Chromium on a machine that stays on, for you and your agents."}]},
  {"type": "text", "text": "Do you have a server already? Join it from this computer, and write \"join\" in this panel. You can also write to the agent in its panel at any step."}
]}
```

Write one line in the panel beside the card: `Choose on the card, or write what
you want here.` A press on `choose.workshop` continues with the goal `workshop`, a
press on `choose.rent` with the goal `rent`, a press on `choose.server` with
`server`, and a press on `choose.browser` with `browser`. The words "I have a
server", "join", or the name of a server computer continue with the goal `join`.

The first start page of this version holds four tiles and no tile for the whole
workshop: that row of this first card is the entry of the goal `workshop`, and the
words "set up my workshop" or "set up everything" start it too.

## Goal `workshop`

The person wants the whole workshop: the server, a browser that stays signed in,
and machines for the repositories that they work on. The goal runs the three parts
in this order with one card that changes: the server, the browser, the machines.
The page [workshop](references/workshop.md) holds the first card with the state of
each part on this computer, the card between two parts, the rule that a part which
rents states its own price once, and the end card with what the workshop holds, its
price for each hour and the three things that a person does next.

The entry of this goal is the row `choose.workshop` of the first card of Goal
`choose`, and the words "set up my workshop" or "set up everything". The first
start page of this version keeps its four tiles, and it has no tile for this goal.

## Goal `rent`

Four cards: where, how big and where, the steps, and the card of the machine.

### 1. Where do you rent?

Run the read of each provider of `horizon-machines` first. A provider that is
signed in is a row with the badge `Ready`. A provider that cannot rent is a row
that cannot be chosen, whose second line names the command that signs in.

```json
{"components": [
  {"type": "heading", "text": "Where do you rent?",
   "subtitle": "The machine is made in your own account. You pay the provider, not Horizon.",
   "badge": {"text": "Step 2 of 4"}},
  {"type": "list", "selection": "single", "rows": [
    {"id": "provider.hetzner", "title": "Hetzner", "subtitle": "Signed in with hcloud",
     "badge": {"text": "Ready", "tone": "ok"}, "selected": true},
    {"id": "provider.azure", "title": "Azure", "subtitle": "Signed in with az",
     "badge": {"text": "Ready", "tone": "ok"}},
    {"id": "provider.gcp", "title": "Google Cloud",
     "subtitle": "Not signed in. Run gcloud auth login", "disabled": true},
    {"id": "provider.aws", "title": "AWS",
     "subtitle": "No profile found. Run aws configure sso", "disabled": true},
    {"id": "provider.runpod", "title": "RunPod",
     "subtitle": "GPU machines. No RUNPOD_API_KEY is stored", "disabled": true}]},
  {"type": "buttons", "items": [
    {"id": "back", "label": "Back"}, {"id": "look_again", "label": "Look again"}]}
]}
```

Keep the order of the rows and the state of each row as the read answered them.
Mark the provider that the person should take first `selected: true`. A press on
a row of a provider that can rent continues with the card of the size and the
place, with that provider. A press on a row that cannot be chosen never arrives.
`Look again` runs the read again and updates the card. `Back` returns to the
first card.

### 2. How big, and where?

Read the machine types and the list price of the chosen provider with its own CLI
just before you draw this card. Take three sizes from that answer, as the rule
below says, and put the price of the row in its `value`, in the money of the
provider and per hour. Take the places from the same answer, and mark the first
one `selected: true`.

The three rows are named Small, Medium and Large, and each one is the cheapest
type of this place that holds what it needs:

| Row | Cores and memory of the type | For |
|---|---|---|
| Small | at least 2 vCPU and 4 GB | agents and light builds |
| Medium | at least 4 vCPU and 8 GB | builds and tests |
| Large | at least 8 vCPU and 16 GB | several agents at once |

- Take the cheapest type that holds each pair, and an x86 type with shared
  cores: a dedicated core, an ARM type or a type with a GPU costs more for the
  same work. Name an ARM type only when the machine is for a person who asks for
  one.
- Read the types, their cores, their memory and their prices from the provider
  in this run, at the place that the person chose. The three rows are the
  cheapest types that hold their pair at that place and at that moment, so a
  second run of the same card at the same moment gives the same three rows. Do
  not keep a size from an earlier card, from a document or from the name of a
  type.
- When two types cost the same, take the one with more vCPU. When they also
  have the same vCPU, take the one with more memory. When they also have the
  same memory, take the type whose name sorts first, so the answer of two runs
  is the same.
- The `subtitle` of a row names the type and its true cores and memory, as the
  provider answered them, for example `cpx21, 3 vCPU, 4 GB. Agents and light
  builds.`. The row says what the place offers, never what the name Small
  suggests.
- A row whose pair this place does not offer is drawn, cannot be chosen, and
  says why: `disabled: true` with the subtitle `No type with 4 vCPU and 8 GB in
  this place.` That row is never `selected: true`, so the first pair that the
  place offers is the selected row.

```json
{"components": [
  {"type": "heading", "text": "How big, and where?",
   "subtitle": "Hetzner. The prices are the provider's list prices.",
   "badge": {"text": "Step 3 of 4"}},
  {"type": "caption", "text": "Size"},
  {"type": "list", "selection": "single", "rows": [
    {"id": "size.small", "title": "Small", "subtitle": "cpx11, 2 vCPU, 2 GB. Agents and light builds.",
     "value": "0.0058 EUR / hour"},
    {"id": "size.medium", "title": "Medium", "subtitle": "cpx21, 3 vCPU, 4 GB. Builds and tests.",
     "value": "0.0089 EUR / hour", "selected": true},
    {"id": "size.large", "title": "Large", "subtitle": "cpx31, 4 vCPU, 8 GB. Several agents at once.",
     "value": "0.0168 EUR / hour"}]},
  {"type": "caption", "text": "Place"},
  {"type": "list", "selection": "single", "layout": "pills", "rows": [
    {"id": "place.hel1", "title": "Helsinki", "selected": true},
    {"id": "place.fsn1", "title": "Falkenstein"},
    {"id": "place.ash", "title": "Ashburn"},
    {"id": "place.sin", "title": "Singapore"}]},
  {"type": "caption", "text": "Network"},
  {"type": "list", "selection": "single", "layout": "pills", "rows": [
    {"id": "network.none", "title": "None", "selected": true},
    {"id": "network.home", "title": "home"}]},
  {"type": "text", "text": "SSH: the machine key only; password login is turned off"},
  {"type": "buttons", "items": [
    {"id": "rent", "label": "Rent this machine", "tone": "primary"},
    {"id": "back", "label": "Back"}]}
]}
```

The line `SSH: the machine key only; password login is turned off` is the same on
every rent card, and it is the whole promise of the rent: the machine accepts the
machine key, and it accepts no password. The rent turns password login off after
the first key login and proves a fresh key login, as step 2 of the wait of
`horizon-machines` says. Do not ask the person about it: the card stated it
before the press, and the flow asks only when that proof fails.

The numbers of this document are examples. Write the values that you read: the
type name, the cores and the memory that the place answers. A size row whose
price the CLI does not answer holds `price not read` in `value`.
`Back` returns to the provider card with the provider of the person still
selected. The press on `Rent this machine` is the only press that may spend
money. The caption `Network` and its pills are an example too: draw them only
when the panel holds a stored network.

#### The network of the machine

Read the names of the stored networks with `block_panel` `secret_names`, as the
page `references/tailscale.md` of the skill `horizon-machines` says: the names
only, never a value. A network is stored only when all three of its names are
there: `TS_<NET>_CLIENT_ID`, `TS_<NET>_CLIENT_SECRET` and `TS_<NET>_TAG`.

- At least one network is stored: draw the caption `Network` and its pills below
the place list. The row `network.none` is the first row and it is
`selected: true`. Then draw one row `network.<net>` for each stored network, in
name order.
- No network is stored: draw no caption and no row. A press on `Join a network`
on the card of a machine draws the card that says what the person stores.

A pill is a choice of the person, and it changes no setting. One machine joins
one network. The names reach this card at any time, but a VALUE reaches only a
panel that starts after the store: only an agent panel gets the named secrets, so
a terminal that `open_terminal` opens and this panel itself have none that the
person stored after their start. A network that the person stores while this
panel runs is a pill here, and its join needs a new panel: say that on the card
and ask the person to open a new agent panel, or to start Set Up again, before
the join runs its commands. Never ask the person for a client id, a client secret
or a tag in this panel, and never take one from the conversation.

### 3. Renting the machine

The person pressed `Rent this machine`. Draw the step card, then do the steps of
`horizon-machines` with the chosen provider, size and place: the machine key,
the create, the wait for SSH, the tmux check and the machine record. Change the
card between the steps, so the `now` step is the step that runs.

```json
{"components": [
  {"type": "heading", "text": "Renting the machine",
   "subtitle": "Small, Helsinki, Hetzner. This takes about two minutes.",
   "badge": {"text": "Working", "tone": "info"}},
  {"type": "progress", "label": "Rent and prepare", "percent": 60, "detail": "3 of 6"},
  {"type": "steps", "steps": [
    {"label": "Make a machine key for Horizon", "state": "done", "at": "2 s"},
    {"label": "Create the machine at the provider", "state": "done", "at": "11 s"},
    {"label": "Wait for SSH with the machine key", "state": "done", "at": "38 s"},
    {"label": "Turn off password login", "state": "now", "at": "now"},
    {"label": "Check tmux on the machine"},
    {"label": "Add the machine to Machines"}]},
  {"type": "text", "text": "You can close this card. The work goes on, and the machine comes to Machines in the sidebar."},
  {"type": "buttons", "items": [
    {"id": "stop", "label": "Stop and delete the machine", "tone": "danger"}]}
]}
```

Write the `subtitle` as the size, the place and the provider of the person, and
the times as you measured them. `percent` is the share of the steps that are
done. The step `Turn off password login` is part of every rent, after the first
key login and before the check of the multiplexer, and it reports the proof of a
fresh key login. When the person chose a network, the list holds one step more,
`Join the network <NET>`, after `Check tmux on the machine` and before
`Add the machine to Machines`. The page `references/tailscale.md` of the skill
`horizon-machines` gives that card. A press on `Stop and delete the machine` deletes what this flow made, with
the delete of `horizon-machines`, and the card then says what was deleted and
what is left. It deletes nothing that this flow did not make.

When SSH answers and the record is written, continue with the card of the
machine.

### 4. The card of the machine

Create one new card, titled with the name of the machine. It stays after the Set
up card closes, and `horizon-machines` describes the same card for a later
session.

```json
{"components": [
  {"type": "heading", "text": "helsinki-1 is ready",
   "subtitle": "It is in Machines in the sidebar. A click there opens a terminal.",
   "badge": {"text": "Running", "tone": "ok"}},
  {"type": "kv", "rows": [
    {"label": "Provider", "value": "Hetzner, Helsinki"},
    {"label": "Size", "value": "2 vCPU, 4 GB"},
    {"label": "Address", "value": "203.0.113.10", "mono": true},
    {"label": "Rented since", "value": "09:40", "mono": true},
    {"label": "Cost", "value": "0.0079 EUR / hour", "mono": true}]},
  {"type": "text", "text": "You pay from Rent until Delete. Delete ends the cost and removes the key."},
  {"type": "buttons", "items": [
    {"id": "connect", "label": "Connect", "tone": "primary"},
    {"id": "server_here", "label": "Put the Horizon server here"},
    {"id": "browser_here", "label": "Add a browser"},
    {"id": "join_network", "label": "Join a network"},
    {"id": "stop", "label": "Stop"},
    {"id": "delete", "label": "Delete", "tone": "danger"}]}
]}
```

The card of a machine holds the buttons of every machine card of the skill
`horizon-machines`, and this card is no exception: `Connect`, `Put the Horizon
server here`, `Add a browser`, `Join a network`, `Stop`, and `Delete`. Draw
`Start` instead of `Stop` while the machine is stopped. A machine that a run prepared for a
repository keeps these buttons too, so a set-up continues from the card that the
run left.

- `Connect` opens the terminal of `horizon-machines`, with the machine record.
- `Put the Horizon server here` continues with the goal `server` and this machine
  as its place.
- `Add a browser` continues with the goal `browser` and this machine as its place.
- `Join a network` draws the card of the network of this machine, which the page
  `references/tailscale.md` of the skill `horizon-machines` gives. The card
  offers the stored networks as pills and no field for a secret.
- `Stop` stops the machine, and `Start` starts it again, as `horizon-machines`
  says. `Delete` asks before it deletes, as that skill says, and the record and
  the key go with the machine.

Then change the Set up card to the summary, so the person knows where the card
of the machine is:

```json
{"components": [
  {"type": "heading", "text": "The machine is ready",
   "subtitle": "helsinki-1 has its own card, and that card stays.",
   "badge": {"text": "Done", "tone": "ok"}},
  {"type": "text", "text": "Press Connect on the card of helsinki-1 for a terminal. Machines in the sidebar lists it too."},
  {"type": "buttons", "items": [{"id": "close", "label": "Close"}]}
]}
```

## Goal `server`

Four cards: where the server runs, the steps, the ready card, and the card for
another computer.

### 1. Where does the server run?

Read the machines of this Horizon with `machines`, and offer `This computer`, one
row for each machine of the list, and `A new rented machine`.

```json
{"components": [
  {"type": "heading", "text": "Where does the server run?",
   "subtitle": "The server keeps your machine list, the machine keys and your settings. Each computer of yours follows it and shows the same.",
   "badge": {"text": "Step 2 of 4"}},
  {"type": "list", "selection": "single", "rows": [
    {"id": "place.this_computer", "title": "This computer",
     "subtitle": "Free. It must be on when another computer needs it."},
    {"id": "place.machine.helsinki-1", "title": "helsinki-1",
     "subtitle": "A machine in your list. Hetzner, running.", "value": "always on", "selected": true},
    {"id": "place.new_machine", "title": "A new rented machine",
     "subtitle": "Rent a small machine first, then put the server on it."}]},
  {"type": "text", "text": "A server on a rented machine costs what the machine costs. Nothing leaves your own machines."},
  {"type": "buttons", "items": [
    {"id": "set_up_here", "label": "Set up on helsinki-1", "tone": "primary"},
    {"id": "back", "label": "Back"}]}
]}
```

One row per machine of the list, in name order, with `place.machine.<name>` as
its id and the provider line that the record holds as its second line. The
button names the place that the selected row holds, such as
`Set up on helsinki-1` or `Set up on this computer`.

A press on `place.new_machine` runs the goal `rent` first, and returns to this
card when the machine answers. `Back` returns to the first card.

### 2. Setting up the server

The person pressed the button that names the place. Draw the step card, then run
the steps of `horizon-server` on that place:

```json
{"components": [
  {"type": "heading", "text": "Setting up the server",
   "subtitle": "On helsinki-1. This computer is linked at the end.",
   "badge": {"text": "Working", "tone": "info"}},
  {"type": "progress", "label": "Install the server", "percent": 40, "detail": "2 of 5"},
  {"type": "steps", "steps": [
    {"label": "Copy Horizon to the machine", "state": "done", "at": "9 s"},
    {"label": "Install the server as a service", "state": "done", "at": "3 s"},
    {"label": "Start the server and read its answer", "state": "now", "at": "now"},
    {"label": "Link this computer over SSH"},
    {"label": "Send your machine list and settings"}]},
  {"type": "text", "text": "The settings of this computer are kept aside in a dated file before the first link."},
  {"type": "buttons", "items": [{"id": "stop", "label": "Stop and undo"}]}
]}
```

`Stop and undo` removes the service and the copy that this flow made on that
place, with the removal of `horizon-server`, and the card says what was removed
and what is left. It deletes nothing on this computer.

The last two steps of the list need the press of the person: call
`server_link_request` with `machine` and the name of the place, and the command
of the server, and wait while Horizon draws its own card. `machine` names the
machine of the list that the flow already gave Horizon, so the link uses the
address, the user, the key file and the known-hosts file of a terminal of that
machine. Name `ssh_target` only when the server runs on a computer of the person.

Read the answer of the request:

- `confirmed` with `link: in_step` means that the server answered and this
  computer is in step with it. Draw the ready card of the next section.
- `confirmed` with `link: away` means that the entry is written and the server
  did not answer within about 20 seconds. Draw the failure card with `reason` as
  its subtitle, which is one sentence that names the cause.
- `declined` and `timed_out` mean the server is set up and this computer does not
  follow it yet. Say which one on the card.

### 3. The server is ready

```json
{"components": [
  {"type": "heading", "text": "The server is ready",
   "subtitle": "This computer follows it now, and the sidebar says Server: in step.",
   "badge": {"text": "Answers", "tone": "ok"}},
  {"type": "kv", "rows": [
    {"label": "Runs on", "value": "helsinki-1"},
    {"label": "Starts by itself", "value": "yes, as a service"},
    {"label": "This computer", "value": "linked over SSH"},
    {"label": "SSH from", "value": "any address, machine key only"},
    {"label": "Sent", "value": "1 machine, your settings"},
    {"label": "Kept aside", "value": "settings.before-server.1763020800000.json", "mono": true}]},
  {"type": "buttons", "items": [
    {"id": "add_computer", "label": "Add another computer", "tone": "primary"},
    {"id": "close", "label": "Close"}]}
]}
```

Draw this card only for `link: in_step`. When the answer carries
`restart_needed`, the `subtitle` is `Close Horizon and start it again one time.`
instead, because this Horizon does not start the link of a new entry itself. An
agent never restarts the program of the person.

The row `SSH from` says which rule is in force on a rented machine:
`any address, machine key only` for the default, or
`<address of this computer>/32, machine key only` for a rule that the person asked
for. Read it from the firewall of the place, or from the card of
`horizon-machines` that made it, and never guess it.

For `link: away`, draw this card instead, and name the cause that `reason` gave:

```json
{"components": [
  {"type": "heading", "text": "The server did not answer",
   "subtitle": "The machine has the server, and this computer cannot reach it yet.",
   "badge": {"text": "Not linked", "tone": "danger"}},
  {"type": "text", "text": "<the sentence of reason>"},
  {"type": "text", "text": "The sidebar says Server away. The log of Horizon holds the words of the attempt."},
  {"type": "buttons", "items": [
    {"id": "try_again", "label": "Try again", "tone": "primary"},
    {"id": "stop", "label": "Stop", "tone": "neutral"}]}
]}
```

The machine, its service and this computer are not broken by an `away` link, so
`Try again` asks the person to press `Link` again on a new card, and `Stop`
leaves the server on the machine.

### 4. Add another computer

A second computer joins with one of three ways, and the place of this server
decides which one works. Choose the row of that way, and draw the other rows
`disabled: true` with the reason.

```json
{"components": [
  {"type": "heading", "text": "Add another computer",
   "subtitle": "This server keeps the machine list, the machine keys and your settings. Here is how another computer joins it.",
   "badge": {"text": "10 minutes"}},
  {"type": "kv", "rows": [
    {"label": "Computers now", "value": "1 (this one)"},
    {"label": "Server runs on", "value": "helsinki-1, a rented machine"},
    {"label": "This computer", "value": "linked over SSH with the machine key"}]},
  {"type": "caption", "text": "The other computer"},
  {"type": "list", "selection": "single", "rows": [
    {"id": "add.ssh_login", "title": "A computer that you log in to",
     "subtitle": "A rented machine holds no login of the person.",
     "disabled": true},
    {"id": "add.key_install", "title": "A machine that only this computer reaches",
     "subtitle": "The other computer makes a machine key, and this computer adds its public line to the machine.",
     "selected": true},
    {"id": "add.invite", "title": "An overlay network that both computers are on",
     "subtitle": "The server listens on the overlay, and Add a computer shows a link and a QR code.",
     "disabled": true, "badge": {"text": "No overlay", "tone": "warn"}}]},
  {"type": "text", "text": "The row that is selected is the way that works for the place of this server. A row that cannot be chosen says why."},
  {"type": "text", "text": "The link and the QR code of the last row are the Settings window, Server: a card cannot draw a code."},
  {"type": "buttons", "items": [
    {"id": "done", "label": "Done", "tone": "primary"},
    {"id": "close", "label": "Close"}]}
]}
```

The three ways are the three rows, in their order:

1. `add.ssh_login`, a server on a computer of the person, which the other computer
   reaches with the login of the person.
2. `add.key_install`, a server on a rented machine, which the other computer reaches
   with a machine key that this computer installs.
3. `add.invite`, a server on an overlay network, which the other computer reaches
   with a link and a code.

- A server on a computer of the person: the other computer writes the SSH name of
  that computer in Settings, Server, SSH target, with the command of the server,
  and starts Horizon again one time, or its agent asks for the same entry on a card
  that applies it at once. Its link then travels over SSH with the login of the
  person. No code, no listener and no key of Horizon.
- A server on a rented machine, with no overlay network: the other computer makes
  a machine key with `machine_key`, the person writes its public line in this
  panel, and you add that line to the `authorized_keys` of the machine through the
  machine key of this computer. Then the other computer writes one `Host` block of
  its SSH configuration for the machine, with that key file, and links over SSH.
  The skill `horizon-server` gives the exact commands.
- An overlay network: the server runs with `--listen <overlay address>:7000`, and
  Settings, Server, Add a computer shows a link and a QR code for ten minutes. The
  other computer pastes the link in Join with a link.

`Done` closes the Set up card, and `Close` does the same.

### 5. When the server must move

The person wants the server on another computer, or replaces the computer that
runs it. The goal is not a card of this flow: it is the skill
`horizon-server-move`, which gives the order, the cards, the check of the store
id and the undo of each step. Tell the person that one sentence, and read that
skill. It moves the same server home, so the machines, the keys, the values and
the computers that joined stay the same.

## Goal `browser`

Three cards: the engine and the place, the steps, and the ready card.

Read the browser host of this computer before the first card, with
`{"operation": "browser_host_status"}`. It answers the entry that the settings
hold: `state` is `host` with the machine or the SSH target, the profile, the
engine and the command of the driver, or `local` when the browser panels of
this computer use its own browser. The cards of this goal state those values and
no guess of your own.

### 1. The engine and the place

```json
{"components": [
  {"type": "heading", "text": "A browser that stays signed in",
   "subtitle": "The browser runs on a machine that stays on. You sign in one time, and every computer of yours opens the same pages. An agent can use it too.",
   "badge": {"text": "Step 2 of 4"}},
  {"type": "caption", "text": "Browser"},
  {"type": "list", "selection": "single", "rows": [
    {"id": "engine.chromium", "title": "Chromium",
     "subtitle": "Live picture. You and an agent share the page.",
     "badge": {"text": "Ready", "tone": "ok"}, "selected": true},
    {"id": "engine.firefox", "title": "Firefox",
     "subtitle": "For sites that refuse Chromium. It needs Horizon and Firefox on that machine, and its picture comes slower.",
     "badge": {"text": "Ready", "tone": "ok"}}]},
  {"type": "caption", "text": "Runs on"},
  {"type": "list", "selection": "single", "rows": [
    {"id": "place.machine.helsinki-1", "title": "helsinki-1",
     "subtitle": "A machine in your list. Hetzner, running.", "selected": true},
    {"id": "place.new_machine", "title": "A new rented machine",
     "subtitle": "Rent a small machine first."}]},
  {"type": "text", "text": "A machine that is in a Tailscale network is reached over that network. The card of a machine shows its network, and Join a network puts the machine in one."},
  {"type": "buttons", "items": [
    {"id": "set_up_browser", "label": "Put the browser on helsinki-1", "tone": "primary"},
    {"id": "back", "label": "Back"}]}
]}
```

Both rows take a press, and each engine has its own needs on that machine:

- Chromium runs on that machine through its `DevTools` port. The machine needs a
  Chromium program, and no Horizon.
- Firefox runs behind a driver of Horizon on that machine, so the binary of this
  computer is the driver there, and that machine needs Firefox and `geckodriver`
  beside it. The picture of that browser is pushed as compressed frames, so it is
  slower than Chromium. Name the program of the driver with
  `browser_host.command` when the default does not fit. On a machine without a
  browser of its own, the browser machine reference of the skill
  `horizon-machines` puts Firefox and `geckodriver` in a container and the driver
  in that container as well.

Read the binary of this computer from `HORIZON_EXECUTABLE` and never from `PATH`:
that variable is the program that runs Horizon, so a copy of it is the version
that the driver of that machine must speak. The copy also needs a C library that
is not newer than the one of the machine, and a binary built on a newer
distribution does not start on an older one. Read its C library before the setup
of a long flow, as step 2 says.

A press on `place.new_machine` runs the goal `rent` first and returns to this
card. `Back` returns to the first card.

### 2. Preparing the browser

The person pressed the button that names the place. Draw the step card, then run
the steps of `browser-machine` and the browser host request:

```json
{"components": [
  {"type": "heading", "text": "Preparing the browser",
   "subtitle": "The browser on helsinki-1. The first time takes some minutes.",
   "badge": {"text": "Working", "tone": "info"}},
  {"type": "progress", "label": "Prepare the browser", "percent": 40, "detail": "2 of 6"},
  {"type": "steps", "steps": [
    {"label": "Copy the browser files to the machine", "state": "done", "at": "4 s"},
    {"label": "Check the C library of the machine", "state": "done", "at": "22 s"},
    {"label": "Install the container program", "state": "done", "at": "41 s"},
    {"label": "Build the browser image", "state": "now", "at": "now"},
    {"label": "Start the browser with the profile work"},
    {"label": "Open a browser panel here"}]},
  {"type": "text", "text": "The browser listens only on the machine itself. This computer reaches it through SSH."},
  {"type": "buttons", "items": [{"id": "stop", "label": "Stop and undo"}]}
]}
```

The profile is the name of the person or `work` when the person names none: 1 to
40 characters of `a-z`, `0-9` and `-`. The word `private` names no profile: the
settings entry and the local panel menu use it for a panel that keeps nothing.

The profile of a local panel of this computer is the other place where a sign-in
stays. A panel of this computer opens in the shared profile `default` unless a
create names `profile: "private"`, so a person signs in one time on this
computer too, and every later panel of theirs is signed in. A private panel is
signed in nowhere, and its folder is gone when the panel closes. The panels of
one profile read the same sign-ins, and a sign-out in one signs out all of them.

The engine of the press decides the steps:

- Chromium: the steps of the reference `browser-machine` of the skill
  `horizon-machines`, and no step of a C library. The machine needs a Chromium
  program there, and no Horizon.
- Firefox in the container: the same steps with `--engine firefox`, and the
  binary of this computer as a fifth input. The setup prints a `command=` value;
  name it as `browser_host.command`, because the driver of Horizon runs inside the
  container and that machine needs no Horizon outside it. That driver needs a C
  library that the image has: read the two versions with the check of the setup
  as its own step, and stop the flow with the failure card when they do not
  allow the driver. The words of that card name the two versions and the three
  ways forward: the engine Chromium, a horizon binary of a system of the library
  of the image or older, or a base image with the newer library.

  ```sh
  ssh <target> 'sh setup.sh --engine firefox --horizon horizon --check'
  ```

  The last line names the two versions, such as
  `the driver needs glibc 2.43 and the image debian:forky-slim has glibc 2.43: the driver can start there`.
  The code is 0 when the driver can start in the image, and 3 when it cannot.
- Firefox on a machine that already has one: probe it with
  `command -v firefox; command -v geckodriver`, and stop when one of them answers
  nothing. Then copy the binary of this computer to the machine:

  ```sh
  scp "$HORIZON_EXECUTABLE" <target>:horizon
  ssh <target> 'chmod 755 horizon && ./horizon --version'
  ```

  The version of that copy must be the version of this computer, because the
  driver of the machine speaks one version of the link.

The last step needs the press of the person: call `browser_host_request` with the
machine of the list or the SSH target, the profile, the engine and, for a Firefox
host whose driver starts another way, the `command` that the setup printed. A
browser host serves several panels of one profile, for Chromium and for Firefox:
each panel is its own page of the one browser of that profile, so a later panel
of the same profile is signed in where the first one signed in. A panel with
`profile: "private"` is a page of an isolated browser context of that host.
Show
that command in the row `Driver command` of the card, as Horizon holds it, and
wait while Horizon draws its own card. Read the entry again with
`browser_host_status` after a `confirmed` answer, and draw the ready card from
that answer. The press serves the next browser panel at once, so no
panel needs a start of Horizon: when the answer says `confirmed`, open the panel
of that browser with `browser_create` beside this card and name no `backend`,
because the host serves the engine of its profile. A panel that was open before
the press keeps its own browser. A press on `Stop and undo` on a Firefox flow
removes the copied binary beside the files and the container of this flow, and the
card says what was removed and what is left.

### 3. The browser is ready

```json
{"components": [
  {"type": "heading", "text": "The browser is ready",
   "subtitle": "A browser panel is open. Sign in to your sites there. They stay signed in when you close Horizon.",
   "badge": {"text": "Running", "tone": "ok"}},
  {"type": "kv", "rows": [
    {"label": "Runs on", "value": "helsinki-1"},
    {"label": "Browser", "value": "Chromium"},
    {"label": "Profile", "value": "work", "mono": true},
    {"label": "New browser panels", "value": "use this browser"},
    {"label": "Downloads", "value": "stay on that machine"}]},
  {"type": "text", "text": "New browser panels use that browser now. Settings, General, Browser host shows the same."},
  {"type": "buttons", "items": [
    {"id": "open_panel", "label": "Open a browser panel", "tone": "primary"},
    {"id": "use_this_computer", "label": "Use this computer again"},
    {"id": "remove", "label": "Remove the browser", "tone": "danger"}]}
]}
```

The row `Browser` names the engine that the settings now hold: `Chromium`, or
`Firefox, with Horizon on that machine`. A Firefox host adds the row
`Driver program` with the command of `browser_host.command`, when the person set
one.

Read `{"operation": "browser_host_status"}` after the press of the person, and
draw the ready card from that answer: `state` `host`, `machine` or `ssh_target`
in the row `Runs on`, `engine` in the row `Browser`, `profile` in the row
`Profile`, and `command` in the row `Driver program` while it is present. A
value that the answer does not hold stays off the card, because no panel follows
it.

`Use this computer again` is a change of a setting, and only the person changes
it: `browser_host_request` writes a host and never clears one. Say on the card
and in the panel that Settings, General, Browser host takes the entry away, and
leave the setting as it is.

`Remove the browser` runs the removal of the files and the container that this
flow made on the machine, after the person confirms on this card. It leaves the
browser host entry as it is, and the card says that.

## Goal `join`

The person has a server that runs. Read the machines of this Horizon with
`machines`, and read the SSH hosts that the person already uses, with the SSH
configuration of this computer.

```json
{"components": [
  {"type": "heading", "text": "Join a server that you already have",
   "subtitle": "The server keeps your machine list, the machine keys and your settings. This computer follows it.",
   "badge": {"text": "Step 2 of 4"}},
  {"type": "caption", "text": "A machine or a computer that you use"},
  {"type": "list", "selection": "single", "rows": [
    {"id": "host.helsinki-1", "title": "helsinki-1",
     "subtitle": "A machine in your list. Hetzner, running.", "selected": true},
    {"id": "host.server-vm", "title": "server-vm",
     "subtitle": "An SSH host of this computer."}]},
  {"type": "text", "text": "Is the server computer not in the list? Write its SSH name in this panel, for example server-vm, and I use it."},
  {"type": "buttons", "items": [
    {"id": "join", "label": "Join", "tone": "primary"}, {"id": "back", "label": "Back"}]}
]}
```

The rows are the machines of the list first, with `host.machine.<name>`, and then
the hosts of the SSH configuration, with `host.<name>`. A press on `join`, or the
SSH name that the person writes in the panel, is the place of the server.

Then call `server_link_request` with that SSH target, and the command that runs
the server there: the whole path of the `horizon` binary of the server computer
and `--server-link`, for example `/home/person/bin/horizon --server-link`. Wait
while Horizon draws its own card, and read its answer.

- `confirmed` and `start: linked`: the entry of this computer is written and the
  link of this run follows it. Show the ready card of the goal `server`, with the
  subtitle `This computer follows it now, and the sidebar says Server: in step.`
- `confirmed` and `start: restart_needed`: show the same card with the subtitle
  `Close Horizon and start it again one time.`
- `declined` or `timed_out`: nothing is written. Show the card of the place again
  and say on it that the person pressed neither button.

## When a step fails

Change the card to the failure card. Keep the step list of the card and draw the
step that did not finish `failed`, so the person reads where the flow stopped.
Put the reason of the step in the first line of the `kv` row `What failed`: one
sentence, in the words of the tool or the system where those words help, and
never a stack of terminal text.

```json
{"components": [
  {"type": "heading", "text": "The step did not finish",
   "subtitle": "Nothing else was changed.", "badge": {"text": "Needs you", "tone": "warn"}},
  {"type": "steps", "steps": [
    {"label": "Make a machine key for Horizon", "state": "done", "at": "2 s"},
    {"label": "Create the machine at the provider", "state": "done", "at": "11 s"},
    {"label": "Wait for SSH with the machine key", "state": "failed", "at": "3 m"}]},
  {"type": "kv", "rows": [
    {"label": "What failed", "value": "The machine answered no SSH after 3 minutes."},
    {"label": "Undone", "value": "The machine key of helsinki-1."},
    {"label": "Left", "value": "The machine at Hetzner. It costs money until Delete."}]},
  {"type": "buttons", "items": [
    {"id": "try_again", "label": "Try again", "tone": "primary"},
    {"id": "stop", "label": "Stop", "tone": "danger"}]}
]}
```

`Try again` repeats the step that failed, and no earlier step. `Stop` ends the
flow and deletes what the flow made that costs money, with the delete of
`horizon-machines`, after the person confirms on this card. When a step changed
nothing, the row `Undone` says `Nothing.`

One failure is the one that the rent promised: a machine that still accepts a
password after `Turn off password login`. That failure is the only question of a
rent, and the card asks it, because a person must decide between another try and
the delete of a machine that a password can reach:

```json
{"components": [
  {"type": "heading", "text": "The machine still accepts a password",
   "subtitle": "You chose the machine key only, and this machine answered that a password works.",
   "badge": {"text": "Needs you", "tone": "danger"}},
  {"type": "text", "text": "The write of /etc/ssh/sshd_config.d/10-horizon.conf answered permission denied."},
  {"type": "text", "text": "Port 22 stays open, so a password would reach this machine from any address. The machine costs money until Delete."},
  {"type": "buttons", "items": [
    {"id": "try_again", "label": "Try again", "tone": "primary"},
    {"id": "delete", "label": "Delete the machine", "tone": "danger"}]}
]}
```

`Try again` repeats `Turn off password login` on the same machine. `Delete the
machine` removes the machine and its key, and then the card says what is left.

## The words of the panel

Say one line in the panel when the flow starts, and one line at each card, so a
person who reads the panel knows what the card asks:

```text
Set up. The card is beside this panel.
Choose on the card, or write what you want here.
```

Never put a secret, a private key, a public key, a token or a command with a
credential in the panel.

## Limits of this version

- Firefox on another machine needs Firefox, `geckodriver` and a `horizon` binary
  of the same version on that machine, and its picture is slower than the picture
  of Chromium. It refuses a file from this computer, a recording of the page and a
  native `<select>`, with one sentence each in the panel.
- A card cannot draw a QR code, a picture or a table. The invitation of another
  computer stays in Settings, Server, and the card names that place.
- The browser host entry serves each browser panel that a create makes after the
  press, and no start of Horizon is needed. A panel that is open keeps its
  browser until the person closes it and opens another one.
- One card holds the flow. An agent owns at most eight cards in one workspace, so
  keep the flow in the Set up card and the card of the machine that stays.
