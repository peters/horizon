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
  `list` rows and 8 buttons, at most 16 steps, at most 8 columns and 8 rows of a
  board with 32 items and 4 items in one cell, at most 4 answers of a callout and
  16 entries of a log, and at most 512 characters in one text. A text keeps no
  control character except the line break of a body text. An id is
  `[a-z0-9_.-]{1,64}` and is unique in the document, and the answers of a callout
  share that one namespace with the buttons of a `buttons` component. A row can be
  selected only in a list with `selection: single`, at most one row of a list. One
  board holds at most one current wave, and every item names a cell that the
  declared columns and rows hold. An unknown field, component or value is refused
  with `invalid_document`, and the message names the place, such as
  `components[2].items[0].id`, and the bound it broke, such as
  `components[1].items[3].column is 4, above the 3 columns of the board`.
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

## A board, a callout and a log carry plain text

Three parts describe a longer piece of work: a board of waves and workers, a
callout that asks a question, and a log of events. They add no way into the
machine and no way out of the card.

- **Plain text only.** Every field of them is drawn as text, a mark or a fill:
  the labels of the waves and of the workers, the titles, values, notes and tags
  of the items, the caption and the question of a callout, and the time and words
  of a log entry. No field is a path, a command, a size or a colour, and no field
  selects a position: the grid, the tints, the marks and the fold are Horizon's
  own layout of them.
- **Nothing is fetched and nothing is opened.** Horizon sends no request and
  starts no program for a board, a callout or a log. The three parts carry no
  address, so they cannot reach a page either.
- **An item, a step and a log line take no press.** They are reports. A press on
  them reaches no agent and no Horizon command. A step carries a label, a time
  and one state of `done`, `now`, `waiting` and `failed`. The state selects one
  mark that Horizon draws: a check, a ring, an empty ring or the cross of a
  failed board item. It selects no colour, no size and no position from the
  document, and it changes no other field of the card.
- **An answer of a callout is a button.** Its id is unique in the document with
  those of `buttons`, its press reaches `next_click` on the same path, and the
  first primary answer of the card is drawn as the primary action like the first
  primary button.
- **The fold of the narrow form is the person's own view.** The leading waves
  whose items all finished fold into one line, and a press on that line hides or
  shows them. That state is kept for the card, it is not part of the document and
  it is not a click of `next_click`, so an agent neither sets it nor reads it.
- **Does not guarantee:** that a wave really runs at the same time as its
  neighbours, that an item really finished, or that the marks of the states are
  true. A board is the agent's account of its own work, as every other part of a
  card is.

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
  `browser_host.ssh_target`, `browser_host.profile`,
  `browser_host.command`) before any card exists: one SSH name without a leading
  dash, a space or a control character, a command of letters, digits and
  `._:/=@+-` with single spaces, a profile of 1 to 40 characters of `a-z`, `0-9`
  and `-`. A request that fails the rule is refused with `invalid_request` and
  draws no card. An engine other than Chromium is `engine_unavailable`, and
  draws no card. A command beside the Chromium engine is refused as well: a
  Chromium host answers its `DevTools` port and starts no program here, so a
  command would name a program that nobody starts and the card would state a
  value that means nothing.
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
- **Only named fields of one entry change:** the SSH target and the command of
  `server`, or the SSH target, the profile, the engine and the command of
  `browser_host`. The address of a join and every other field of the entry stay
  as the store holds them; a browser host entry that names the Chromium engine
  writes no command, so a command of an earlier entry does not survive a press
  that names another engine. A confirmed browser host entry serves the browser
  panels that a create makes after the press of this run, which the card says,
  and a panel that is already open keeps its browser.
- **Does not guarantee:** that the target answers. Horizon writes the entry and
  starts the link; the link reports its own reason when the server is away, in
  the sidebar and in the Server tab. A press is not a proof that the person read
  the card.

### An agent reads the entry, and the answer holds no secret

`server_status` answers the entry that the settings of this computer hold, the
state of the link and the store id that this computer saw last. It is a read: it
writes no file, starts no link and changes no setting.

