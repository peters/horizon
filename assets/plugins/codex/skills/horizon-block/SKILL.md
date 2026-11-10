---
name: horizon-block
description: Draw a card of plain text in the calling agent's Horizon workspace with the block_panel MCP tool, and inspect, update, hide or close it. A person's click on a button or a selectable row reaches the agent that owns the card through next_click, which can wait on every card of the agent at once. Browser pages use horizon-browser and native desktops use horizon-device.
---

# Horizon cards

A card is a small surface of plain text that an agent composes in the person's
Horizon workspace. Use a card to show the state of the work that the agent
does: a machine that it holds, a plan, a list of results, a figure that it
computed. A card is not a terminal, a browser page or a native desktop, and it
takes no input.

The `block_panel` MCP tool manages cards. Every operation is scoped to the
agent panel that Horizon launched and to its workspace. A caller without that
panel gets `caller_unavailable`.

`block_panel` operations are `create`, `update`, `list`, `inspect`,
`next_click`, `visibility`, `close` and `open_terminal`.

## Create a card

The card is drawn in the caller's workspace. It does not take keyboard focus.

```json
{"operation": "create", "title": "cloud-1",
 "document": {"components": [
   {"type": "heading", "text": "cloud-1", "subtitle": "Hetzner · Helsinki",
    "badge": {"text": "Running", "tone": "ok"}},
   {"type": "badge", "text": "running", "tone": "ok"},
   {"type": "kv", "rows": [
     {"label": "Address", "value": "203.0.113.10", "mono": true},
     {"label": "Price", "value": "0.42 EUR/h"}]},
   {"type": "text", "text": "Billed from 14:02 UTC."},
   {"type": "progress", "label": "Hour 3 of 24", "percent": 12, "detail": "Saving"},
   {"type": "list", "selection": "single", "rows": [
     {"id": "size.cx22", "title": "CX22 - 2 vCPU", "subtitle": "4 GB memory"},
     {"id": "size.cx32", "title": "CX32 - 4 vCPU",
      "subtitle": "gcloud is not installed", "tone": "warn"}]},
   {"type": "buttons", "items": [
     {"id": "connect", "label": "Connect", "tone": "primary"},
     {"id": "delete", "label": "Delete", "tone": "danger"}]}]}}
```

Keep the returned `panel_id`. The answer is `created` with `panel_id` and
`revision`: the first revision is 1.

An agent owns at most eight cards in one workspace. A ninth card is refused
with `too_many_cards`; close a card that the person does not need.

## Update a card

`update` replaces the whole document. It does not merge with the old one. Read
the returned `revision`; it increases with every new document.

```json
{"operation": "update", "panel_id": "<panel_id>",
 "document": {"components": [{"type": "text", "text": "Hour 4 of 24."}]}}
```

Only the agent that created a card may update it, hide it, show it or close it.
Another agent of the same workspace gets `not_owner`.

## List and inspect cards

`list` answers every card of the calling workspace, with `panel_id`, `title`,
`revision`, `owned_by_caller`, `pending_clicks` and `listening`.

```json
{"operation": "list"}
```

`inspect` answers one card with its document, its revision, `visible`,
`agent_present`, `owned_by_caller`, `pending_clicks` and `listening`.
`agent_present` is false when no agent panel of the owner is in the workspace
any more: the person then keeps the last document with no agent behind it, and
the buttons and rows of the card are disabled. `pending_clicks` counts the
clicks that wait, and `listening` is true while a `next_click` of the owner
waits on that card, alone or on every card of the owner.

```json
{"operation": "inspect", "panel_id": "<panel_id>"}
```

List and inspect work for every agent of the workspace. A card of another
workspace answers `panel_unavailable`, and a panel that is not a card answers
`not_block_panel`.

## Hide, show and close a card

`visibility` hides or shows a card that the caller owns. A hidden card keeps its
document and its revision.

```json
{"operation": "visibility", "panel_id": "<panel_id>", "visible": false}
```

`close` removes a card that the caller owns. Its document is lost. Read what you
need from `inspect` before you close it.

```json
{"operation": "close", "panel_id": "<panel_id>"}
```

## Connect an SSH terminal

`open_terminal` opens an SSH terminal panel for the person. The panel goes in the
calling workspace, beside your agent panel, and it does not take keyboard focus.
Use it when the person needs a shell on a machine that you set up.

```json
{"operation": "open_terminal",
 "ssh": {"host": "lab-1", "user": "deploy", "port": 2222},
 "remote_command": "tmux new -A -s build", "title": "lab-1 build"}
```

