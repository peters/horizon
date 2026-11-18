---
applyTo: "crates/horizon-sync/src/live.rs,crates/horizon-sync/src/live/**,crates/horizon-sync/src/supervisor/live.rs,crates/horizon-sync/tests/live.rs,crates/horizon-pty-tap/src/share.rs,crates/horizon-pty-tap/src/share/**,crates/horizon-core/src/terminal/share.rs,crates/horizon-core/src/terminal/share/**,crates/horizon-core/src/terminal/follow.rs,crates/horizon-core/src/terminal/follow/**,crates/horizon-core/src/terminal/events.rs,crates/horizon-core/src/terminal/resize.rs,crates/horizon-core/src/terminal/lifecycle.rs,crates/horizon-core/src/panel/follow.rs,crates/horizon-core/src/panel/follow/**,crates/horizon-core/src/board/follow.rs,crates/horizon-core/src/config/server.rs,crates/horizon-ui/src/app/shared_terminals.rs,crates/horizon-ui/src/app/shared_terminals/**,crates/horizon-ui/src/app/undrawn.rs,crates/horizon-ui/src/app/sidebar/computers.rs,crates/horizon-ui/src/app/settings/server/sharing.rs,crates/horizon-ui/src/app/settings/server/sharing/**,docs/architecture/shared-terminals.md"
---

# Trust boundary of shared terminals

A shared terminal is a terminal panel of one computer that the other computers
of the same server show and type in. Each computer holds a live link to the
server, and the server relays the screens, the output and the typed bytes. A
follow panel therefore parses the output of a program of another computer. A
program of one computer gets the keys of a person at another computer.

The design is in [`docs/architecture/shared-terminals.md`](../../architecture/shared-terminals.md).
The live link uses the transports, the greeting and the login of the sync link,
which [`server-link.md`](server-link.md) states.

**The owner of the product decided this boundary.** Every computer that is
linked to a server can see the terminals of the other linked computers and type
in them. A person who controls one linked computer, or the server, can therefore
run commands on every linked computer that shares its terminals with typing on.
The server is trusted with the content that goes through it. The link has the
confidentiality of its transport only. That is the cost of the decision, and it
is not a defect to report.

**A computer shares its terminals by default, with typing on.** A computer
that follows a server shares each terminal panel of its board, and it lets the
other computers type in them. Two settings of that computer turn them off:
`server.share_terminals` and `server.remote_typing`. The section `server` stays
with one computer, so no other computer and no server changes them.

The window shows the sentence `SHARED_TERMINALS_NOTICE` in three places:

- the **Server** tab of the Settings window;
- the join of a computer;
- the card of a request that names a server.

The sentence is:

> The other computers that you link to this server can see the terminals of
> this computer and type in them. Anyone who gets into one of those computers,
> or into the server, can then run commands here.

## Untrusted: every frame of another computer

- **A live frame comes from a computer that the transport let in.** The live
  link sends the `hello` of the sync link. On the Unix socket the device name is
  a claim. On the TCP listener the token proves it.
- **A link is a live link or a sync link, not the two.** `open` must be the
  first request after the greeting. A live request on another link ends that
  link with `not_live`, and another frame on a live link ends it with
  `live_link`.
- **A frame that the protocol does not know ends the link.** A field that the
  protocol does not know and `data` that is not base64 end the link with
  `bad_frame`. A frame above 256 KiB ends it with `frame_too_large`.
- **Each text is cleaned and cut at the server.** The server removes the control
  characters of each text. It keeps 60 characters of a label, 40 of a device
  name and 64 of a panel identifier. It keeps 120 characters of a title, 24 of
  a kind, 24 of a state and 80 of a workspace name.
- **Each list has a bound.** One computer shares at most 64 panels and follows
  at most 8. One panel has at most 4 followers. One server has at most 32 live
  links. The panels of the list of all computers share 192 KiB.