- **The answer is the store's own value.** Horizon reads the entry from the
  settings in memory and the link state from the snapshot of the supervisor, so no
  field is a claim of the caller and no field is a value that a person did not
  save. It carries the SSH target or the machine name, the command and the
  address, the state code of the link and the store id.
- **No secret is part of the entry.** The token of a join, the machine keys and
  the credential values are separate rows of the secrets file and separate files,
  so an answer that names only the entry of the settings cannot reach them. The
  link reads the token itself, and no card operation answers it.
- **Every agent panel of the workspace reads it.** The entry is the person's, and
  the answer names the same values to every agent of this Horizon. It carries no
  secret, so that is no new disclosure; an agent must still not write the address
  of a machine or a command into a card that a person did not agree to publish.

### An agent reads the browser host, and the answer holds no secret

`browser_host_status` answers the browser host entry that the settings of this
computer hold, or the word `local` when they name no host. It is a read: it
writes no file, opens no tunnel, starts no driver and changes no setting.

- **The answer is the store's own value.** Horizon reads the entry from the
  settings in memory and answers its trimmed values, so no field is a claim of
  the caller: the SSH target or the machine name, the profile, the engine and
  the driver command while the person set one. The agent therefore reads which
  browser serves a new browser panel, and needs no guess.
- **An entry that names no host answers `local` alone.** The browser panels of
  this computer then use its own browser, so no profile, engine or command of an
  older entry reaches the answer. A tunnel of a stopped entry is no part of the
  answer either, because the entry is the whole state that a person saved.
- **No secret is part of the entry.** The key of a machine, the password of a
  site and the token of a join are separate rows of the secrets file and
  separate files, so an answer that names only the entry of the settings cannot
  reach them. The tunnel reads the machine key itself, and no card operation
  answers it.
- **Every agent panel of the workspace reads it.** The entry is the person's,
  and the answer names the same values to every agent of this Horizon. It
  carries no secret, so that is no new disclosure; an agent must still not write
  the address of a machine into a card that a person did not agree to publish.
- **Does not guarantee:** that the machine answers, that the profile exists on
  it or that its browser is signed in. The answer states the entry that the
  person saved, not the state of a tunnel or of a page.

### An agent reads the presets, and the answer is a name and a kind

`presets` answers the presets that `open_panel` accepts, so an agent learns which
name to pass without asking the person. It is a read: it writes no file, changes
no setting and starts no program.

- **The answer is the settings' own value.** Horizon reads the list from the
  settings in memory, in the order of the store, and filters it with the rule of
  `open_panel` itself: a preset that runs no program of this computer, such as a
  browser page, a Device viewer, a card or an SSH terminal, is left out. The
  filter is the same predicate that refuses such a request, so the list and the
  operation cannot disagree.
- **A name and a kind alone.** One entry holds `name` and `kind`. The command,
  the arguments, the alias, the resume rule and the SSH route of a preset are in
  no answer, so a read of the list reaches no program of the person and no
  credential that a preset names. The kind is the token of the store, such as
  `claude` or `shell`, and not a display name.
- **Every agent panel of the workspace reads it.** The list belongs to the
  person's settings, and the answer names the same entries to every agent of this
  Horizon. It carries no secret and no command, so that is no new disclosure.
- **Does not guarantee:** that a preset works. The list states what the settings
  hold, not that the program of a preset is installed, that its folder exists or
  that its command answers. A name in the list is still the person's text, and an
  agent compares it exactly.

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

## A card whose owner panel is gone can be closed by another agent

A card stays on the board after the agent panel that owns it leaves the
workspace, and for a person it was the only one who could close it. A later
coordinator can now clean up after a plan of an earlier run.

- **Only when no owner panel is there.** The host resolves the owner actor of the
  card to an agent panel of the card's workspace, with the rule that `inspect`
  reports as `agent_present`. While such a panel exists, `close` by another agent
  is `not_owner`, so a living owner keeps its card closed to others.