The answer is `opened` with the `panel_id` of the terminal.

Give the person a session that survives a disconnect: pass a remote command that
attaches or creates a terminal multiplexer session. Horizon adds `-t`, so the
program gets a terminal:

```
ssh lab-1 -t tmux new -A -s build
```

- `ssh.host` (required) and `ssh.user` take letters, digits, `.`, `_` and `-`,
  and `:` in the host for an IPv6 address. A label cannot start with `-` and
  holds no space and no control character.
- `ssh.port` is 1 to 65535. Omit it to use the SSH configuration of the person.
- `remote_command` is one line of letters, digits, spaces and the characters
  `.`, `_`, `:`, `/`, `,`, `=`, `@`, `+` and `-`, at most 512 characters. It
  cannot start with `-`. Do not use a quote, `;`, `&`, `|`, `$`, a backtick,
  `<`, `>`, `(`, `)`, a backslash or a line break.
- `title` is one line of at most 80 characters. It defaults to `user@host`.

You can pass no other field. Horizon builds the `ssh` command line itself, so
you cannot add an ssh option, an identity file, an argument or an environment.

Tell the person on a card what the terminal is for, and what to do in it. The
titlebar of the panel carries the mark `opened by <agent>`, so the person knows
that you opened it. The panel is an ordinary SSH terminal after that: the person
types in it and closes it. You cannot read it, write to it or close it with this
tool.

## The document

A document holds components in draw order, and only these components:

- `heading`: `text`, optional `subtitle` and optional `badge` with `text` and
  optional `tone`. The badge is drawn on the title row, on the right.
- `text`: `text`. Body text; the only text that may hold line breaks.
- `kv`: `rows` with `label`, `value` and optional `mono`. A `mono` value is
  drawn in the monospace family: use it for an address, a command or a hash.
- `badge`: `text` and optional `tone`. A badge on a row of its own.
- `progress`: `label`, `percent` and optional `detail`, drawn on the right of
  the label row.
- `divider`: no fields.
- `list`: `selection` (`none` or `single`) and `rows` with `id`, `title`,
  optional `subtitle`, optional `tone` for the colour of the subtitle and
  optional `selected`. A row with `selected: true` is the row that the list
  starts with: a click reports it until the person selects another row. Only a
  list with `selection: single` may hold a selected row, and at most one row of
  a list.
- `buttons`: `items` with `id`, `label` and optional `tone`.

The limits are:

- 32 KiB for the whole serialized document.
- 64 components.
- 24 rows in one `kv` and 32 rows in one `list`.
- 8 buttons in one `buttons`.
- 512 characters in one text, including `subtitle`, the `badge` text and
  `detail`.
- Ids from `[a-z0-9_.-]`, at most 64 characters, unique in the whole document.
- A `tone` from `neutral`, `info`, `ok`, `warn`, `danger` or `primary`.
- A `percent` from 0 to 100.
- A `selected` row only in a list with `selection: single`, at most one row of
  a list.

Only the first `primary` button of a card is drawn as the primary action; a
later one is drawn as a neutral button. A button with tone `danger` is drawn at
the right edge of its row. Horizon draws every badge in the neutral tone when
the owner of the card is gone, with the prefix `Last seen: `.

An unknown field, component or field value is refused. There are no inputs,
images, tables or links, and no field that Horizon fetches or opens.

A card is plain text that Horizon draws. It is not verified Horizon state: the
person reads the agent's claim. Do not show a credential, a token or a private
key on a card.

## Read a click

A person can press a button, and select a row in a list that has
`selection: single`. A selection is Horizon's own state: it is not saved, and a
new document drops a selection whose row is gone. A press on a selectable row
changes the selection and is also a click. A list whose row has `selected: true`
starts with that row selected, until the person selects another row.

`next_click` waits for the next click on a card that the caller owns. Leave out
`panel_id` and it waits on every card that the caller owns, oldest click first:
that is the normal listen loop, because the person presses whichever card they
see. It answers as soon as a click waits, or with `timed_out` when its wait ends.

```json
{"operation": "next_click", "panel_id": "<panel_id>", "timeout_millis": 300000}
```

```json
{"operation": "next_click", "timeout_millis": 300000}
```

The wait is 1000 to 3600000 milliseconds; the default is 300000. A click names
the button or row that the person hit and the document revision that they saw:

```json
{"status": "clicked", "click": {
  "event_id": 4, "panel_id": "<panel_id>", "component_id": "continue",
  "revision": 7, "selected": ["size.cx32"],
  "at_millis": 1700000000000, "stale": false}}
```

