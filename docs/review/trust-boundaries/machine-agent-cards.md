---
applyTo: "crates/horizon-core/src/node_cards.rs,crates/horizon-core/src/node_cards/**,crates/horizon-ui/src/app/block_requests.rs,crates/horizon-ui/src/app/block_requests/**,crates/horizon-ui/src/app/block_clicks.rs,crates/horizon-ui/src/app/block_clicks/**,crates/horizon-browser-mcp/src/node.rs,crates/horizon-browser-mcp/src/node/**,crates/horizon-browser-mcp/src/server.rs,crates/horizon-browser-mcp/src/controller/block.rs"
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
  one `-R <socket on the machine>:<socket on this computer>` for each `tmux`
  session of the user that the watch reports for that machine, with
  `-o BatchMode=yes`, `-o ExitOnForwardFailure=yes` and
  `-o StreamLocalBindUnlink=yes`, and the login options of a terminal of that
  machine.
- **One removal before each start of that child.** A child that ended leaves one
  socket file for each session on the machine, a machine binds no path that a file
  holds, and `sshd` binds nothing in a folder that is not there. Before each start,
  the same login makes `/tmp/horizon-cards` and the folder of that machine, sets
  both to mode 700, and runs one `rm -f` whose paths are exactly the socket paths
  of that machine's sessions, built here from the machine name and the session
  names after their rules. It removes no other file, it takes no value from the
  text of the machine, and a machine that does not answer costs at most ten seconds
  of the start. Both folders belong to the one login user of the machine that made
  them; the limit that this costs is under [does not guarantee](#does-not-guarantee).
- **A relay program that the person's own account holds.** The setup writes
  `~/.horizon-cards/relay.py`, mode 700, and every agent program of that machine
  starts it. It is one readable python program: it reads the session name from the
  `tmux` server of the account, it opens one Unix socket of that machine and no
  network connection, it forwards the standard input and output of its agent, and
  it holds no secret, no key and no code of Horizon. The machine may change it,
  and every byte that it sends arrives as untrusted.
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
- **The instructions of a card socket are text of this build.** A card socket
  answers `initialize` with the words of the cards alone: the one tool, the press,
  the card operations, the open `next_click`, and a call again after a lost link.
  No word of them comes from the machine.

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
  the words of `ssh`, and its cards do not exist. The last line of those words
  reaches the log and the hover of the quiet line as one line of visible text,
  cut at 200 characters, because that text is the machine's.
- **A relay program that a machine alters.** The program is the person's file on
  the person's account, and the setup cannot prove what it runs later. Its bytes
  cross this boundary as the bytes of any agent of that machine.
- **The card while the watch setting is off.** The sessions of a machine come
  from the watch snapshot. With `machines.watch_agents` off, Horizon knows no
  session of a machine, so it forwards no card socket and a relay that connects
  gets nothing. A machine with no program to relay its stream gets no entry at
  all: that refusal is the text of the skill, which writes nothing, and this
  build holds no code for it.
- **The cards of a second login user of one machine.** One login user of a
  machine can have cards. The folder of the sockets is `/tmp/horizon-cards`, and
  the first login user that sets the cards up makes it with mode 700, so a second
  login user of that machine cannot make its folder there and gets no socket. The
  folder of a user would be `/tmp/horizon-cards-<uid>`, and Horizon would have to
  name that path before it starts the child, because the `-R` of the child carries
  the path that `sshd` binds: a build that made the folder per user needs one
  extra call of the machine for the id of the user before each start of a child,
  and a machine that answered slowly would hold the sockets of every machine of
  the list while it did. This build keeps the single folder, and the skill says
  the limit to the person who asks for the cards of a second account.
