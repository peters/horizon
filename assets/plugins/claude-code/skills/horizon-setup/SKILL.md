---
name: horizon-setup
description: >-
  Run the Set up wizard for the person, one card per step: rent a cloud machine, put the Horizon
  server on a computer, add a browser that stays signed in, or join a server that already runs. Use
  it when Horizon starts an agent with "Use the horizon-setup skill" and a goal, or when the person
  asks to set up Horizon, to rent a machine for Horizon, to use Horizon on all their computers, to
  open a browser on another machine, or to join a server.
---

# Horizon Set up

Set up is the wizard of Horizon. Horizon starts an agent panel and gives it one
first message:

```text
Use the horizon-setup skill. The person pressed Set up. Goal: <goal>.
```

The goal is `rent`, `server`, `browser`, `join`, or `choose` when the person
pressed the plain Set up control and not one of the four tiles of the first
start page. This skill is the script of the wizard for each goal. The work
itself belongs to other skills:

- `horizon-block` holds the card contract, `next_click`, the machine records,
  the machine keys and the terminals.
- `horizon-machines` holds the providers: the commands, the rent, the wait for
  SSH and the delete.
- `horizon-server` holds the Horizon server on a machine: the copy, the service,
  the check and the removal.

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
you want here.` A press on `choose.rent` continues with the goal `rent`, a press
on `choose.server` with `server`, and a press on `choose.browser` with `browser`.
The words "I have a server", "join", or the name of a server computer continue
with the goal `join`.

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
just before you draw this card. Take three sizes from that answer, smallest
first, and put the price of the row in its `value`, in the money of the provider
and per hour. Take the places from the same answer, and mark the first one
`selected: true`.

```json
{"components": [
  {"type": "heading", "text": "How big, and where?",
   "subtitle": "Hetzner. The prices are the provider's list prices.",
   "badge": {"text": "Step 3 of 4"}},
  {"type": "caption", "text": "Size"},
  {"type": "list", "selection": "single", "rows": [
    {"id": "size.small", "title": "Small", "subtitle": "2 vCPU, 4 GB. Agents and light builds.",
     "value": "0.0079 EUR / hour", "selected": true},
    {"id": "size.medium", "title": "Medium", "subtitle": "4 vCPU, 8 GB. Builds and tests.",
     "value": "0.0158 EUR / hour"},
    {"id": "size.large", "title": "Large", "subtitle": "8 vCPU, 16 GB. Several agents at once.",
     "value": "0.0316 EUR / hour"}]},
  {"type": "caption", "text": "Place"},
  {"type": "list", "selection": "single", "layout": "pills", "rows": [
    {"id": "place.hel1", "title": "Helsinki", "selected": true},
    {"id": "place.fsn1", "title": "Falkenstein"},
    {"id": "place.ash", "title": "Ashburn"},
    {"id": "place.sin", "title": "Singapore"}]},
  {"type": "buttons", "items": [
    {"id": "rent", "label": "Rent this machine", "tone": "primary"},
    {"id": "back", "label": "Back"}]}
]}
```

The numbers of this document are examples. Write the values that you read. A
size row whose price the CLI does not answer holds `price not read` in `value`.
`Back` returns to the provider card with the provider of the person still
selected. The press on `Rent this machine` is the only press that may spend
money.

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
  {"type": "progress", "label": "Rent and prepare", "percent": 60, "detail": "3 of 5"},
  {"type": "steps", "steps": [
    {"label": "Make a machine key for Horizon", "state": "done", "at": "2 s"},
    {"label": "Create the machine at the provider", "state": "done", "at": "11 s"},
    {"label": "Wait for SSH with the machine key", "state": "done", "at": "38 s"},
    {"label": "Check tmux on the machine", "state": "now", "at": "now"},
    {"label": "Add the machine to Machines"}]},
  {"type": "text", "text": "You can close this card. The work goes on, and the machine comes to Machines in the sidebar."},
  {"type": "buttons", "items": [
    {"id": "stop", "label": "Stop and delete the machine", "tone": "danger"}]}
]}
```

