# The cards of an agent that runs on a machine

An agent program runs in a `tmux` session of a machine of the machine list. It
draws a card in the workspace of that machine, and the person presses a button
on the card. No Horizon program runs on the machine, the machine opens no
connection to this computer, and the machine states no identity.

The trust boundary is
[the boundary page](../review/trust-boundaries/machine-agent-cards.md). The card
itself is the boundary of
[`docs/review/trust-boundaries/block-panel.md`](../review/trust-boundaries/block-panel.md),
and the agents of a machine without a panel are the design of
[`docs/architecture/machine-watch.md`](machine-watch.md).

## Slice 1

Slice 1 carries the byte stream of one agent of a machine. It does not carry a
wake line, a browser tool, a device tool, an `open_terminal`, and no machine-list
or key operation of a machine agent.

| Step | Where |
|---|---|
| The socket path of one session on the machine, and here | `crates/horizon-core/src/node_cards/paths.rs` |
| The identity `horizon:machine/<machine>/<session>` | `paths::actor`, `paths::actor_parts` |
| The relay command, the MCP entry and the words of the agent | the text of the machines skill, which the setup runs on the machine; no Rust of this build writes them |
| The card contract over one stream, one tool, seven operations | `crates/horizon-browser-mcp/src/node.rs`, `server.rs` (`for_cards`), `controller/block.rs` |
| The `ssh` child of one machine, and its `-R` of each session | `HorizonApp::reconcile_card_sockets`, `node_cards::forward_args` |
| One listener for each card socket | `crates/horizon-ui/src/app/node_cards.rs` |
| The sessions to serve: every `tmux` session of the user | `HorizonApp::reconcile_card_sockets`, the `S` line of the text of the watch |
| The relay program of a machine, and the entry of the agent | the section of the machines skill; no Rust writes it |
| The removal of a stale socket file before each start | `node_cards::prune_args`, and its call in the child loop |

## The identity, and the mark of a setup

- **One identity for each session, minted here.** It is
  `horizon:machine/<machine>/<session>`, built from the machine name and the
  session name after `block_machines` checked both. The remote side has no field
  in which to state an identity: its bytes arrive on the socket whose path
  Horizon chose, and the server of that socket starts with the identity that
  Horizon chose.
- **A machine has its cards set up when its record says so.** The field is
  `cards` of the machine record
  (`crates/horizon-core/src/machines/record.rs`), `false` by default, and
  `machine_set` writes it. A machine whose record does not say so gets no socket
  and no child.
- **The sessions are the `tmux` sessions of the user of the machine.** The text of
  the watch names every one of them, also a session whose program is no agent and
  whose pane reports no state, and a machine with the mark gets one socket for
  each name. The rows of the sidebar are still the agents and the panes that
  report a state: an agent program that ends leaves its session standing, so its
  socket and its card link do not change. With the setting `machines.watch_agents`
  off, the watch reports no session, so Horizon serves no card socket and an agent
  of that machine reaches nothing.

## The workspace of the cards of a machine

The first card of a machine makes the workspace of that machine. That workspace
starts free, and a card takes the height that its content needs, as the card of
a panel's workspace does: a preset of this build owns the size of every panel in
it (`crates/horizon-ui/src/app/card_fit.rs`), so a card in a preset keeps the
cell of that preset and its content can pass the lower edge of the cell. A
workspace that holds no panel has no arrangement of the person's yet, so nothing
of the person is dropped there. A layout that the person picks in the toolbar of
that workspace stays theirs, and the cards that arrive after it keep the cells of
that layout.

## What a frame does

A frame reads the watch snapshot, compares one number (the snapshot version and
the names of the machines with the mark), and sends the set of sessions to the
supervisor thread of `crates/horizon-ui/src/app/node_cards.rs` only when that
number changed. The supervisor binds a new socket, joins a socket of a session
that left, and removes its file. No frame binds a socket and no frame starts a
thread.

## The child of one machine

One second `ssh` child for each machine that has the mark carries one
`-R <socket on the machine>:<socket here>` for each session that the watch
reports there, with `-N -T -o ExitOnForwardFailure=yes
-o StreamLocalBindUnlink=yes` and the login options of a terminal of that
machine. The machine opens no connection and is given no address of this
computer.