- **Each frame of bytes has a bound.** `output` holds 1 byte to 48 KiB and
  `typed` holds 1 byte to 16 KiB. The server drops a frame outside its bound.
  The grid of a `screen` and of a `resize` is 2 to 1000 columns and 1 to 500
  rows (`horizon_core::bounded_screen_grid`).
- **A `resize` asks for the grid of the program of the owner.** A follower
  names a computer and a panel that it follows, and a grid. The server drops a
  third request in a second for one follower and one panel. The relay passes
  the request only while the list of the owner says `typing: true` for that
  panel, and the owner checks its two switches and its own list again. The grid
  changes the size that the program of the owner reads, so it changes what that
  program does next. The guarantees below name the place of each check.
- **A `heartbeat` names nothing.** The hub drops one that a computer sends. It
  only says that the link is there, so a frame of that kind can change one time
  and nothing else.
- **A frame does not name its sender.** The server writes the `owner` of a
  screen and of output from the link that sent the request. It writes the
  `from` and the `label` of typed bytes the same way.
- **A computer that does not read cannot hold another computer.** No request of
  the hub waits for a link. At most 1 MiB of frames wait for one live link.
  Above that, the server drops output for that link. If the other frames alone
  go above it, the server ends the link.
- **An event of the server is checked against the list of this computer.** The
  window watches a panel, and types in a panel, only when the panel is in the
  list that this computer gave the link. The name that an event gives a panel
  is not a path and not a command: it is compared with the identifiers of that
  list.
- **The window cuts each text that it shows.** A name, a title and a reason of
  another computer show without control characters, at 80 characters at most.

## Untrusted: the bytes of a program on another computer

- **A follow panel parses the output of a program that this computer did not
  start.** The outlet of the owner, the two handles and the server copy that
  output and filter none of it. The terminal of the follow panel parses it with
  `alacritty_terminal`, under the rules of a local panel and the rules below.
- **The grid has a bound.** The feed sets the grid of a screen to 2 to 1000
  columns and 1 to 500 rows.
- **The output that waits has a bound.** At most 8 MiB wait in the feed, and
  above that the feed drops what waits. The handle holds events up to a ceiling
  of 4 MiB, and above that the reader of the link waits.
- **A follower answers no question of those bytes.** A program can ask a
  terminal for the cursor position, the device attributes, a colour and the
  size of the text area. It can also ask for the clipboard and for the status
  protocol. A follower writes no answer and no focus report. The terminal of
  the owner answers its own program.
- **A follower takes no copy request.** An OSC 52 copy request of those bytes
  does not go to the clipboard of this computer.
- **A follower takes no resize of its own grid.** A resize of the panel
  changes nothing of the terminal of a follower, and no size of that panel goes
  to the program of the owner. Only a `screen` event sets the grid of a
  follower. The `resize` request is another rule: a person of a follower gives
  the program of the owner a grid, and that changes no screen of a follower.
- **The texts of a stream are cleaned and cut.** The feed keeps 200 characters
  of each text and no control character. The texts are the device name, the
  label, the panel identifier, the title and the reason of an end.
- **No program of this computer belongs to a follow panel.** It starts none,
  restarts none and signals none. No saved session holds it.

## Untrusted: what a person on another computer types

- **Typed bytes are keys for the program of the owner.** In a shell they are
  commands, with the rights of the person of the owner computer. That is the
  purpose of the feature.
- **The server relays typed bytes on two conditions.** The computer that sent
  them must follow that panel now, and the list of the owner must say
  `typing: true` for that panel. If not, the server drops the frame.
- **A follower without typing sends none.** The feed of a follow panel drops
  the bytes of its person when `typing` is `false`. It also drops them after
  the stream ended.
- **The owner checks the event again.** The window writes the bytes into the
  program only while the two switches of this computer are on and the panel is
  in its list. The switches are read at the moment of the event, so a switch
  that the person cleared stops the next typed byte.
- **The server names the sender, and the owner uses the name for its chip.** A
  `typed` event holds the device name and the label of the link that sent the
  bytes. The window cuts that text at 80 characters and shows it in the chip of
  the panel as `typed by <name>`. The name decides nothing: the owner writes
  the bytes on the two switches and its list, whatever the name says.

