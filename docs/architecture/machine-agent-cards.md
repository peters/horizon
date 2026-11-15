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
| The sessions to serve, from the watch snapshot | `HorizonApp::reconcile_card_sockets` |

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
- **The sessions come from the watch.** A session that the watch reports for a
  machine with the mark gets one socket here. With the setting
  `machines.watch_agents` off, the watch reports no session, so Horizon serves no
  card socket and an agent of that machine reaches nothing.

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

A child that ends waits the delay of `place::reconnect_delay` and starts again,
and the words of `ssh` name the refusal in the log. The child of a machine ends
and starts again when the set of its sessions changes, because its argv changed
with it. The stop of the window kills the child of every machine and joins its
loop, so no child outlives Horizon.

A machine with no known session gets no socket and no child: the watch is the
only source of the sessions of a machine, so with `machines.watch_agents` off a
machine with the mark carries nothing.

## What is not built yet

- **A card of a machine session.** The host resolves a caller to an agent panel
  of its board today. A request whose actor is a machine session must become the
  second kind of caller: its workspace, `beside: None`, its presence from the
  watch snapshot, and a footer that names `<machine> · <session>`.
- **The setup on the machine.** The probe of the relay program, the writer of
  the agent, the merge with one backup, the `Remove` direction and the text that
  the agent on the machine reads are the section of the machines skill and the
  button of the machine card.
- **The live proof.** The fixture of `scripts/device-smoke` and the procedure
  `docs/testing/procedures/` do not cover a card of a machine yet.

## The bounds

| Bound | Value |
|---|---|
| The bytes of one socket path | `MAX_SOCKET_PATH_BYTES` 100, because macOS takes 104 |
| The connections of one socket at a time | 4 |
| The operations of the one tool on a socket | the seven card operations |
| The tools on a socket | `block_panel` alone |
