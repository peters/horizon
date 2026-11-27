# Cards of the orchestrator of a workspace

The orchestrator of a workspace is the agent program that the orchestrator dock
runs with no terminal (`crates/horizon-ui/src/app/orchestrator_dock/`). This page
states its identity, what its program gets at a start, how the host accepts it,
which operations of `block_panel` it reaches, and what this part does not build.

The card itself keeps the rules of
[`../review/trust-boundaries/block-panel.md`](../review/trust-boundaries/block-panel.md).
The program and its line format keep the rules of
[`agent-bridge.md`](agent-bridge.md).

## The actor

The actor of the orchestrator of the workspace with local id `W` is
`horizon:orchestrator/<W>`. The module
[`crates/horizon-core/src/agent_bridge/actor.rs`](../../../crates/horizon-core/src/agent_bridge/actor.rs)
builds it and parses it.

- Only a local id of the form that `new_local_id` makes is accepted: 36 bytes of
  lower-case hexadecimal in groups of 8, 4, 4, 4 and 12, divided by hyphens.
- The form refuses every other text. An actor therefore holds no slash, no dot,
  no space and no control character, so the name of a person or of a file cannot
  be spelled as an actor.
- The prefix differs from the actor of a panel, `horizon:<local id>`, and from
  the actor of a session of a machine, `horizon:machine/<machine>/<session>`.
  One actor names one caller.
- A workspace whose local id this form refuses gets no actor, and its
  orchestrator draws no card: the host answers `caller_unavailable`.

## What the child gets at a start

`agent_bridge::program::claude_for_workspace` builds the program of one
conversation. It is `agent_bridge::program::claude` plus the identity of the
workspace:

- the `--plugin-dir` of Claude Code, which is the directory that an agent panel
  of that kind gets, so the program has the Horizon MCP server and the Horizon
  skills. The argument is added only while that directory is a directory of this
  computer;
- the environment variables that an agent panel gets, from the one function that
  an agent panel uses (`agent_identity_vars`): `HORIZON=1`,
  `HORIZON_BROWSER_ACTOR=horizon:orchestrator/<W>`,
  `HORIZON_BROWSER_HOST_INSTANCE` and `HORIZON_EXECUTABLE`.

The `BridgeIdentity` of a start holds the actor, the host instance, the home of
this Horizon, its executable and the plugin directory. `HORIZON_HOME` is not
written into the environment of the child: the child inherits it from this
Horizon as a panel does, so the tools of the orchestrator read the folder that
the host reads and keep the rule that a panel's home keeps.

The start adds no work-resume plugin, no `HORIZON_WORK_*` variable and no named
secret, because this part has no turn ledger, no panel of the board and no key
of the person.

## The caller rule

The host maps an actor to a caller before it applies a request
(`crates/horizon-ui/src/app/block_requests.rs`, `request_caller`).

- A panel actor is the caller `Panel`, as before.
- An actor that parses as an orchestrator actor is the caller `Orchestrator`, and
  only while a workspace with that local id is on the board. A request of an
  unknown workspace is `caller_unavailable`, and the answer names the caller and
  no workspace.
- A machine session actor is the caller `Session`, as before.

A caller of `Orchestrator` has no panel, so:

- its card is placed by the rule that a card of a machine session uses: the card
  is a `CardPlace::Tile` of that workspace, and the board puts it on a free tile
  that covers no panel of the workspace
  (`crates/horizon-core/src/board/arrangement.rs`, `free_tile_position`). The
  column of an agent panel and the `beside` anchor of that column are not
  offered, because no panel of this caller exists;
- an operation that needs the panel of the caller is refused with the refusal
  that a caller without a panel gets, `caller_unavailable`.

The owner of a card of an orchestrator is the actor, so `update`, `visibility`,
`close` and `next_click` require that exact actor, and another agent of the same
workspace gets `not_owner`.

## The presence of the owner

`card_owner_present` answers `true` for a card of an orchestrator while the dock
holds a running bridge for that workspace
(`OrchestratorDock::running`), and the state chip of the card names the program
of the dock and says whether it listens. While no bridge runs, the chip says
`No agent` and every control of the card is drawn disabled.

Horizon types no wake line for this owner, because the owner has no terminal. An
orchestrator must keep a `next_click` open while a card of its own waits for the
person, as an agent of a session of a machine does. A press that no wait of the
owner reads stays in the queue of the card for five minutes, 32 presses per card,
and nothing tells the program that the press came.

## The operations of this caller

| Operation | This caller | Reason |
| --------- | ----------- | ------ |
| `create`, `update`, `list`, `inspect`, `next_click`, `visibility`, `close` | works | the card operations need the workspace of the caller and no panel |
| `open_terminal` | `caller_unavailable` | the terminal stands beside the panel of the caller and carries the mark of that panel |
| `open_panel` | `caller_unavailable` | the panel of a preset stands beside the panel of the caller, and no panel of this caller exists |
| `presets` | works | a read of the names of the settings, which needs no actor |
| `machine_set`, `machine_delete`, `machines`, `machine_key`, `machine_keys`, `delete_machine_key` | works | the machine list of this Horizon takes no actor, and the orchestrator rents machines of it |
| `secret_names` | works | the answer is the names of the stored secrets and no value |
| `server_status`, `browser_host_status` | works | a read of the entry that the settings hold |
| `server_link_request`, `browser_host_request` | `caller_unavailable` | the card of a setting request belongs to a panel of the workspace, and its answer reaches that panel |
| the browser page tools, `browser_*` | unchanged, no panel | the actor of an orchestrator is in no workspace stamp, so `browser_list` names no panel, `browser_create` answers `workspace_unavailable`, and every other page operation answers the refusal of a caller outside the workspace of the panel |
| `device_panel` | unchanged, `caller_unavailable` | a Device request resolves its actor to an agent panel of the board |

Nothing in this table is a right that an agent panel does not have.

## What is not built

- **No wake.** A `next_click` is the only way that a press reaches the
  orchestrator, as for an agent of a machine session.
- **No row in the dock.** The dock draws its conversation and no reference to a
  card: the person finds a card of the orchestrator in the canvas and in the
  sidebar, as every card.
- **No named secret.** A later part decides whether the orchestrator gets a key
  of the person.
- **No work ledger.** The program gets no work-resume plugin and no
  `HORIZON_WORK_*` variable, so the attention of the dock is the state of the
  conversation and not the state of a turn of a panel.
- **No persistence.** Nothing here is saved: the conversation, the bridge and the
  presence of the owner end with this run of Horizon.