## Untrusted: the grid that a follower asks for

- **A grid of one computer reaches the program of another computer.** `resize`
  carries a computer, a panel and a grid. The relay and the owner each check
  it, and the guarantees below name the two places.
- **A program can act on its grid.** A program reads its size. It can print a
  different screen, stop, or start a command for one size. The person of the
  owner therefore sees what another computer asked for, and the next key or
  mouse report of that person gives the program the grid of the body again.
- **The panel of the owner draws a foreign grid while it holds one.** It draws
  that grid from the top left of its body and clips what does not fit, and it
  pushes no size of its own body.
- **The grid cannot leave the bounds.** `horizon_core::bounded_screen_grid`
  gives 2 to 1000 columns and 1 to 500 rows for a request, for a panel of a
  list and for the terminal of a follower.

## The guarantees

- **Bounds.** The constants and `clean_panels` are in
  `crates/horizon-sync/src/live/protocol.rs`. `State::request`, `State::follow`
  and `State::computers` of `crates/horizon-sync/src/live/hub.rs` apply them,
  and `LiveHub::open` holds the bound of the live links. The frame bound is
  `MAX_FRAME_BYTES` of `crates/horizon-sync/src/frame.rs`.
- **The ceiling of a link.** `Outbox::push` of
  `crates/horizon-sync/src/live/outbox.rs` never waits. It drops output above
  1 MiB, and `State::send` of the hub ends a link whose kept frames alone go
  above 1 MiB.
- **Clean texts.** `clean` of `crates/horizon-sync/src/live/protocol.rs` removes
  control characters and cuts a text. The server calls it for a label in
  `LiveHub::open`, for a list in `clean_panels`, and for a follow in
  `State::request`. `plain_text` of `crates/horizon-core/src/terminal/follow.rs`
  does the same for the texts of a follower.
- **The checks of `typed` at the server and at the follower.** `State::typed`
  of `crates/horizon-sync/src/live/hub.rs` holds the two conditions and writes
  the sender. `Terminal::follow_typed` of
  `crates/horizon-core/src/terminal/follow.rs` drops the bytes of a follower
  without typing.
- **The check of `typed` at the owner.** `Run::typed` of
  `crates/horizon-ui/src/app/shared_terminals/owner.rs` writes the bytes only
  when four conditions are true:
  1. `share_terminals` is `true`.
  2. `remote_typing` is `true`.
  3. The identifier of the event is in the list that the link has.
  4. A terminal panel of this computer has that identifier.

  The share pump does this check on its thread. It reads the two switches from
  its copy of what the window wants, and it takes the events of a pass before
  that copy. A frame gives the pump a changed switch at once.
- **A switch that is off shares nothing.** `Run::apply_wanted` of
  `crates/horizon-ui/src/app/shared_terminals/pump.rs` gives the link an empty
  list while `share_terminals` is `false`. The server then ends each follow of
  a panel of this computer.
- **Only a panel of the list has a watch.** `Run::watchers` of `owner.rs`
  returns for a panel that is not in the list that the link has. No event of a
  server therefore starts the copy of another panel.
- **A follow panel is never shared.** `Terminal::share_source` gives no source
  for a terminal that follows a stream, and `HorizonApp::shared_list` of
  `crates/horizon-ui/src/app/shared_terminals/list.rs` lists only a terminal
  with a source. Two computers therefore do not send one terminal in a circle.
- **A watched panel says so.** The title bar of a panel that another computer
  follows shows `seen by <name>`, or the count of the followers. It shows
  `typed by <name>` for 2 seconds after the bytes of that computer arrived, and
  `sized by <name>` while the program holds the grid of that computer
  (`SharedTerminals::chip` of `crates/horizon-ui/src/app/shared_terminals.rs`,
  `Watched::chip_text` of `crates/horizon-ui/src/app/shared_terminals/owner.rs`).
