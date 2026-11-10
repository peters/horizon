---
name: horizon-block
description: Draw a card of plain text in the calling agent's Horizon workspace with the block_panel MCP tool, and inspect, update, hide or close it. Cards show state; their buttons and rows do nothing in this version. Browser pages use horizon-browser and native desktops use horizon-device.
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
`visibility` and `close`.

## Create a card

The card is drawn in the caller's workspace. It does not take keyboard focus.

```json
{"operation": "create", "title": "cloud-1",
 "document": {"components": [
   {"type": "heading", "text": "cloud-1"},
   {"type": "badge", "text": "running", "tone": "ok"},
   {"type": "kv", "rows": [
     {"label": "Address", "value": "203.0.113.10"},
     {"label": "Price", "value": "0.42 EUR/h"}]},
   {"type": "text", "text": "Billed from 14:02 UTC."},
   {"type": "progress", "label": "Hour 3 of 24", "percent": 12}]}}
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
`revision` and `owned_by_caller`.

```json
{"operation": "list"}
```

`inspect` answers one card with its document, its revision, `visible`,
`agent_present` and `owned_by_caller`. `agent_present` is false when no agent
panel of the owner is in the workspace any more: the person then keeps the last
document with no agent behind it.

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

## The document

A document holds components in draw order, and only these components:

- `heading`: `text`. One line.
- `text`: `text`. Body text; the only text that may hold line breaks.
- `kv`: `rows` with `label` and `value`.
- `badge`: `text` and optional `tone`.
- `progress`: `label` and `percent`.
- `divider`: no fields.
- `list`: `selection` (`none` or `single`) and `rows` with `id`, `title` and
  optional `subtitle`.
- `buttons`: `items` with `id`, `label` and optional `tone`.

The limits are:

- 32 KiB for the whole serialized document.
- 64 components.
- 24 rows in one `kv` and 32 rows in one `list`.
- 8 buttons in one `buttons`.
- 512 characters in one text.
- Ids from `[a-z0-9_.-]`, at most 64 characters, unique in the whole document.
- A `tone` from `neutral`, `info`, `ok`, `warn`, `danger` or `primary`.
- A `percent` from 0 to 100.

An unknown field, component or field value is refused. There are no inputs,
images, tables or links, and no field that Horizon fetches or opens.

A card is plain text that Horizon draws. It is not verified Horizon state: the
person reads the agent's claim. Do not show a credential, a token or a private
key on a card.

## Buttons and rows do nothing

Buttons and list rows are drawn, and they are inert. No click reaches an agent
in this version, and no operation waits for one. Never put a decision on a card
and wait for it. Ask the person in the terminal instead.

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
- `request_expired`: Horizon did not apply the request in 10 seconds. Call
  `list` before you retry, because the change may have completed.
- `host_timeout`: Horizon did not answer within 15 seconds. Call `list` before
  you retry a mutation; older Horizon hosts do not support cards.

## Limits of this version

- No click or selection reaches an agent.
- A card has no input, no image, no table and no link.
- A card is not saved as a file; its document lives in the panel and comes back
  after a restart of Horizon.
- Only the agent that created a card can change it.