Write the `subtitle` as the size, the place and the provider of the person, and
the times as you measured them. `percent` is the share of the steps that are
done. A press on `Stop and delete the machine` deletes what this flow made, with
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
    {"id": "delete", "label": "Delete", "tone": "danger"}]}
]}
```

- `Connect` opens the terminal of `horizon-machines`, with the machine record.
- `Put the Horizon server here` continues with the goal `server` and this machine
  as its place.
- `Add a browser` continues with the goal `browser` and this machine as its place.
- `Delete` asks before it deletes, as `horizon-machines` says, and the record and
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

For `link: away`, draw this card instead, and name the cause that `reason` gave:

```json
{"components": [
  {"type": "heading", "text": "The server did not answer",
   "subtitle": "The machine has the server, and this computer cannot reach it yet.",
   "badge": {"text": "Not linked", "tone": "error"}},
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

```json
{"components": [
  {"type": "heading", "text": "Add another computer",
   "subtitle": "Start Horizon on the other computer, open Set up there and choose Join a server that you already have.",
   "badge": {"text": "10 minutes"}},
  {"type": "kv", "rows": [
    {"label": "Computers now", "value": "1 (this one)"},
    {"label": "Reached over", "value": "your private network only"}]},
  {"type": "text", "text": "With an SSH login on the server computer, the other computer links by itself. Without one, Settings, Server shows a QR code and a code for one computer."},
  {"type": "buttons", "items": [{"id": "done", "label": "Done", "tone": "primary"}]}
]}
```

The QR code and the list of the computers are the Settings window, Server: a card
cannot draw a code. Say that on the card, as this document does. `Done` closes
the Set up card.

## Goal `browser`

Three cards: the engine and the place, the steps, and the ready card.

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
     "subtitle": "For sites that refuse Chromium. Not on another machine yet.",
     "badge": {"text": "To build"}, "disabled": true}]},
  {"type": "caption", "text": "Runs on"},
  {"type": "list", "selection": "single", "rows": [
    {"id": "place.machine.helsinki-1", "title": "helsinki-1",
     "subtitle": "A machine in your list. Hetzner, running.", "selected": true},
    {"id": "place.new_machine", "title": "A new rented machine",
     "subtitle": "Rent a small machine first."}]},
  {"type": "buttons", "items": [
    {"id": "set_up_browser", "label": "Put the browser on helsinki-1", "tone": "primary"},
    {"id": "back", "label": "Back"}]}
]}
```

Only Chromium runs on another machine. The Firefox row keeps
`"disabled": true` and the badge `To build` until Horizon serves it, and one
field of that row turns it on. Do not offer a Firefox row that takes a press.

A press on `place.new_machine` runs the goal `rent` first and returns to this
card. `Back` returns to the first card.

### 2. Preparing the browser

The person pressed the button that names the place. Draw the step card, then run
the steps of `browser-machine` and the browser host request:

```json
{"components": [
  {"type": "heading", "text": "Preparing the browser",
   "subtitle": "Chromium on helsinki-1. The first time takes some minutes.",
   "badge": {"text": "Working", "tone": "info"}},
  {"type": "progress", "label": "Prepare the browser", "percent": 40, "detail": "2 of 5"},
  {"type": "steps", "steps": [
    {"label": "Copy the browser files to the machine", "state": "done", "at": "4 s"},
    {"label": "Install the container program", "state": "done", "at": "41 s"},
    {"label": "Build the browser image", "state": "now", "at": "now"},
    {"label": "Start the browser with the profile work"},
    {"label": "Open a browser panel here"}]},
  {"type": "text", "text": "The browser listens only on the machine itself. This computer reaches it through SSH."},
  {"type": "buttons", "items": [{"id": "stop", "label": "Stop and undo"}]}
]}
```

The profile is the name of the person or `work` when the person names none: 1 to
40 characters of `a-z`, `0-9` and `-`. The last step needs the press of the
person: call `browser_host_request` with the SSH target, the profile and the
engine, and wait while Horizon draws its own card. `Stop and undo` removes the
container and the files of this flow on the machine, and the card says what was
removed and what is left.

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
  {"type": "text", "text": "New browser panels use it after the next start of Horizon. Settings, General, Browser host shows the same."},
  {"type": "buttons", "items": [
    {"id": "open_panel", "label": "Open a browser panel", "tone": "primary"},
    {"id": "use_this_computer", "label": "Use this computer again"},
    {"id": "remove", "label": "Remove the browser", "tone": "danger"}]}
]}
```

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

Change the card to the failure card. Put the reason of the step in the first line
of the `kv` row `What failed`: one sentence, in the words of the tool or the
system where those words help, and never a stack of terminal text.

```json
{"components": [
  {"type": "heading", "text": "The step did not finish",
   "subtitle": "Nothing else was changed.", "badge": {"text": "Needs you", "tone": "warn"}},
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

- Firefox on another machine is not built. Its row cannot be chosen.
- A card cannot draw a QR code, a picture or a table. The invitation of another
  computer stays in Settings, Server, and the card names that place.
- The browser host entry applies at the next start of Horizon. The card says so.
- One card holds the flow. An agent owns at most eight cards in one workspace, so
  keep the flow in the Set up card and the card of the machine that stays.