- **Each shown text is cut.** `shown` of
  `crates/horizon-ui/src/app/shared_terminals.rs` removes control characters
  and keeps 80 characters. The pump uses it where a text of another computer
  enters what the window reads: the names and the titles of the list, the
  chips and the reason of a lost link.
- **No answer of a follower to the program.** `Terminal::write_protocol` of
  `crates/horizon-core/src/terminal/lifecycle.rs` writes nothing for a follower.
  `Terminal::answer_query` of `crates/horizon-core/src/terminal/events.rs`
  returns before it. The bytes of the person go to the feed and never to the
  link.
- **No clipboard.** `Terminal::handle_event` of
  `crates/horizon-core/src/terminal/events.rs` drops the copy request of a
  follower.
- **The checks of a resize at the relay and at the owner.** `State::resize`
  of `crates/horizon-sync/src/live/hub.rs` passes a request only while a follow
  of that follower, owner and panel exists and the listed panel of the owner
  says `typing: true`. It cuts the grid at `bounded_screen_grid` and passes two
  requests a second. `Run::resized` of
  `crates/horizon-ui/src/app/shared_terminals/owner.rs` holds the grid only
  while both switches are on, the list that the link has names the panel, and
  one panel of this computer has that identifier and is no follow panel.
  `ShareSource::hold_grid` of `crates/horizon-core/src/terminal/share.rs`
  resizes the terminal and sends the size to the program.
- **The sign of life.** `Alive` of `crates/horizon-sync/src/live/alive.rs` keeps
  the two times and their three bounds. `sign_of_life` of
  `crates/horizon-sync/src/live/relay.rs` sends `heartbeat` and ends a silent
  link with `live_silent`. `LiveHandle::next_batch` and the reader of a link in
  `crates/horizon-sync/src/supervisor/live.rs` do the same on the side of a
  computer.
- **The history of a fresh screen has a bound.** `draw_history` of
  `crates/horizon-core/src/terminal/share.rs` keeps 1000 lines and 256 KiB of
  them (`HISTORY_LINES`, `HISTORY_BYTES`). It draws no line above the viewport
  of a program on the alternate screen.
- **A screen has a bound and goes out in parts.** `Watched::drain` of
  `crates/horizon-ui/src/app/shared_terminals/owner.rs` sends no screen above
  8 MiB (`KEPT_SCREEN_BYTES`). `Watched::send_owed` gives the grid and then the
  parts of at most 48 KiB, and it waits for the next pass with half a part that
  the link refused.
- **No resize of a follower or of a held grid.** `Terminal::resize_with_policy`
  of `crates/horizon-core/src/terminal/resize.rs` returns for a follower and for
  a terminal that holds the grid of a follower, so the body of such a panel
  pushes no size. `FollowFeed::screen` of
  `crates/horizon-core/src/terminal/follow.rs` is the one place that sets the
  grid of a follower. `Terminal::release_grid` of
  `crates/horizon-core/src/terminal/share.rs` is the one place that ends the
  hold of a grid.
- **No program.** `Panel::follow` of `crates/horizon-core/src/panel/follow.rs`
  makes the panel from its content and never by the start of a panel. The kind
  of the panel is the shell, so no agent rule applies to it.
- **Nothing stored.** The hub and the outbox of `crates/horizon-sync/src/live/`
  hold memory only, and they open no file. The handle of
  `crates/horizon-sync/src/supervisor/live.rs` holds memory only.
  `crates/horizon-core/src/runtime_state.rs` saves no follow panel. The
  greeting of a live link writes the row of its computer in the store, as the
  greeting of each link does.
- **No byte of a terminal in a log line.** A log line of the hub names a device
  and a count. `LiveBytes` prints its count and never its bytes.
- **A removal of a computer ends its live link.** `remove_device` of
  `crates/horizon-sync/src/server.rs` ends each open link of that computer with
  `removed`, and the live link is one of them. `LiveHub::close` then removes its
  panels from the list, and its followers get `ended` with `offline`. The
  handle makes no more attempt after `removed`. A computer of the Unix socket
  can open a live link again after a new start, because `ssh` is its login.