- **The answer names the closer.** The outcome is `closed` with `by_owner: false`
  and the `closed_by` panel id of the caller, so a person reads who closed the
  card. A card that its owner closes answers `by_owner: true`.
- **No other operation opens.** `update`, `visibility` and `next_click` still
  require the exact owner, so a card of a gone owner can be closed and not
  changed. Its document is dropped with the card, as every closed card loses it.
- **The same claim every card has.** The card belongs to the caller's workspace,
  and an agent of another workspace gets `panel_unavailable` as before.
- **Does not guarantee:** that the plan of the card is finished, or that its work
  stopped. The closing agent decides from the plan file, and the person reads that
  file and not the card.

## Horizon types one line into a panel that an agent opened

`open_panel` can carry `prompt`: one line that Horizon writes into the terminal of
the new panel one time. The caller starts the program, so the line is a message to
that program and no new right.

- **Untrusted: the line.** It is text from an agent like a card's text. Horizon
  trims it, refuses a control character and refuses more than 512 characters, in
  the MCP server and again in the UI, with `invalid_request`. It is typed as a
  person's paste and then Enter, so the program of the panel receives one line:
  it is no command line, no argument of the preset, no environment and no file.
  The preset still holds every command and argument that Horizon runs.
- **An agent preset only.** The line goes out for a preset of an agent kind. A
  shell, a command or an SSH preset reads no sentence, so a request that names one
  with a prompt is `invalid_request` and opens no panel.
- **Once, and only when the program is ready.** Horizon types the line when the
  program of the panel runs, has drawn at least one output of its own, and its
  agent is at its prompt. Until then the line waits; a panel whose program never
  draws inside 60 seconds drops it, and the answer of the request has already
  said `opened`. The line goes out one time and is never repeated.
- **The person reads it.** The panel belongs to the person, its titlebar carries
  the mark `opened by <agent>`, and the line is in the terminal of that panel
  where the person can read it.
- **Does not guarantee:** that the program reads the line, acts on it, or is an
  agent that understands the sentence. Horizon proves that the bytes reached the
  terminal of a panel whose preset the person saved, not what the program does
  with them. The line carries no secret: another agent of the workspace can read
  the request, so an agent must not put a token or a password in it.

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
  as tall as the host measured its content needs, between 140 points and the
  ceiling of the card, and a title that the host trims to one line. A document
  that asks for the wide card opens at half of the canvas view of that moment,
  between 1100 and 2400 points, and as tall as its content needs. The ceiling of
  a narrow card is 900 points and the ceiling of a wide card is 1200 points,
  and the room that the canvas view shows at the zoom of the moment beats the
  ceiling while it is larger: the card of a window that shows 1150 points may
  take 1150 points, so nothing of its content hides under its footer. Every
  update of a document takes the width that it asks for, so a card of a board
  widens when the board gains a wave and narrows again when the document is
  narrow, and its left edge stays where the person put it. A card that the
  person resized by hand keeps that size, a card of an arranged workspace keeps
  the size of its preset, and a card that is taller than its ceiling and than
  the room of the view scrolls its body above a footer that stays at the
  bottom. The form of every component follows the real width of the card body,
  so the same document draws the wide form at 1000 points or more and the narrow
  form under it.

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

## An agent reads names, and no value

`secret_names` answers the names of the named secrets that an agent panel of this
computer gets when it starts, so an agent draws a choice of a stored network
without reading a value.

- **A name is the whole answer.** The outcome holds the names, in name order, and
  no value, no length of a value and no hash of one. There is no operation of
  `block_panel` that answers a value, and the card operations hold no field that
  could carry one.
- **It is a read.** The operation starts nothing, writes nothing and reads no
  file: it answers the snapshot of the named secrets that the worker of this
  start published. The boundary of that snapshot is
  [`named-secrets.md`](named-secrets.md), which also states who can read a name
  and why a value reaches only a panel that starts after the store.
- **A panel's own environment does not change.** The calling panel keeps the
  environment of its start, so a value of a secret that the person stores while
  the panel runs reaches no command of that panel, of a card and of a terminal
  that the agent opens.