- `component_id` is the button id or list row id that the person hit.
- `revision` is the document revision that the person saw, not the current one.
- `selected` holds the row ids that were selected at that moment, in document
  order. It holds the row that the document marked `selected` until the person
  selects another one. This is how a button such as Continue tells the agent
  which option the person chose.
- `event_id` counts the clicks of one card from 1. Two clicks on the same
  component have different ids.
- `stale` is true when the document changed after the person pressed. Confirm
  the intent before you act on a stale click.

A click reaches an agent at most once. Keep a listen loop:

1. Create or update the card with the choice that the person must make.
2. Call `next_click` without `panel_id`.
3. Act on the click, and send the result with `update`.
4. Call `next_click` again.
5. On `timed_out`, call `next_click` again while the card matters, or stop and
   tell the person in the terminal.

One `next_click` waits per card, and one without `panel_id` waits per agent: a
later wait of the same form answers the older one with `timed_out` first. A wait
that names a card and a wait on every card of its agent can run together; a
click goes to the wait that names its card first, and to the any-card wait
otherwise. Closing a card answers a `next_click` that names it with
`panel_unavailable`, and an any-card wait whose agent owns no card left answers
the same. A wait on the card of another agent answers `not_owner`.

A click that no `next_click` waits for is kept for five minutes, at most 32 per
card. A click on a full queue is dropped and the card says that too many clicks
wait; an older click is never dropped for a newer one.

## The wake line

When a click waits and no `next_click` listens, Horizon types one fixed line
into the owner's agent terminal:

```
A button was pressed on your Horizon card. Call the block_panel tool with operation next_click to read it.
```

The line never carries text from the card, so a card cannot write into a
terminal. It arrives only when the owner's agent panel is idle and holds no
keyboard focus, at most once per streak of clicks and at most every 30 seconds
per agent panel. Do not wait for it when you are already in `next_click`: while
it listens, the click is delivered at once and no line is typed.

An agent that ignores the line never reads the click. `listening` and
`pending_clicks` in `list` and `inspect` say whether a click waits.

## Errors

Every refusal is a `failed` outcome with a stable `code` and a message.

- `wrong_host`: the request belongs to another Horizon host. Send it from the
  agent panel that Horizon launched.
- `caller_unavailable`: the calling agent panel is not in this host, or the
  server started without the identity that Horizon injects. Start the agent
  from Horizon.
- `panel_unavailable`: the panel id is not in the calling workspace. Call
  `list` to see the cards that the caller can reach.
- `not_block_panel`: the panel id is a panel that is not a card.
- `not_owner`: another agent created this card. Read it with `inspect`; only its
  owner changes it.
- `invalid_document`: the document breaks a rule. The message names the
  offending place, such as `components[2].items[0].id` or
  `components[1].rows`. Correct that place and send the document again.
- `document_too_large`: the serialized document is above 32 KiB. Send less
  text, or more than one card.
- `too_many_cards`: the agent owns eight cards in this workspace. Close one.
- `no_cards`: an agent without a card in this workspace asked `next_click`
  without `panel_id`. Call `create` first.
- `invalid_ssh_route`: the host, the user or the port of `open_terminal` breaks a
  rule. The message names the field.
- `invalid_remote_command`: the remote command breaks a rule. The message names
  the field.
- `too_many_terminals`: the agent panel opened eight terminals in this Horizon
  run. Closing a terminal does not free its place; ask the person to open the
  next one.
- `terminal_unavailable`: Horizon could not open the terminal panel.
- `request_expired`: Horizon did not apply the request in 10 seconds, or in the
  10 seconds and the wait that a `next_click` asks for. Call `list` before you
  retry, because the change may have completed.
- `host_timeout`: Horizon did not answer within 15 seconds, or within the wait
  of a `next_click` plus 5 seconds. Call `list` before you retry a mutation;
  older Horizon hosts do not support cards.

## Limits of this version

- A card holds buttons and selectable rows. It has no text input, no image, no
  table and no link.
- The selection of a row is Horizon state, not a click of its own: it reaches
  the agent with the next click, and it starts at the row the document marks
  `selected`.
- A wait without `panel_id` covers every card that the agent owns now, also one
  that it creates while the wait runs.
- A click reaches an agent at most once. There is no replay, and the queue of a
  card is not saved.
- A card is not saved as a file; its document lives in the panel and comes back
  after a restart of Horizon.
- Only the agent that created a card can change it.
- A terminal that you open with `open_terminal` is not a card. It has no
  document, no click and no `next_click`, and this tool cannot change it after
  it is open.
