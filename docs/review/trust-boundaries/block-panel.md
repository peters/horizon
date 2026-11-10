---
applyTo: "crates/horizon-core/src/block_panel.rs,crates/horizon-browser-control/src/block.rs,crates/horizon-browser-control/src/block/**,crates/horizon-browser-control/src/manifest/block.rs,crates/horizon-browser-mcp/src/controller/block.rs,crates/horizon-browser-mcp/src/server.rs,crates/horizon-ui/src/app/block_requests.rs,crates/horizon-ui/src/app/block_requests/**,crates/horizon-ui/src/block_widget/**"
---

# Trust boundary of agent cards

An agent composes a document and Horizon draws it as a card in the workspace of
that agent. The document is untrusted text. The design is in
[`crates/horizon-browser-control/src/block.rs`](../../../crates/horizon-browser-control/src/block.rs)
and the module comments of the files above.

## Untrusted: the agent document

`block_panel` takes a document from the agent, and the card draws it beside the
person's terminals.

- The document is untrusted text. The host reads it as data and draws it with
  plain `egui` labels, buttons and bars. No text is parsed as markup, a format
  string, a path or a command.
- The host checks the document before any UI sees it, in the MCP server and again
  in the UI: at most 32 KiB, at most 64 components, at most 24 `kv` rows, 32
  `list` rows and 8 buttons, and at most 512 characters in one text. A text keeps
  no control character except the line break of a body text. An id is
  `[a-z0-9_.-]{1,64}` and is unique in the document. An unknown field, component
  or value is refused with `invalid_document`, and the message names the place,
  such as `components[2].items[0].id`.
- No field is a URL, a path, a file or a command. Horizon fetches nothing, opens
  nothing and runs nothing for a card. A card has no input, no image, no table
  and no link.
- The title is one line, trimmed, without control characters, at most 80
  characters.
- The panel id of a card is the host's own id. A request names a panel id only to
  select a card of the caller's workspace; a panel of another workspace is
  `panel_unavailable`.
- The host assigns the revision. An agent cannot write the revision, the owner
  or the visibility of a card that it does not own.

## Trusted: the caller identity

The caller is the agent panel that Horizon launched, and its identity is the two
variables that Horizon injects into that panel.

- The MCP server refuses a caller that has no `horizon:` actor and no live host
  instance with `caller_unavailable`.
- The UI resolves the actor to an agent panel of its own board. A request whose
  actor is not an agent panel of this host is `caller_unavailable`, and a request
  of another host is `wrong_host`.
- One card has one owner: the actor that created it. `update`, `visibility` and
  `close` require the exact owner, and another agent of the same workspace gets
  `not_owner`. One agent owns at most eight cards in one workspace.
- `list` and `inspect` are read-only and workspace-scoped. A card of another
  workspace is invisible.

## Does not guarantee

- **The text can lie.** Horizon checks the shape and the bounds of the document,
  never its meaning. A card that says `Connected` or `Deleted` states what the
  agent claims, not what Horizon observed.
- **The card can imitate Horizon.** A card is a panel with plain text and tones.
  An agent can write text that looks like a Horizon dialog, a banner or a system
  message, and can choose a panel title that looks like a real one. Its tone
  colours come from the same palette as other UI, so a tone is not a Horizon
  status. The card has no badge, border or icon that proves it is agent
  authored, and a person must not treat a card as host truth.
- **The document is not private between agents of one workspace.** `list` and
  `inspect` let every agent panel of the workspace read the full document of
  every card of that workspace. An agent must not write a credential, a token or
  a private key into a document.
- **The card's buttons and rows do nothing in this version.** No click or
  selection is delivered to an agent, and no operation waits for one. A card is
  not a prompt, and an agent that waits for a click waits forever.
- **The request queue is private, not authenticated per message.** A request is a
  `0600` file under the runtime root, bounded to 32 waiting requests and to 96 KiB
  each, pruned like the Device queue. Another process of the same user can write
  such a file; the UI still requires that the actor be an agent panel of its board
  and that the host instance match, so such a file can change no card outside
  those limits.
- **A document is dropped with its card.** Closing a card, or losing the panel
  from the session, loses the document. The revision is not a history, and past
  documents are not kept.
- **Restore is a copy of the saved panel state.** A restored card shows the last
  saved document at its revision. Its owner is the persisted actor of the agent
  panel, so an agent panel that is not restored cannot change the card; a person
  can still close it in the UI.