- **One live link for each device name.** `LiveHub::open` ends an older live
  link of the same device name with `live_replaced`.
- **A terminal that no computer follows copies nothing.** `ShareOutlet::copy` of
  `crates/horizon-pty-tap/src/share.rs` returns after one atomic load. The
  outlet holds at most 1 MiB, and it drops all of it above that.
- **An older server gets no live frame.** `run` of
  `crates/horizon-sync/src/supervisor/live.rs` sends `open` only to a server
  that answered version 9 or higher (`LIVE_PROTOCOL_VERSION`).
- **A computer gets only the streams that it asked for.** `Inner::accept` of
  the same file drops a `screen` and `output` of a panel that the application
  did not ask to follow. The mark `own` of a list never comes from a frame.
- **No call waits for I/O.** No call of `LiveHandle`, of `FollowFeed` and of
  `ShareSource` waits for a link, so the share pump and the UI thread can call
  them. The pump and the UI thread hold a lock between them for one copy only.

## What the guarantees do not cover

- **Who can follow.** Each computer with a live link can follow each panel in
  the list of each other computer, up to the limits. The server has no
  permission for one computer, and an owner cannot refuse one follower. One
  switch shares each terminal panel of a computer or none. No setting keeps one
  panel out, and the list also holds the agent panels and the `ssh` panels.
- **Who can type.** Each follower of a panel with `typing: true` can type in it.
  A person who controls one linked computer can therefore run commands on each
  linked computer that shares a terminal with typing on. The chip of the panel
  says `typed by <name>` for 2 seconds after the bytes of a computer arrived,
  but a label is text that a computer chose, so it does not prove which
  computer sent the bytes.
- **The server.** The server reads each screen, each byte of output and each
  typed byte that goes through it. A server that another person controls can
  send a `typed` event to an owner. The owner obeys it while its two switches
  are on and the panel is in its list. Such a server can also send a screen and
  output to a follower, and a list to each computer.
- **The default.** A computer shares its terminals, with typing on, from its
  first link. A person who does not read the sentence of the Settings window
  shares the terminals of that computer with typing on.
- **Confidentiality.** Horizon adds no encryption to a live link. The transport
  is `ssh`, or the private overlay network of the TCP listener. Nothing protects
  the content between two computers from the server.
- **Who a computer is.** The label of a computer is text that the computer
  chose, and two computers can have the same label. On the Unix socket the
  device name is a claim too, as [`server-link.md`](server-link.md) states. A
  computer of the socket can claim the device name of another computer. It
  then ends the live link of that computer and has its name in the list.
- **The content of a terminal.** The parts filter none of it. A follower sees
  all that the program shows, also a secret that the program prints. Those
  bytes can draw any text and set the title of the terminal. A click on a link
  that they drew opens that link on this computer.
- **What a person types in a follow panel.** The keys, a paste and the mouse
  reports go to a program of another computer, also a secret that the person
  pastes.
- **The texts of an event on a computer.** `LiveHandle` does not clean a text
  again. The window cuts the texts that it draws, with one exception. The hover
  text of the name of a computer is its device name as the list gave it. The
  bound of that text is the frame bound.
- **The names in a chip.** The chip `seen by <name>` shows the label that the
  server gave for a follower, and the chip `typed by <name>` or `sized by
  <name>` shows the label that the server gave for the computer that typed or
  that holds the grid. A label is text that a computer chose, so a chip does
  not prove which computer follows, types or holds a grid.
- **A record.** Nothing records which computer followed a panel or typed in it.
  The `watchers` event tells the owner which computers follow now.
- **Availability.** Output can be dropped and a new screen can replace it. The
  handle drops typed bytes while its link is away. A link that read no frame
  for 30 seconds ends, so a computer that went away leaves the list in about
  half a minute. A grid that another computer asked for stays until the person
  of the owner types in that panel.
- **A change that the person did not save.** A change of a switch in the
  Settings window applies before **Save**. A person who closes the window
  without **Save** gets the old value back, so a switch that was cleared that
  way shares again.
