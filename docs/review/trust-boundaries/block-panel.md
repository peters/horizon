---
applyTo: "crates/horizon-core/src/block_panel.rs,crates/horizon-core/src/block_panel/**,crates/horizon-browser-control/src/block.rs,crates/horizon-browser-control/src/block/**,crates/horizon-browser-control/src/manifest/block.rs,crates/horizon-browser-mcp/src/controller/block.rs,crates/horizon-browser-mcp/src/server.rs,crates/horizon-ui/src/app/block_clicks.rs,crates/horizon-ui/src/app/block_clicks/**,crates/horizon-ui/src/app/block_requests.rs,crates/horizon-ui/src/app/block_requests/**,crates/horizon-ui/src/block_widget/**"
---

# Trust boundary of agent cards

An agent composes a document and Horizon draws it as a card in the workspace of
that agent. The document is untrusted text, and a click of the person travels
back to that agent. The design is in
[`crates/horizon-browser-control/src/block.rs`](../../../crates/horizon-browser-control/src/block.rs)
and the module comments of the files above.

## Untrusted: the agent document

`block_panel` takes a document from the agent, and the card draws it beside the
person's terminals.

- The document is untrusted text. The host reads it as data and paints it as
  plain text, fills and outlines. No text is parsed as markup, a format string, a
  path or a command, and no field selects a size, a colour or a position.
- The host checks the document before any UI sees it, in the MCP server and again
  in the UI: at most 32 KiB, at most 64 components, at most 24 `kv` rows, 32
  `list` rows and 8 buttons, and at most 512 characters in one text. A text keeps
  no control character except the line break of a body text. An id is
  `[a-z0-9_.-]{1,64}` and is unique in the document. A row can be selected only
  in a list with `selection: single`, at most one row of a list. An unknown
  field, component or value is refused with `invalid_document`, and the message
  names the place, such as `components[2].items[0].id`.
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
- `next_click` requires the exact owner too. Another agent of the workspace gets
  `not_owner`, and a card that leaves the board answers `panel_unavailable`. A
  `next_click` without `panel_id` covers only the cards that the caller owns in
  its own workspace; a caller that owns none gets `no_cards`, so the wait never
  reaches another agent's card.

## Horizon writes into an agent terminal

A person's click must reach an agent that is idle and out of its tool loop, so
Horizon types one line into the terminal of the panel that owns the card.

- The line is the constant
  [`WAKE_LINE`](../../../crates/horizon-core/src/block_panel/wake.rs). It never
  carries text from a card, a panel title, a row or a button: a card cannot put
  its own text into a terminal. A test holds the constant to a literal and
  refuses a placeholder.
- The line goes out only while all of these hold: an agent panel of the owner is
  in the card's workspace; its status is `Idle`; it does not hold keyboard focus,
  so the person may be typing there without a collision; no click wait of the
  owner is open; no line went out for this streak of clicks yet; and the last
  line to that agent panel is at least 30 seconds old.
- The decision is the pure function
  [`wake_verdict`](../../../crates/horizon-core/src/block_panel/wake.rs), tested
  for every condition. A held line is tried again on a later frame while the
  click still waits; no timer thread and no repaint loop is added.
- The write uses the encoder of a person's paste and then Enter, so the agent
  TUI receives one complete line. It runs on the UI thread and touches no file.
- Horizon writes to no other panel, and a card panel has no terminal to write
  to.

## Horizon owns the frame and the reading of a document

A document reaches the body only. The parts that a person must be able to trust
are written by Horizon and no document field can set them:

- **The state chip** in the titlebar names the agent that owns the card and
  whether it waits for a click right now. The state comes from the host's click
  waits and from the agent panel in the workspace, never from the document.
- **The footer** names the owner as the host resolved it and shows the local
  clock time of the last create or update that the host applied. The time is the
  host's own stamp, so a card cannot claim to be older or newer.
- **The status frames** say that the agent is gone and that a click queue is
  full. Their text is a constant of the host, so an agent cannot dress its own
  words up as a host message. While the owner is gone every badge is drawn in the
  neutral tone with the prefix `Last seen: `, which is also the host's text.
- **The layout and the colours** are Horizon's: padding, row heights, tones and
  which button is the primary one follow fixed rules. A later `primary` button is
  drawn neutral, and every control of a card without its agent is drawn
  disabled.
- **The panel size and title** of a new card come from the host: 420 wide and
  as tall as the host measured its content needs, between 220 and 900 points,
  and a title that the host trims to one line. A card that the person resized by
  hand keeps that size, and a card that is shorter than its content scrolls its
  body above a footer that stays at the bottom.

A document may still choose its own words, its own ids, its tones, its order of
components and which row of a list starts selected. None of those carry host
state: a default row is a row id that the person can change.

## Does not guarantee

- **The text can lie.** Horizon checks the shape and the bounds of the document,
  never its meaning. A card that says `Connected` or `Deleted` states what the
  agent claims, not what Horizon observed.
- **The card can imitate Horizon.** A card is a panel with plain text and tones.
  An agent can write text that looks like a Horizon dialog, a banner or a system
  message, and can choose a panel title that looks like a real one. Its tone
  colours come from the same palette as other UI, so a tone is not a Horizon
  status. The footer and the state chip say that an agent drew the card, but no
  mark proves that the words inside it are host state, and a person must not
  treat a card as host truth.
- **The document is not private between agents of one workspace.** `list` and
  `inspect` let every agent panel of the workspace read the full document of
  every card of that workspace. An agent must not write a credential, a token or
  a private key into a document.
- **A click can be lost.** A click reaches an agent at most once, and the queue
  of a card lives in the Horizon process: a restart drops it. A click older than
  five minutes is dropped, a click on a full queue of 32 is dropped, and a click
  whose `next_click` ends at its deadline is answered with `timed_out`, not with
  the click. A card stage that is clicked after its document changed reports
  `stale`, and Horizon does not decide whether the older intent still holds. A
  row that a document marks as selected is Horizon's runtime state: the person's
  own selection wins while its row is still drawn.
- **The wake line can be ignored.** Horizon proves that the line was written to
  the terminal of an idle agent panel, not that the agent read it or acts on it.
  An agent that ignores the line never reads the click, and the person sees only
  the card and the clicks that wait.
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
