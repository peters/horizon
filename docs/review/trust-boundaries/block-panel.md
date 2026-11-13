---
applyTo: "crates/horizon-core/src/block_panel.rs,crates/horizon-core/src/block_panel/**,crates/horizon-browser-control/src/block.rs,crates/horizon-browser-control/src/block/**,crates/horizon-browser-control/src/manifest/block.rs,crates/horizon-browser-mcp/src/controller/block.rs,crates/horizon-browser-mcp/src/server.rs,crates/horizon-ui/src/app/block_clicks.rs,crates/horizon-ui/src/app/block_clicks/**,crates/horizon-ui/src/app/block_links.rs,crates/horizon-ui/src/app/block_links/**,crates/horizon-ui/src/app/block_requests.rs,crates/horizon-ui/src/app/block_requests/**,crates/horizon-ui/src/app/settings_request.rs,crates/horizon-ui/src/app/settings_request/**,crates/horizon-ui/src/block_widget/**"
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
- No field is a path, a file or a command. Horizon fetches nothing, opens
  nothing and runs nothing for a card, and a `link` reaches a page only when the
  person presses it. A card has no input, no image and no table.
- The title is one line, trimmed, without control characters, at most 80
  characters.
- The panel id of a card is the host's own id. A request names a panel id only to
  select a card of the caller's workspace; a panel of another workspace is
  `panel_unavailable`.
- The host assigns the revision. An agent cannot write the revision, the owner
  or the visibility of a card that it does not own.

## A link names one page, and a person opens it

An agent writes the address of a link, and the person decides whether to open it.
Horizon is the one that opens it, in a browser panel of its own.

- **The agent chooses the URL and the label.** Both are its text. Horizon checks
  the address when the document arrives, in the MCP server and again in the UI:
  an absolute `http` or `https` URL with a host, without credentials
  (`user:pass@`), without whitespace and without a control character, at most
  2048 characters, and at most 8 links in one card. Anything else is
  `invalid_document` and names the place, such as `components[3].url`. The
  refusal does not repeat the address, because an agent can put a secret in it.
- **The real host is always shown.** The card draws the host of the address under
  the agent's label, normalized as the browser resolves it, with its port when the
  URL states one that the scheme does not imply. A host that lookalike letters
  spell is drawn with its own address, so the label cannot hide where the link
  goes. The host is read once per document, not on every frame.
- **Nothing is fetched before a press.** Horizon parses the address and draws it.
  It sends no request, opens no socket and starts no browser engine until the
  person presses the link.
- **The press is Horizon's, not the agent's.** A press opens a browser panel of
  the card's own workspace, beside the card, through the same internal path as a
  person's new browser panel: the browser host and profile rules of the settings
  apply, and the panel is an ordinary browser panel afterwards, which the person
  drives and closes. The press records no click and reaches no agent, so a link
  works while the agent that drew it is gone.
- **A refused creation is Horizon's own line.** If the browser panel cannot be
  created, the reason reaches the person through the log line that every refused
  panel creation of a person uses. A card cannot draw it.

## An agent states a setting, and only the person confirms it

The server target and the browser host decide where the machine list, the keys
and the settings of the person go, so `server_link_request` and
`browser_host_request` change nothing by themselves.

- **Untrusted: the target, the command, the profile and the engine.** They are
  text from an agent. Horizon checks them with the rule of the settings field
  that stores them (`server.ssh_target`, `server.command`,
  `browser_host.ssh_target`, `browser_host.profile`) before any card exists: one
  SSH name without a leading dash, a space or a control character, a command of
  letters, digits and `._:/=@+-` with single spaces, a profile of 1 to 40
  characters of `a-z`, `0-9` and `-`. A request that fails the rule is refused
  with `invalid_request` and draws no card. An engine other than Chromium is
  `engine_unavailable`, and draws no card.
- **The card shows the values that the store will hold.** Horizon trims the
  target and the command, stores the trimmed value, and draws that same value in
  the card, so the person reads exactly what is written. The card is Horizon's
  own document: its words are constants of the host, and no field of the request
  reaches a sentence except the values above, drawn as plain text.
- **Only a press writes.** The request is held open while the card asks, and the
  answer is `confirmed`, `declined` or `timed_out`. An agent can press nothing:
  the two buttons are the host's controls, the card has no owner agent, and a
  press reaches the host and no agent. A closed card and a card whose five
  minutes passed both answer `timed_out` and remove the card, and a second
  request while one waits is `request_pending`.
- **The write is the one the Settings window makes.** Horizon saves the store on
  the settings worker, with the sections that the write changed, exactly as the
  Settings window's save does. The card states that the settings of this
  computer are kept aside in a dated file first, which is the rule of the first
  link with a server: the link writes `settings.before-server.<time>.json` beside
  the store when the server holds a section that replaces a value of this
  computer.
- **A confirmed server entry starts the link of this run.** The old link ends
  before the new one starts, as it does after a join, so the person starts
  Horizon no second time. `start` says `linked` when the running program follows
  the entry. A host that cannot start the link answers `restart_needed` instead,
  and the person starts Horizon again one time. A link that starts and cannot
  reach its server is still `linked`: the entry is in force, and the link says
  its own reason in the sidebar and in the Server tab.
- **Only two fields of one entry change:** the SSH target and the command of
  `server`, or the SSH target and the profile of `browser_host`. The address of a
  join and every other field of the entry stay as the store holds them. The
  browser host applies at the next start of Horizon, which the card says, because
  the browser panels of this run read the entry of their start.
- **Does not guarantee:** that the target answers. Horizon writes the entry and
  starts the link; the link reports its own reason when the server is away, in
  the sidebar and in the Server tab. A press is not a proof that the person read
  the card.

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
- A setting request passes the same checks as every other operation, and its
  card is drawn in the caller's own workspace. A request of another host is
  `wrong_host`, a request whose actor is no agent panel of this board is
  `caller_unavailable`, and a request that reached the host after its deadline is
  `request_expired`.

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
  as tall as the host measured its content needs, between 140 and 900 points,
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
- **A label can lie about its destination.** A link label is the agent's words;
  only the host under it is the address that Horizon opens. The host is the
  whole destination that Horizon shows, so a person must read it before a press.
- **The page is whatever that host serves.** Horizon opens the URL in a browser
  panel with the profile and browser host of the settings, and the page can be a
  sign-in form, a redirect or an attack by that site. The page is not a card
  claim and not a Horizon page.
- **A link is not a secret channel.** The address is visible in the card, in the
  browser panel and in the panel title, and another agent of the workspace reads
  the whole document. An agent must not put a token, a password or a signed URL
  in a link.
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
- **A setting wait is lost with a restart too.** The card of a setting request
  and its wait live in the Horizon process, so a restart answers nothing and the
  asking agent reads `host_timeout` or no answer. The entry that the person
  already confirmed stays in the store.
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
