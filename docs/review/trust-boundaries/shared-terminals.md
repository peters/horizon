---
applyTo: "crates/horizon-sync/src/live.rs,crates/horizon-sync/src/live/**,crates/horizon-sync/src/supervisor/live.rs,crates/horizon-sync/tests/live.rs,crates/horizon-pty-tap/src/share.rs,crates/horizon-pty-tap/src/share/**,crates/horizon-core/src/terminal/share.rs,crates/horizon-core/src/terminal/share/**,crates/horizon-core/src/terminal/follow.rs,crates/horizon-core/src/terminal/follow/**,crates/horizon-core/src/terminal/events.rs,crates/horizon-core/src/terminal/resize.rs,crates/horizon-core/src/terminal/lifecycle.rs,crates/horizon-core/src/panel/follow.rs,crates/horizon-core/src/panel/follow/**,crates/horizon-core/src/board/follow.rs,crates/horizon-core/src/config/server.rs,crates/horizon-ui/src/app/shared_terminals.rs,crates/horizon-ui/src/app/shared_terminals/**,crates/horizon-ui/src/app/sidebar/computers.rs,crates/horizon-ui/src/app/settings/server/sharing.rs,crates/horizon-ui/src/app/settings/server/sharing/**,docs/architecture/shared-terminals.md"
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
  The grid of a `screen` is 1 to 1000 columns and rows.
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
- **A follower takes no resize.** A resize of the panel changes nothing of a
  follower, and no size goes to the program. Only a `screen` event sets the
  grid of a follower.
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
- **The server names the sender, and the owner does not read it.** A `typed`
  event holds the device name and the label of the link that sent the bytes.
  The window does not use them.

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
- **The check of `typed` at the owner.** `HorizonApp::shared_typed` of
  `crates/horizon-ui/src/app/shared_terminals/owner.rs` writes the bytes only
  when four conditions are true:
  1. `share_terminals` is `true`.
  2. `remote_typing` is `true`.
  3. The identifier of the event is in the list that the link has.
  4. A panel of the board has that identifier.
- **A switch that is off shares nothing.** `HorizonApp::shared_list` of the
  same file gives an empty list while `share_terminals` is `false`. The server
  then ends each follow of a panel of this computer.
- **Only a panel of the list has a watch.** `HorizonApp::shared_watchers` of
  the same file returns for a panel that is not in the list. No event of a
  server therefore starts the copy of another panel.
- **A follow panel is never shared.** `shared_list` leaves out each terminal
  that follows a stream, so two computers do not send one terminal in a circle.
- **A watched panel says so.** The title bar of a panel that another computer
  follows shows `seen by <name>`, or the count of the followers
  (`SharedTerminals::chip` of `crates/horizon-ui/src/app/shared_terminals.rs`).
- **Each shown text is cut.** `shown` of
  `crates/horizon-ui/src/app/shared_terminals.rs` removes control characters
  and keeps 80 characters. The section of the sidebar, the chips and the reason
  of a lost link use it.
- **No answer of a follower to the program.** `Terminal::write_protocol` of
  `crates/horizon-core/src/terminal/lifecycle.rs` writes nothing for a follower.
  `Terminal::answer_query` of `crates/horizon-core/src/terminal/events.rs`
  returns before it. The bytes of the person go to the feed and never to the
  link.
- **No clipboard.** `Terminal::handle_event` of
  `crates/horizon-core/src/terminal/events.rs` drops the copy request of a
  follower.
- **No resize.** `Terminal::resize_with_policy` of
  `crates/horizon-core/src/terminal/resize.rs` returns for a follower.
  `FollowFeed::screen` is the one place that changes its grid.
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
  that answered version 6 or higher.
- **A computer gets only the streams that it asked for.** `Inner::accept` of
  the same file drops a `screen` and `output` of a panel that the application
  did not ask to follow. The mark `own` of a list never comes from a frame.
- **No call waits for I/O.** No call of `LiveHandle` and no call of `FollowFeed`
  waits for a link, so the UI thread can call them.

## What the guarantees do not cover

- **Who can follow.** Each computer with a live link can follow each panel in
  the list of each other computer, up to the limits. The server has no
  permission for one computer, and an owner cannot refuse one follower. One
  switch shares each terminal panel of a computer or none. No setting keeps one
  panel out, and the list also holds the agent panels and the `ssh` panels.
- **Who can type.** Each follower of a panel with `typing: true` can type in it.
  A person who controls one linked computer can therefore run commands on each
  linked computer that shares a terminal with typing on. The owner does not
  read the sender of a typed event, and its window shows no mark while another
  computer types.
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
  server gave for a follower. A label is text that a computer chose, so the
  chip does not prove which computer follows.
- **A record.** Nothing records which computer followed a panel or typed in it.
  The `watchers` event tells the owner which computers follow now.
- **Availability.** Output can be dropped and a new screen can replace it. The
  handle drops typed bytes while its link is away. The live link has no
  keepalive, so a computer that went away can stay in the list.
- **A change that the person did not save.** A change of a switch in the
  Settings window applies before **Save**. A person who closes the window
  without **Save** gets the old value back, so a switch that was cleared that
  way shares again.
