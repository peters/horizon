---
applyTo: "crates/horizon-ui/src/app/block_requests.rs,crates/horizon-ui/src/app/block_requests/**,crates/horizon-ui/src/app/block_clicks.rs,crates/horizon-ui/src/app/block_clicks/**,crates/horizon-browser-mcp/src/server.rs,crates/horizon-browser-mcp/src/controller/block.rs"
---

# Trust boundary of a card of an agent on a machine

An agent program runs in a `tmux` session of a machine of the machine list. It
draws a card in the workspace of that machine, the person presses a button, and
the agent reads the press. No Horizon program is installed on the machine, the
machine opens no connection to this computer, and the machine states no
identity.

The design is in [`docs/architecture/machine-agent-cards.md`](../../architecture/machine-agent-cards.md).
The card itself stays the boundary of [`block-panel.md`](block-panel.md): this
page states only what the machine adds.

## What Horizon starts, and what the machine gets

- **One second `ssh` child per machine that has cards set up.** The watch child
  of [`machine-watch.md`](machine-watch.md) is unchanged. The card child carries
  one `-R <socket on the machine>:<socket on this computer>` for each session
  that the watch reports for that machine, with `-o BatchMode=yes`,
  `-o ExitOnForwardFailure=yes` and `-o StreamLocalBindUnlink=yes`, and the
  login options of a terminal of that machine.
- **The link leaves this computer only because Horizon started it.** The machine
  reads no file and no port of this computer. It cannot open a connection here,
  because it is given no address and no port: it has one socket path on its own
  disk, and the `sshd` of the machine makes it.
- **One identity for each session, minted on this computer.** It is
  `horizon:machine/<machine>/<session>`, built from the machine name and the
  session name after [`block_machines`] checked both. The remote side has no
  field in which to state an identity: its bytes arrive on the socket whose path
  Horizon chose, and the server of that socket is started with the identity that
  Horizon chose for it.
- **The machine gets no secret.** The socket path is a name. The folder on the
  machine is `0700` and the remote socket is `0600`, so another login user of
  that machine cannot reach either one.

## Untrusted: every byte that arrives on a card socket

Everything on the machine is untrusted: the agent process, its configuration,
the relay program, the socket path that it connects to, that `sshd` and that
user account. The bytes that arrive on the socket are an MCP message stream of
that machine, and they reach the same validators as a message of a local agent
panel:

- the document is checked in the MCP server and again in the UI, with the
  bounds of [`block-panel.md`](block-panel.md), and no field of it is a path, a
  command or an address that Horizon opens by itself;
- a request names one card of the workspace of its own identity and no card of
  another workspace;
- a request is bounded to 96 KiB, a queue to 32 waiting requests, and a card to
  8 cards of one owner.

## What the socket does not open

- **Only `block_panel` is listed, and inside it only the card operations.**
  `create`, `update`, `list`, `inspect`, `next_click`, `visibility` and `close`
  pass the guard. Every other operation of that tool, such as `open_terminal`,
  `open_panel`, `machine_set`, `machine_delete`, `machines`, `secret_names`,
  `machine_key`, `machine_keys`, `delete_machine_key`, `server_status`,
  `browser_host_status`, `presets`, `server_link_request` and
  `browser_host_request`, answers the typed refusal `cards_only`. The other 27
  tools of the server are not listed and cannot be called.
- **No card operation of a machine reaches a file of the person.** A machine
  agent cannot change the machine list, a key, a setting or a secret, because
  the operations that do that are not served on the socket.
- **No wake line is typed for a machine agent.** There is no terminal of this
  computer to write into, so the owner of such a card is never present as a
  panel, and the click waits its five minutes for the `next_click` of the agent.

## What a taken-over machine can do, and what it cannot do

It can:

- draw a false card in the workspace of its own machine, with false text and
  false states, up to the bounds of a document;
- press nothing: a press is the person's, and the agent reads it;
- fill the click queue of its own cards, and stop its own link;
- send a document that names another card of its own workspace in `update`,
  `visibility` or `close`, which is `not_owner`.

It cannot:

- act as another machine or as another session of its own machine;
- reach a card of another workspace, a local panel, a terminal or a panel of
  this computer;
- start a program, open a panel or a terminal, or run a command on this
  computer;
- read a key, a credential, a secret, a setting or a path of this computer;
- write a file of this computer outside the private queue of the card requests.

## Does not guarantee

- **Protection from another process of the same login user of this computer.**
  The queue of a card request is a `0600` file under the runtime root, and the
  identity of a request is a field of that file. A process of this user can
  write one, as it can for a local agent card.
- **The truth of the agent's words.** A card of a machine names its machine and
  its session in Horizon's own footer, and nothing proves that the program that
  drew it is the agent that the session names.
- **Availability.** A lost link ends the call and loses an in-flight
  `next_click`. No card is drawn while Horizon is closed, because the result
  file of a request never leaves this computer.
- **Anything about the `sshd` of the machine beyond OpenSSH's own host key.** A
  machine that refuses a streamlocal forward is refused with the typed reason of
  the words of `ssh`, and its cards do not exist.
- **The card while the watch setting is off.** The sessions of a machine come
  from the watch snapshot. With `machines.watch_agents` off, Horizon knows no
  session of a machine, so it forwards no card socket and a relay that connects
  gets nothing. The relay answers `relay_missing` on its own side only when it
  finds no program to run.