A child that ends leaves one socket file for each session on the machine, and the
`sshd` of a machine binds no streamlocal path that a file still holds: its unlink
option, `StreamLocalBindUnlink`, is the option of that `sshd` and not of this
child. `sshd` binds nothing in a folder that is not there either, and a fresh
machine holds neither `/tmp/horizon-cards` nor the folder of its machine. Before
each start, the child therefore asks its own login for one call, under the same
route, key, batch mode and a ten-second bound, that makes both folders, sets both
to mode 700, and then removes exactly the paths that it will bind. That is one
extra connection of the machine for each start; the paths and the two folders are
built from the checked names, so this call needs no quote. Both folders belong to
the login user that made them, which is what limits the cards to one login user of
one machine: the boundary page states the limit and why a folder of a user needs a
call that this child cannot make before it binds.

A child that ends waits the delay of `place::reconnect_delay` and starts again,
and the words of `ssh` name the refusal in the log and in one quiet line under
the row of the machine, `Cards: no link · trying again`, whose hover and spoken
text is the sentence of that refusal and the last line of those words, as one line
of visible text of the machine, cut at 200 characters. The line goes when the next child starts,
so a machine that refuses keeps saying so and the line of one that recovers
goes. The child of a machine ends and starts again when the set of its sessions
changes, because its argv changed with it. The stop of the window kills the
child of every machine and joins its loop, so no child outlives Horizon.

A machine with no known session gets no socket and no child: the watch is the
only source of the sessions of a machine, so with `machines.watch_agents` off a
machine with the mark carries nothing.

## The relay of a machine

The setup writes one relay program for the account of the machine
(`~/.horizon-cards/relay.py`, mode 700), and every entry of an agent program of
that machine starts `python3 ~/.horizon-cards/relay.py <machine>`. The program is
the text of the machines skill, so the fixture tests read it out of that text, and
the relay that a person writes is the relay that those tests prove.

A link that ends leaves the relay alive. It answers every call that waited for the
link with the error `-32000` and the words `The link to Horizon was lost. Call
again.`, and it reads an `OSError` of `recv` as a lost link too: a hard cut of a
forward resets the stream, and a relay without that reading ended with a traceback
and took the entry of the agent program with it. The relay then meets the next
stream of the socket: the remembered `initialize` and its
`notifications/initialized` go before the calls that waited, so the far end sees
one hello for the new stream, and the answer of that hello never reaches the agent.
The agent program calls again.

An entry of an agent program holds a start bound where its program has the field,
and the setup sets it to 60 seconds: the socket of a new session comes about one
second after the session, so the bound of a program covers a machine that answers
slowly. An entry that failed at its start needs a restart of the agent program.

## What is not built yet

- **A card of a machine session** is built: the second kind of caller is
  `block_requests::CardCaller`, whose session variant carries the machine name
  that the actor of the request was checked to hold. Its first card makes the
  workspace of the machine, its card takes a free place instead of one beside a
  panel, its presence is the session that the watch snapshot of that machine
  names, row or no row, and its footer names `<machine> · <session>` in Horizon's
  own words. A card whose session is gone says so in the words of that session,
  through `block_widget::gone_lines`, and keeps the words of a local card for the
  owner of a panel of this computer.
  The chip of its titlebar names the machine, through `CardChip::on_machine`.
- **The setup on the machine.** The probe of the relay program, the writer of
  the agent, the merge with one backup, the `Remove` direction and the text that
  the agent on the machine reads are the section of the machines skill and the
  button of the machine card.
- **The live proof** is the fixture of `scripts/device-smoke`: one session of its
  private `tmux` server runs `machine_card_agent.py`, and its stand-in `ssh`
  carries that session's socket as `sshd` carries it on a machine. The procedure
  is
  [`docs/testing/procedures/machine-agent-cards.md`](../testing/procedures/machine-agent-cards.md).
  A real `sshd` and a real agent program of a machine still need a rented machine.

## The bounds

| Bound | Value |
|---|---|
| The bytes of one socket path | `MAX_SOCKET_PATH_BYTES` 100, because macOS takes 104 |
| The connections of one socket at a time | 4 |
| The operations of the one tool on a socket | the seven card operations |
| The tools on a socket | `block_panel` alone, with the instructions of a card socket |
| The seconds that the relay waits for a link, once | 30 |
| The start bound of the entry of an agent | 60 seconds, where the program has the field |
| The characters of the words of `ssh` of a refused card link | 200 |
